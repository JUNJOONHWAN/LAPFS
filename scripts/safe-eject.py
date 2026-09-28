#!/usr/bin/env python3
"""Normal LAPFS unmount followed by an exclusive, positive handoff receipt."""
import argparse
import json
import os
import pathlib
import subprocess
import sys
import time


def fail(message):
    raise RuntimeError(message)


def mounted_at(path):
    result = subprocess.run(
        ["findmnt", "-n", "-o", "SOURCE,FSTYPE", "--mountpoint", str(path)],
        capture_output=True, text=True, check=False,
    )
    if result.returncode == 1 and not result.stdout.strip():
        return None
    if result.returncode != 0:
        fail(f"Cannot inspect mountpoint: {result.stderr.strip()}")
    parts = result.stdout.strip().split()
    if len(parts) != 2:
        fail("Ambiguous mountpoint state")
    return parts


def matching_mount_process(target, offset, mountpoint, session):
    expected = ["mount-rw", str(target), str(offset), str(mountpoint), str(session)]
    matches = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            args = [os.fsdecode(v) for v in (entry / "cmdline").read_bytes().split(b"\0") if v]
        except (OSError, PermissionError):
            continue
        if len(args) == 6 and args[1:] == expected and pathlib.Path(args[0]).name == "lapfs":
            matches.append(int(entry.name))
    if len(matches) > 1:
        fail("More than one LAPFS writer matches this handoff")
    return matches[0] if matches else None


def process_finished(pid):
    proc = pathlib.Path(f"/proc/{pid}")
    if not proc.exists():
        return True
    try:
        # A child can remain as a zombie until its caller reaps it. It has
        # already closed the device and completed FUSE destroy at that point.
        state = (proc / "stat").read_text().rsplit(") ", 1)[1][0]
        return state == "Z"
    except OSError:
        return False


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("target", type=pathlib.Path)
    parser.add_argument("offset", type=int)
    parser.add_argument("mountpoint", type=pathlib.Path)
    parser.add_argument("session", type=pathlib.Path)
    args = parser.parse_args()
    if args.offset < 0 or not all(p.is_absolute() for p in (args.binary, args.target, args.mountpoint, args.session)):
        fail("Use absolute paths and a nonnegative offset")
    mount = mounted_at(args.mountpoint)
    if mount is not None:
        if mount != ["LAPFS-buffered", "fuse"]:
            fail(f"Refusing to unmount another filesystem: {mount}")
        pid = matching_mount_process(args.target, args.offset, args.mountpoint, args.session)
        if pid is None:
            # A dead FUSE daemon can leave a disconnected mount entry. A
            # normal unmount is still required before exclusive recovery.
            for entry in pathlib.Path("/proc").iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    argv = [os.fsdecode(v) for v in (entry / "cmdline").read_bytes().split(b"\0") if v]
                except OSError:
                    continue
                if len(argv) == 6 and argv[1] == "mount-rw" and argv[4] == str(args.mountpoint):
                    fail("A different live LAPFS writer owns this mountpoint")
        unmount = subprocess.run(["fusermount3", "-u", str(args.mountpoint)], capture_output=True, text=True)
        if unmount.returncode != 0:
            fail(f"Normal unmount refused; leave the drive attached: {unmount.stderr.strip()}")
        for _ in range(300):
            if (pid is None or process_finished(pid)) and mounted_at(args.mountpoint) is None:
                break
            time.sleep(0.1)
        else:
            fail("FUSE has not finished closing; leave the drive attached and check the session")
    check = subprocess.run(
        [str(args.binary), "handoff-ready", str(args.target), str(args.offset), str(args.session)],
        capture_output=True, text=True,
    )
    if check.returncode != 0:
        fail(f"Handoff refused; leave the drive attached: {check.stderr.strip()}")
    receipt = json.loads(check.stdout)
    if receipt.get("status") != "ready_to_disconnect" or receipt.get("session", {}).get("closed") is not True:
        fail("Handoff command did not return a positive closed-session receipt")
    print(json.dumps(receipt, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    try:
        run()
    except Exception as exc:
        print(f"LAPFS safe eject failed: {exc}", file=sys.stderr)
        sys.exit(1)

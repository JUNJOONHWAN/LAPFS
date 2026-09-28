"""Independent Apple validation of disposable images made inside evidence/.
No physical disk path is accepted as input. hdiutil device IDs come only from
the just-created image's attach receipt and are always detached in finally.
"""
from pathlib import Path
import hashlib
import json
import plistlib
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(tempfile.mkdtemp(prefix="macos-qa-", dir=ROOT / "evidence"))
BIN = ROOT / "target/debug/spark-apfs-safe"

def run(args):
    r = subprocess.run([str(x) for x in args], capture_output=True)
    if r.returncode:
        raise RuntimeError(r.stdout.decode(errors="replace") + r.stderr.decode(errors="replace"))
    return r.stdout

image = OUT / "case.dmg"
run(["hdiutil", "create", "-size", "64m", "-fs", "APFS", "-volname", "SPARK_SAFE_QA", "-layout", "GPTSPUD", image])
raw = image.read_bytes()
entries = struct.unpack_from("<Q", raw, 512 + 72)[0] * 512
count, stride = struct.unpack_from("<II", raw, 512 + 80)
matches = []
for i in range(count):
    e = raw[entries+i*stride:entries+(i+1)*stride]
    if e[:16].hex() == "ef57347c0000aa11aa1100306543ecac":
        matches.append(struct.unpack_from("<Q", e, 32)[0] * 512)
assert len(matches) == 1
offset = matches[0]
source = OUT / "payload"
expected = {}
results = []
cases = [
    ([{"op": "mkdir", "path": "/자료"}], None),
    ([{"op": "put", "source": str(source), "path": "/자료/원본.txt"}], "첫 번째 내용\n".encode()*100),
    ([{"op": "put", "source": str(source), "path": "/자료/원본.txt"}], "변경 후 내용\n".encode()*700),
    ([{"op": "rename", "path": "/자료/원본.txt", "name": "변경.txt"}], None),
    ([{"op": "put", "source": str(source), "path": "/empty.txt"}], b""),
    ([{"op": "remove", "path": "/자료/변경.txt"}], None),
]
# Grow beyond the former 8 MiB limit with bounded 4 MiB chunks.
chunk = bytes(range(256)) * (4 * 1024 * 1024 // 256)
cases.append(([{"op": "put", "source": str(source), "path": "/large.bin"}], chunk))
for chunk_no in range(1, 6):
    cases.append(([{"op": "append", "source": str(source), "path": "/large.bin", "expected_size": chunk_no * len(chunk), "source_offset": 0, "length": len(chunk)}], chunk))
cases.append(([{"op": "append", "source": str(source), "path": "/large.bin", "expected_size": 6 * len(chunk), "source_offset": 0, "length": 117}], b"last"*29+b"!"))
# Enough catalog entries to split multiple B-tree leaves, followed by deletes.
for group in range(3):
    cases.append(([{"op": "put", "source": str(source), "path": f"/자료/entry-{group*24+j:03}.txt"} for j in range(24)], b"catalog regression"*31))
for group in range(3):
    cases.append(([{"op": "remove", "path": f"/자료/entry-{group*24+j:03}.txt"} for j in range(24)], None))
for index, (batch, payload) in enumerate(cases):
    if payload is not None: source.write_bytes(payload)
    description = OUT / f"batch-{index}.json"
    description.write_text(json.dumps(batch, ensure_ascii=False))
    journal = OUT / f"journal-{index}"
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    run([BIN, "prepare", image, offset, description, journal, 16, 1024])
    assert hashlib.sha256(image.read_bytes()).hexdigest() == before
    status = json.loads(run([BIN, "status", journal]))
    run([BIN, "apply", journal])
    for action in batch:
        if action["op"] == "put": expected[action["path"]] = payload
        elif action["op"] == "append": expected[action["path"]] += payload
        elif action["op"] == "rename": expected["/자료/변경.txt"] = expected.pop(action["path"])
        elif action["op"] == "remove": expected.pop(action["path"])
    attached = plistlib.loads(run(["hdiutil", "attach", "-readonly", "-plist", image]))["system-entities"]
    parent = next(e["dev-entry"] for e in attached if e.get("content-hint") == "GUID_partition_scheme")
    try:
        container = next(e["dev-entry"] for e in attached if e.get("content-hint", "").startswith("EF57347C"))
        mount = Path(next(e["mount-point"] for e in attached if "mount-point" in e))
        fsck = run(["/sbin/fsck_apfs", "-n", container]).decode()
        (OUT / f"fsck-{index}.log").write_text(fsck)
        assert "appears to be OK" in fsck
        assert not any(marker in fsck.lower() for marker in ["error", "warning", "corrupt", "overallocation", "is invalid", "is not valid"])
        for path, content in expected.items(): assert (mount / path.lstrip("/")).read_bytes() == content
        for action in batch:
            if action["op"] in ("remove", "rename"): assert not (mount / action["path"].lstrip("/")).exists()
        denied = subprocess.run([str(BIN), "inspect", str(image), str(offset)], capture_output=True)
        assert denied.returncode != 0, "Attached-image guard failed"
    finally:
        run(["hdiutil", "detach", parent])
    run([BIN, "cleanup", journal])
    run([BIN, "cleanup", journal])  # terminal cleanup is resumable/idempotent
    results.append({"operation": action["op"], "operation_count": len(batch), "prepared_original_unchanged": True,
                    "apple_fsck": "clean", "native_read_matches": True,
                    "journal_bytes": status["journal_data_bytes"], "cleanup": "passed"})
    print(json.dumps(results[-1], ensure_ascii=False), flush=True)
(OUT / "results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
print("Evidence:", OUT)

# Operation, logs and recovery

## Read-only access

```bash
lapfs inspect /dev/disk/by-id/YOUR-PARTITION 0
lapfs mount-ro /dev/disk/by-id/YOUR-PARTITION 0 /path/to/empty-mount
```

Use the APFS partition, not the whole disk. `inspect` needs read permission and exclusive access; unmount other users first. Permission denied and device busy are different conditions.

## Physical writable beta

Only use an expendable test device with a verified backup. Identify the partition and UUID with read-only inspection. Enrollment itself does not modify APFS blocks.

```bash
sudo lapfs device-enroll /dev/disk/by-id/YOUR-PARTITION EXPECTED-CONTAINER-UUID
# Use the exact target path returned by enrollment:
sudo lapfs mount-rw /var/lib/lapfs/devices/UUID/target.lapfs-device.json 0 \
  /path/to/empty-mount /var/lib/lapfs/devices/UUID/session-01
```

The session parent must exist. It must be on a separately identified physical disk with local ext4/XFS, outside the APFS device. Direct raw `/dev/sdXN` writes are refused. Device identity, GPT identity, capacity, container UUID and checkpoint state are checked. Whole-disk writes and ambiguous/same-disk/volatile/network recovery storage are refused.

A sudo RW mount exposes the invoking SUDO_UID/SUDO_GID with private projected permissions. Close files before `sudo fusermount3 -u /path/to/empty-mount`. No forced/lazy unmount is part of the normal procedure.

## If a write or unmount fails

1. Stop application writes. Preserve the original APFS device and the internal session/queue/journal directory.
2. Inspect the error log. Do not delete owner records or retry using an unrelated session to bypass a lock.
3. Stop/unmount the failed FUSE instance normally. Verify no other process is using the device.
4. With the same enrolled device attached, run:

```bash
sudo lapfs mount-recover /var/lib/lapfs/devices/UUID/session-01
```

Recovery validates identity and recorded checksums, handles interrupted application, and replays acknowledged queued input. Committed work is retired without duplicate application. If recovery refuses, preserve its error and all state; do not attempt native repair on the only copy.

A cleanly closed session cannot be reused for a new mount: choose a new session directory. Until recovery completes, do not attach the volume to macOS as an independent writer. Offline `prepare/apply/recover/cleanup` commands have separate transaction semantics; see `lapfs help`.

## Diagnostic logs

| Execution | Location |
|---|---|
| root / sudo | `/var/log/lapfs/errors.jsonl` |
| ordinary user | `~/.local/state/lapfs/errors.jsonl` |

Two MiB per file, three rotated histories (`.1`–`.3`) plus the active file, about eight MiB total. Records contain UTC Unix milliseconds, PID, operation, errno and bounded error text. Mount-start records identify the target and recovery session. File payloads and passwords are not logged. Paths may be sensitive: redact them before posting an issue.

Expected `lookup` ENOENT is excluded. Write/flush/fsync/release/unmount and CLI recovery errors are recorded. Unsupported operations handled directly by FUSE's default callbacks may return ENOSYS without a custom diagnostic. SIGKILL, kernel crash and power removal cannot guarantee a final log record. Startup refuses if logging cannot initialize; runtime logging failures are reported on stderr without changing the original I/O result.

```bash
sudo tail -n 30 /var/log/lapfs/errors.jsonl
```

Diagnostic rotation never deletes recovery journals.

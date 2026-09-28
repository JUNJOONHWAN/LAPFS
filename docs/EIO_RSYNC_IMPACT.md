# LAPFS beta.5 EIO and rsync impact report

## 변경 전 영향 보고서

Authority: DGX Spark canonical source, starting commit `86341cf`. Current physical Corsair runtime at investigation start: beta.4 root-owned RW FUSE process. Existing APFS files, research backup, external recovery journal, unrelated services and schedules are outside the change.

Observed: a prior five-root walk recorded 4,414 directories, 160,646 files and 795 EIO lookups. A fresh beta.4 mount could stat previously failing names, consistent with a mount-lifetime lookup state problem, but that alone did not prove every EIO cause. Both RO/RW FUSE hosts imposed a 100,000 inode-path count; RW also imposed 4,096 open handles. These limits returned generic errors that FUSE exposed as EIO. On a disposable image, `rsync -a` returned 23 because `fchmod` on its temporary file and `utimensat` on the destination directory returned EOPNOTSUPP. `openat(O_CREAT|O_EXCL)` had succeeded.

Direct impact: inode lookup/forget/release, chmod and timestamps, symlink create/remove/rename, APFS inode metadata and volume counters, user-facing compatibility docs and reproducible image tests. The known 4 MiB durable input queue and 32 MiB undo/redo journal remain unchanged; they are durability/space bounds, not arbitrary APFS directory entry caps.

## 변경 후 영향 보고서

- Removed fixed inode-path and handle counts. FUSE `forget` and release reclaim lookup state. Host allocation failure returns ENOMEM. Unit test covers 125,001 inode entries and reference eviction.
- Implemented journaled chmod and atime/mtime for ordinary files and directories. `rsync` temporary-file permission and destination timestamps now succeed.
- Added symlink create/remove and same-directory rename/replace. Fixed APFS symlink count and filesystem-owned, NUL-terminated target encoding. macOS readback is an independent compatibility gate.
- Reproducible `tests/verify_rsync_archive.py` exercises first copy, no-op repeat, regular file replacement, symlink target replacement and `--delete` of a file, symlink and nonempty directory. It includes a 12,550,013-byte file, permissions, nanosecond mtime, SHA and original fixture preservation. `tests/verify_rsync_apple.py` independently checks the resulting image on macOS.
- `tests/verify_linux_fuse.py` now expects chmod success and still tests unsupported sparse writes, SIGKILL recovery, original 104-file preservation and error logs.
- No research file, APFS user file, scheduler or mail was modified during disposable-image tests. The physical Corsair transition and live EIO/full traversal remain pending in this report until the authenticated normal remount and subsequent QA are complete. Do not count the synthetic pass as that live result.

Limitations: differing uid/gid, special device nodes, xattrs, ACLs, hardlinks, unplug and power loss remain unqualified. A clean Apple `fsck_apfs` on one fixture does not certify all APFS layouts or hardware bridges.

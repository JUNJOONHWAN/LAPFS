# Beta limits and qualification plan

## Supported subset

Single unencrypted APFS volume; no snapshots; supported allocator geometry; ordinary unshared files. Basic reads, directory enumeration, symlink reads and creation/removal, range reads above 4 GiB; writes through a durable queue; create/copy/range overwrite/append, mkdir, rmdir of empty directories, unlink of closed files, same-volume file and directory move/rename/replace, chmod and atime/mtime, fsync, close and normal unmount.

New truncate size is limited to 8 MiB. Large file copy/append/range-write does not have an 8 MiB total-file limit. Input queue payload cap is 4 MiB, active undo+redo cap is 32 MiB, reserve is 1 GiB with 96 MiB working headroom. These are separate limits, not a total RAM guarantee. There may be more than one retained failed session; do not automatically delete it to free space.

## Explicitly unsupported

- Multiple volumes, snapshots, encrypted writes, pending revert, unsupported incompatible feature bits and CAB indirection.
- Shared/cloned/hardlinked/compressed/sparse/special/immutable/append-only file mutation and unvalidated extended attributes.
- Non-empty directory removal; open-target replacement and open-file unlink (EBUSY). Cross-directory moves and grouped delete batching have image/FUSE/macOS fsck evidence in beta.16, but real TB-scale Corsair application is pending.
- Hardlinks, writable mmap, chown to a different owner, special device creation, arbitrary xattrs and ACL updates. Symlink chmod remains unsupported.
- Complete POSIX semantics, database suitability, arbitrary application save protocols, and full `cp -a` metadata preservation. `rsync -a` was verified only for regular files, directories and symlinks owned by the mounted user; device nodes and differing uid/gid remain unqualified.

## Still untested or unqualified

| Area | Required evidence before a stronger claim |
|---|---|
| Broader actual USB qualification | Selected device single-canary test passed; further long loads, other media and independent Apple fsck required |
| USB unplug / power loss | Repeatable failure injection across payload/queue/apply/flush/commit/recovery on multiple bridges |
| Flush reliability | Hardware that reports success before durable storage must be characterized |
| Real TB-scale catalogs | Long copies, random updates, full/fragmented storage, allocator/layout diversity |
| Long-running stress | Repeated remount, filled queues, log rotation, thermal/USB reset cases |
| Portability | Multiple kernels, Linux distributions, CPUs, macOS/APFS format variants |
| Security | Independent review and fuzzing of malicious filesystem metadata and privilege boundaries |
| Commercial suitability | Independent qualification, support/recovery process and broader compatibility |

There is no sustained USB 3.2 MB/s claim or recovery-time SLA. Small cached-image timings are not hardware throughput. Process SIGKILL tests do not model loss of internal disk power or USB controller caches.

## Durability boundary

Beta.7 copies modified file blocks, chunk bitmaps and CIBs, rotates the internal-pool bitmap, and writes checkpoint-local ephemeral objects. A dependency flush precedes ring NX publication; another flush precedes success. The previous active checkpoint and bootstrap remain unchanged. This improves Mac-first crash consistency for the tested supported subset; it does not guarantee hardware persistence when a device lies about flushes. Input accepted into the DGX queue may not yet exist on the USB device. Preserve journals and queues; stale recovery must not be forced over a Mac-modified generation. Diagnostic logs are not recovery data. See [exact evidence and limits](NATIVE_COW_IMPACT.md).

## Historical beta.2 real-volume blocker

A prepare-only canary on a roughly 1 TB volume exceeded the 32 MiB journal cap. Original APFS blocks were unchanged; RW activation did not proceed. Catalog-wide metadata work can exceed the budget even for a tiny new file. That beta.2 build was unsuitable for the tested volume. The beta.3 correction and remaining physical-device qualification are documented below.

## beta.3 candidate

The original 32 MiB limit is retained. Incremental metadata updates now pass the previously failing real-volume prepare simulation. A selected physical-volume canary later passed; see [current report](REAL_VOLUME_AFTER.md). Previous evidence above retains its original build identity.

## beta.5 compatibility gate

The disposable APFS test covers ordinary `rsync -a` initial transfer, identical repeat, changed-file atomic replacement, and `--delete` of a regular orphan. It checks nested directories, symlink target, permissions, nanosecond mtime, SHA and macOS readback. Linux success alone is not a release gate: Apple `fsck_apfs -n` must have no warning and original fixture files must retain their hashes. The physical Corsair beta.5 EIO and rsync gate is tracked separately in `docs/VALIDATION.md`.

## Historical Mac/Linux handoff limit (beta.6)

In beta.6, a positive `handoff-ready` receipt proves a normal DGX unmount/recovery/flush and exclusive read-only APFS parse at that moment. It does not prove that a USB bridge persisted a flush it falsely acknowledged. If the cable is pulled during an APFS transaction, the DGX-only external journal is required for recovery; Mac cannot apply it. The user must return the drive to the same DGX before a Mac writer touches it. Unqualified no-eject Mac-first use is not guaranteed safe.

# Beta limits and qualification plan

## Supported subset

Single unencrypted APFS volume; no snapshots; supported allocator geometry; ordinary unshared files. Basic reads, directory enumeration, symlink reads, range reads above 4 GiB; writes through a durable queue; create/copy/range overwrite/append, mkdir, unlink of closed files, same-directory file rename/replace, fsync, close and normal unmount.

New truncate size is limited to 8 MiB. Large file copy/append/range-write does not have an 8 MiB total-file limit. Input queue payload cap is 4 MiB, active undo+redo cap is 32 MiB, reserve is 1 GiB with 96 MiB working headroom. These are separate limits, not a total RAM guarantee. There may be more than one retained failed session; do not automatically delete it to free space.

## Explicitly unsupported

- Multiple volumes, snapshots, encrypted writes, pending revert, unsupported incompatible feature bits and CAB indirection.
- Shared/cloned/hardlinked/compressed/sparse/special/immutable/append-only file mutation and unvalidated extended attributes.
- Directory rename/delete; cross-directory rename; open-target replacement and open-file unlink (EBUSY).
- New links, writable mmap, chmod/chown, explicit timestamps, xattrs and ACL updates.
- Complete POSIX semantics, database suitability, arbitrary application save protocols, `cp -a` metadata preservation.

## Still untested or unqualified

| Area | Required evidence before a stronger claim |
|---|---|
| Actual USB writes | Expendable physical media, verified original/new hashes and independent Apple fsck |
| USB unplug / power loss | Repeatable failure injection across payload/queue/apply/flush/commit/recovery on multiple bridges |
| Flush reliability | Hardware that reports success before durable storage must be characterized |
| Real TB-scale catalogs | Long copies, random updates, full/fragmented storage, allocator/layout diversity |
| Long-running stress | Repeated remount, filled queues, log rotation, thermal/USB reset cases |
| Portability | Multiple kernels, Linux distributions, CPUs, macOS/APFS format variants |
| Security | Independent review and fuzzing of malicious filesystem metadata and privilege boundaries |
| Commercial suitability | Independent qualification, support/recovery process and broader compatibility |

There is no sustained USB 3.2 MB/s claim or recovery-time SLA. Small cached-image timings are not hardware throughput. Process SIGKILL tests do not model loss of internal disk power or USB controller caches.

## Durability boundary

Some existing APFS data blocks are modified in place under an **external** undo/redo journal. Correctness depends on the journal remaining available, flush behavior, device identity validation, and exclusive ownership. Never delete a pending owner/queue/journal or switch the volume to a native Mac writer before completing recovery. Diagnostic logs are not the recovery journal.

## Historical beta.2 real-volume blocker

A prepare-only canary on a roughly 1 TB volume exceeded the 32 MiB journal cap. Original APFS blocks were unchanged; RW activation did not proceed. Catalog-wide metadata work can exceed the budget even for a tiny new file. That beta.2 build was unsuitable for the tested volume. The beta.3 correction and remaining physical-device qualification are documented below.

## beta.3 candidate

The original 32 MiB limit is retained. Incremental metadata updates now pass the previously failing real-volume prepare simulation. Physical write qualification remains pending; see [current report](REAL_VOLUME_AFTER.md). Previous evidence above retains its original build identity.

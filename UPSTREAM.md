Source: https://github.com/enesilhaydin/apfs-explorer
Commit: 0ef6cd705ac3cca4efc953adbb99f705d04fe440
Vendored apfs-core, apfs, apfs-write source only. GPL-3.0-only.
The local patch set below modifies the vendored source. Package manifests omit upstream integration test registrations.

Local correctness patch: apfs-core/src/nx.rs reads nx_uuid at hexadecimal 0x48
(72 decimal), instead of erroneously treating the layout annotation as decimal
48. The regression test uses different feature bytes and UUID bytes.
Second local correctness patch: apfs-write/src/file.rs build_extref_node reserves
one 8-byte table-of-contents slot when the extent-reference tree is empty.
Before this patch, Apple fsck reported an invalid btn_table_space.len (0) after
overwriting, renaming and deleting the final data file. The six-step native
macOS regression is in tests/verify_macos.py; before/after logs are retained.
The external recovery layer records in-place writes as well as CoW writes.

Additional local implementation and protections:
- file.rs append_aligned: append bounded chunks to exclusive, block-aligned,
  non-sparse streams without rewriting previous data. Final chunk can be short.
- txn.rs: materialize allocation bitmaps for all-free chunks in the reserved
  internal pool. The original bitmap-less path allocated just one block and
  then skipped the now-partially-free chunk, exhausting usable allocation at
  104 MiB in a mostly empty 6 GiB Apple-created image. The failed prepare left
  the last committed image fsck-clean. Internal-pool layout and allocation
  semantics were checked against linux-apfs-rw/apfs_raw.h and spaceman.c;
  this is an independently written Rust implementation, not copied C code.
  Like other upstream allocator paths, it updates bitmaps in place and REQUIRES
  the external durable undo journal. It is not a standalone crash-safe CoW fix.
- txn.rs: reject unsupported CAB indirection and invalid chunk counts; propagate
  allocator read errors instead of silently trying another region.
- catalog.rs / apfs/lib.rs: conservative write eligibility for unshared plain
  files, preserving inline com.apple.provenance; reject other unvalidated
  xattrs, clones, hard links, compressed/sparse/special files.
- The allocator unit fixture now includes a real internal-pool layout and checks
  persistent bitmap creation plus subsequent multi-block allocations.

- catalog.rs / apfs/lib.rs: a true bounded range reader for plain files. The
  upstream FsView::read(offset,len) reads/decompresses the entire file and only
  then slices it. Large-file testing exposed both memory growth and quadratic
  I/O. LAPFS now uses read_range, which reads only overlapping bytes; compressed
  streaming reads are rejected. A synthetic 8 GiB extent regression asserts a
  37-byte request beyond 4 GiB performs exactly 37 data bytes of device reads.

## LAPFS beta additions (2026-09-28)

- fuser 0.16.0 from crates.io / https://github.com/cberner/fuser/tree/v0.16.0, vendor/fuser. Preserve its LICENSE. Only build.rs changed: select Linux support using CARGO_CFG_TARGET_OS, not the build host cfg.
- Linux read-only device/FUSE and enrolled offline block-device backend are local LAPFS code. Actual USB read-only access and disposable loop block-device read/write acceptance passed. Physical USB write/power-loss acceptance remains pending; image/loop tests do not cover it.

- catalog.rs: object-ID branch selection for all nine catalog query paths. Include both adjacent ranges at equal separator IDs; reject malformed levels/order/depth. Two synthetic tests compare every ID against a full walk and verify corruption behavior. Primary comparator reference: https://github.com/linux-apfs/linux-apfs-rw/blob/master/key.c (apfs_keycmp compares id, then type). Changes are included in the vendored source. Real-device read-only and 104-file native round-trip evidence: docs/VALIDATION.md.

## Buffered RW beta 0.3 (2026-09-28)

- file.rs write_range_plain is a local bounded plain-file range writer derived from the existing append orchestration. It preserves untouched extents, allocates only additional tail blocks, and updates existing data blocks through the external undo/redo overlay. It requires conservative caller eligibility (no snapshots/shared/compressed/encrypted streams); it is not independently crash-safe native CoW. Native Apple fsck/SHA and 26 write/flush fault boundaries are recorded in docs/VALIDATION.md.
- src/buffered.rs and src/mount_rw.rs are local code implementing a durable bounded input queue, exactly-once retirement of committed batches, recovery/replay, and FUSE callbacks. Kernel writeback cache remains disabled; direct I/O ensures userspace pending-state visibility. File attributes not implemented are explicitly rejected; full POSIX support is not claimed.

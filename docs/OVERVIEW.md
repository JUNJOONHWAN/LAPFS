# LAPFS overview

LAPFS is an experimental Linux APFS reader and buffered writable FUSE host for DGX Spark / GB10. Its public name and CLI are LAPFS/lapfs; the internal Rust crate name remains `spark-apfs-safe` for source compatibility, and is not a safety certification.

- Default grouped FUSE writes: volatile RAM input and redo, with necessary undo persisted before APFS changes. fsync/close/normal unmount complete and verify the changes. Per-write durable mode retains the local WAL. Unflushed grouped input may be lost.
- Two bounded 32 MiB logical input batches (not a total memory bound) and one storage worker. fsync/close/metadata changes/unmount drain the queue. Both disk log syncs remain mandatory in the legacy durable journal path; grouped FUSE data writes persist only necessary undo.
- A bounded recovery journal protects supported APFS mutations. New grouped data writes use undo with RAM redo; durable and metadata operations keep disk undo/redo.
- Full-device and full-file staging are not required. Catalog memory and long-term performance remain unqualified.
- Error diagnostics live separately from recovery data and rotate at approximately 8 MiB total.

**Use disposable images or expendable test media.** Real USB write/power-cut qualification, large-volume write throughput, complete APFS/POSIX semantics and commercial certification are not established. Beta.7 writes file data and allocator state through CoW and publishes a flushed descriptor-ring checkpoint while preserving the previous active checkpoint. Native Mac-first interrupted-image checks are documented in [the allocator report](NATIVE_COW_IMPACT.md). Pending input can still reside only in the DGX queue. Preserve external recovery state and use normal safe eject; never replay an old DGX journal over a volume that another host has changed.

The DGX source repository is canonical. Linux ARM64 release assets are built and tested there. macOS is used for independent Apple fsck and file-hash checks; this release does not offer a macOS FUSE driver. See [validation](VALIDATION.md), [limitations](LIMITATIONS.md), [recovery](RECOVERY.md) and [license notices](../THIRD_PARTY_NOTICES.md).

Sequential I/O grouping, ARM SHA-256 acceleration and bounded RAM input are documented in [beta.9 throughput](SEQUENTIAL_IO.md). Local image results are not USB throughput guarantees.

Beta.10 adds a bounded 8MiB committed-read window, invalidated by writes, flushes and recovery. Pending data overlays each reply. See [read performance](READ_AHEAD.md).

See [beta.12 pipeline](WRITE_PIPELINE.md) for implementation, failure checks and local-image benchmarks. Physical USB throughput and comparative performance against linux-apfs-rw remain unverified.

Beta.13 grouped FUSE data writes use RAM redo and durable undo, with no input WAL or disk redo copy. Original checkpoint CIB/bitmap validation permits omitting preimages only for originally free blocks; original allocated bytes retain undo protection. Interrupted volatile groups roll back instead of replaying missing input. New v3 sessions require beta.13 or later for recovery; normal safe eject is required before downgrade. Per-write durable policy retains the existing WAL path. See [write amplification](WRITE_AMPLIFICATION.md) for measurements and limits.

Beta.16 uses catalog path CoW for cross-directory moves and batches up to eight grouped-mode unlinks. Disposable Linux FUSE and macOS APFS fsck checks passed; real Corsair read-only move probe and move/delete canaries passed; Mac roundtrip and TB-scale qualification remain pending. Pending move queues require beta.15 or later for recovery. See [move and delete validation](BETA16_MOVE_DELETE.md).

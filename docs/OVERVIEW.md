# LAPFS overview

LAPFS is an experimental Linux APFS reader and buffered writable FUSE host for DGX Spark / GB10. Its public name and CLI are LAPFS/lapfs; the internal Rust crate name remains `spark-apfs-safe` for source compatibility, and is not a safety certification.

- Beta.9 default: checksummed grouped writes buffered in RAM, then persisted to the local input log at synchronization boundaries. A normal write acknowledgement does not promise power-loss durability; fsync, close and normal unmount persist and apply the group. `--durable-writes` retains per-write local durability. Unflushed grouped input may be lost after sudden removal or host power failure.
- Default 32 MiB grouping; fsync/close/metadata changes/unmount drain the queue.
- A bounded external undo/redo journal protects supported APFS mutations and enables readback verification and recovery.
- Full-device and full-file staging are not required. Catalog memory and long-term performance remain unqualified.
- Error diagnostics live separately from recovery data and rotate at approximately 8 MiB total.

**Use disposable images or expendable test media.** Real USB write/power-cut qualification, large-volume write throughput, complete APFS/POSIX semantics and commercial certification are not established. Beta.7 writes file data and allocator state through CoW and publishes a flushed descriptor-ring checkpoint while preserving the previous active checkpoint. Native Mac-first interrupted-image checks are documented in [the allocator report](NATIVE_COW_IMPACT.md). Pending input can still reside only in the DGX queue. Preserve external recovery state and use normal safe eject; never replay an old DGX journal over a volume that another host has changed.

The DGX source repository is canonical. Linux ARM64 release assets are built and tested there. macOS is used for independent Apple fsck and file-hash checks; this release does not offer a macOS FUSE driver. See [validation](VALIDATION.md), [limitations](LIMITATIONS.md), [recovery](RECOVERY.md) and [license notices](../THIRD_PARTY_NOTICES.md).

Sequential I/O grouping, ARM SHA-256 acceleration and bounded RAM input are documented in [beta.9 throughput](SEQUENTIAL_IO.md). Local image results are not USB throughput guarantees.

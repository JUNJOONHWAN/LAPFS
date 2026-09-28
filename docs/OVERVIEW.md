# LAPFS overview

LAPFS is an experimental Linux APFS reader and buffered writable FUSE host for DGX Spark / GB10. Its public name and CLI are LAPFS/lapfs; the internal Rust crate name remains `spark-apfs-safe` for source compatibility, and is not a safety certification.

- Accepted writes first enter a persistent, checksummed input queue on local ext4/XFS.
- Default 4 MiB grouping; fsync/close/metadata changes/unmount drain the queue.
- A bounded external undo/redo journal protects supported APFS mutations and enables readback verification and recovery.
- Full-device and full-file staging are not required. Catalog memory and long-term performance remain unqualified.
- Error diagnostics live separately from recovery data and rotate at approximately 8 MiB total.

**Use disposable images or expendable test media.** Real USB write/power-cut qualification, large-volume write throughput, complete APFS/POSIX semantics and commercial certification are not established. A crash can leave APFS dependent on an external recovery journal. Do not attach an incomplete volume to macOS before LAPFS recovery.

The DGX source repository is canonical. Linux ARM64 release assets are built and tested there. macOS is used for independent Apple fsck and file-hash checks; this release does not offer a macOS FUSE driver. See [validation](VALIDATION.md), [limitations](LIMITATIONS.md), [recovery](RECOVERY.md) and [license notices](../THIRD_PARTY_NOTICES.md).

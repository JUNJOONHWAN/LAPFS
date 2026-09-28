# LAPFS v0.3.0-beta.2 — public experimental beta

First public LAPFS prerelease, maintained and built from the DGX canonical source.

## Included
- Linux ARM64 APFS read-only access and a buffered writable FUSE host.
- Persistent 4 MiB input queue, bounded external undo/redo, explicit recovery and device enrollment.
- Ordinary file create/copy/range update/append, fsync/close, and the documented subset of metadata operations.
- Separate rotating JSONL diagnostics, operation/errno logging and expected-lookup filtering.
- Visual Korean README, architecture diagram, English overview, specification/limitations/recovery guides, GPL-3.0-only license and third-party notices.
- Complete source with locked, vendored Cargo dependencies and a synthetic APFS test fixture.

## Validation and limits

DGX image FUSE and independent Apple fsck/file hashes pass. Historical kernel-loop, interruption and failure-boundary evidence is labeled by build in docs/VALIDATION.md. The native release has its own build/test/hash receipt. **Real USB writes, unplug/power-cut durability, TB-scale writer throughput, full POSIX/APFS compatibility and commercial qualification remain unverified.** Use disposable images or expendable media; not the only copy of important data.

The Linux archive includes a DGX-native GNU/Linux ARM64 binary, notices, docs, QA helper and synthetic fixture. The source archive contains the matching source and dependencies. SHA256SUMS covers assets. macOS is a validation host, not a shipped writable FUSE driver. The original legacy crate name is retained for compatibility.

### Known real-volume failure

A prepare-only canary on an approximately 1 TB APFS volume exceeded the 32 MiB journal budget, leaving the original unchanged. RW mount activation for that volume failed. This release does not claim that large real volume is writable; catalog mutation scalability is an explicit remaining blocker.

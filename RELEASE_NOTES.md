# LAPFS v0.3.0-beta.3 — public experimental beta

- Fix real-volume journal exhaustion by copying changed catalog/extent-reference tree paths, including splits, deletion and root statistics; retain unchanged catalog mappings and reclaim only superseded pages.
- Resolve directory/file metadata with bounded key-range traversal; avoid quadratic Unicode name lookup and redundant readdir stats.
- Preserve read lookup caches only within the same volume-superblock address/XID generation.
- Flush one coalesced write group per journal; record and retire only its durable queue prefix. Atomic remove+rename batches remain together.
- Add a structurally read-only `probe` command with measured journal bytes and explicit refusal status.
- Keep 4 MiB input, 32 MiB undo+redo and 16,000 write-operation limits.

The approximately 1 TB volume that failed beta.2 preflight now passes tiny-file and 4 MiB prepare simulation within the original cap. This measures staging, not USB write throughput. Actual selected-device 8,388,617-byte canary, fsync, normal unmount, raw APFS SHA, remount/read/delete and unchanged original root names passed. Initial EBUSY on normal unmount was resolved by resuming the same session without forced unmount; see docs/PHYSICAL_CANARY_INCIDENT.md. This is one device and one cycle, not unplug/power-loss or broad commercial qualification. See docs/REAL_VOLUME_AFTER.md for evidence and limits.

---

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

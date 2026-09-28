# LAPFS v0.3.0-beta.6 — Mac/Linux handoff guard

- Add final device sync before a session is marked closed and a fail-closed `handoff-ready` check.
- Package `scripts/safe-eject.py` for normal FUSE unmount, dead-daemon stale mount cleanup, exact-session recovery and a positive disconnect receipt.
- Disposable-image normal exit and SIGKILL recovery passed native Apple fsck and all 104 original hashes. Mac-native write -> DGX write -> Mac fsck/hash roundtrip passed using the same handoff implementation.
- This does **not** certify unplug during an APFS transaction, USB bridge flush reliability, or direct Mac-first recovery without the DGX journal. The selected Corsair live mount was not moved or modified for this test. See `docs/HANDOFF_AFTER.md`.

---

# LAPFS v0.3.0-beta.5 — public experimental beta

- Remove the FUSE mount-lifetime 100,000 inode-path and 4,096 handle counters. FUSE `forget` and handle release now reclaim bookkeeping; allocation failures return ENOMEM rather than a synthetic EIO.
- Add chmod, atime/mtime and symlink create/remove to the supported writable subset. Fix APFS symlink counters, filesystem-owned xattr and NUL-terminated target for macOS readback.
- Make ordinary `rsync -a` work for same-user regular files, directories and symlinks. A disposable image passed initial copy, unchanged repeat, atomic replacement, symlink-target replacement and `--delete` of a file/link/directory; all 104 original fixture files, new hashes, symlink target, mode and nanosecond mtime passed independent macOS readback and warning-free `fsck_apfs -n`.
- Keep existing durability caps and unsupported APFS layouts. This release does not qualify device nodes, differing uid/gid, xattrs/ACLs, unplug/power loss or broad hardware compatibility.

The selected approximately 1 TB Corsair volume was normally transitioned to beta.5, and a separate five-stage `rsync -a` canary passed with the original root names unchanged; see `docs/VALIDATION.md`. The same-condition full five-root name traversal counted 17,368 directories, 397,593 files and 388 links with 0 errors; all 795 prior EIO paths passed individual `lstat`. The stricter every-entry `lstat` run was partial and is labeled as such in the validation ledger. No unplug or power-loss qualification is claimed.

---

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

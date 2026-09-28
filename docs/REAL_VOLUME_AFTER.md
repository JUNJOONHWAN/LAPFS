# Real-volume correction: after-impact report (qualification in progress)

Canonical source/runtime: DGX. Branch: `fix/real-volume-journal-budget`. Candidate: 0.3.0-beta.3. No public beta.3 release has been published.

## Changes and direct impact

- Shared prepare uses a generic overlay; read-only probe holds an O_RDONLY Reader, never publishes an applicable journal, and deletes only its own scratch.
- Catalog and extent-reference writes copy changed paths and update separators, splits, root counters and object-map allocation deltas. Unchanged catalog pages are retained; only explicitly superseded pages are reclaimed.
- Supported file mutation reads use bounded catalog ranges.
- Durable queue retirement supports a committed prefix; interleaved writes no longer multiply one journal without a bound. Metadata replacement remains one batch. Old session records remain readable.
- Name indexing, generation-bound lookup caches, typed (object ID, record type) queries and direct directory-record enumeration address large-directory lookup costs.
- CLI, tests, version and documentation updated. No schedulers, other applications, research inputs or historical recovery journals changed.

## Measured physical-volume simulation

The source is an approximately 1 TB APFS partition. All probes open it read-only; physical target writes = 0. Previous read-only mount was restored after each probe.

| Case | Journal bytes (undo + redo) | Staging time | Outcome |
|---|---:|---:|---|
| Original tiny-file writer | 33,554,432 then refused | — | 32 MiB cap |
| Original writer with 128 MiB diagnostic cap | 131,072,000 then refused | 14.70 s | 16,000 operations |
| Incremental tiny-file writer | 8,839,168 | 0.251 s | Passed, 1,081 operations |
| Incremental 4 MiB file | 17,219,584 | 0.280 s | Passed, 2,104 operations |

These timings measure staging through a read-only overlay, not actual USB write speed. The production cap has not been increased.

## Verification completed so far

- Workspace library tests: 269 passed (21 apfs, 138 core, 108 writer, 2 diagnostics).
- Durable queue roundtrip/guards plus original eight and new seven prefix-retirement crash points passed.
- All 15 crash-result images independently passed Apple fsck and recovered-file SHA checks.
- Final-source APFS torn-write/flush sweeps: 28 create boundaries + 27 range boundaries, full-image SHA restored at every boundary (160.09 s). [Receipt](validation/beta3-release3-apfs-recovery.log).
- Portable DGX FUSE workload, ordinary cp, 12,550,013-byte copy, random writes, rename/replace/delete, Unicode, accepted-write SIGKILL recovery, separate diagnostics: passed; independent Apple fsck, 104 original files and all changed files passed.
- Probe success and cap refusal preserve the complete image SHA, release source ownership and remove temporary scratch.
- Large-volume synthetic workload: all 5,000 originals, 600 creates, 200 renames, 300 deletes and 40 interleaved overlapping writes passed in 121.88 seconds with the final binary. Apple fsck is clean; all 5,000 originals and 300 surviving new files match SHA, and removed names are absent. Earlier matching workload took 1,286.45 seconds before lookup fixes; these are local image observations with different concurrent host load, not a controlled USB benchmark.

## Remaining gates

Root kernel-loop acceptance, actual selected-device canary with fsync/unmount/remount/readback/deletion, and publication. The remote session cannot obtain root (`sudo -n` requires authentication), so the prebuilt, SHA-pinned transition script must run once in the operator terminal. No physical USB write success, unplug/power-cut qualification, or commercial certification is claimed.

## Exact candidate and receipts

Final Linux ARM64 binary SHA256: `27382b42285a850c9df95b51a66501dbe3cd2785f8e5aa6854d43ab09ee29e3c`. Cargo package and CLI both report 0.3.0-beta.3. The binary is staged on DGX but the selected device remains read-only.

- [Final FUSE + Apple validation](validation/beta3-fuse-native.json): 12,550,013-byte copy, 21 committed batches, 104 originals and all changed files; 0.958 s copy observation on a cached synthetic image.
- [Large catalog + Apple validation](validation/beta3-catalog-native.json): complete 5,000-file verification.
- [8 queue crash cases](validation/beta3-crash-native.json) and [7 prefix-retirement cases](validation/beta3-prefix-native.json): recovered content independently verified by Apple. These use the test profile with fault injection, not the release executable.
- [Final binary actual-volume read-only probe](validation/beta3-physical-readonly-probe.json): 32 MiB cap unchanged, 24 root entries readable after RO restoration, zero physical target writes.
- Ordinary Rust regression: 19 passed. Python partition-selection regression: 6 passed. Typed catalog queries were compared with full traversal for every queried ID/type in their synthetic boundary fixture.
- Source changes are confined to LAPFS writer/reader/queue/FUSE, diagnostics for staging, their direct tests and release documentation. Existing failed recovery records, research files, unrelated services and schedules were not changed.
- Native Apple validators now avoid Finder browsing and retry normal detach when busy; no forced detach.

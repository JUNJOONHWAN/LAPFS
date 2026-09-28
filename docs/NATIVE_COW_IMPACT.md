# Native allocator CoW: impact and validation

## Change-before assessment

Authority: DGX canonical source, isolated branch `fix/mac-first-cow`. Baseline main is beta.6 `b430daf`; prior file-data CoW prototype is `ab5b134`. Production Corsair mount is excluded from fault tests.

The preceding audit found that file-data CoW alone preserves file bytes but leaves shared chunk bitmaps/CIBs inconsistent when a write stops before checkpoint publication. Patching old spaceman counts also modifies the checkpoint that must remain readable. A local undo journal cannot protect Mac-first mounting when that journal remains on DGX.

Affected callers: all transaction commits, including prepared/apply batches and buffered FUSE/rsync writes. Changes are confined to `vendor/apfs-write/src/txn.rs`, new `space_cow.rs`, prior file CoW code, and disposable-image validation. No scheduler, research data, live mount, credentials, or service changes.

Intended change: copy chunk bitmap and CIB into free internal-pool addresses; rotate the internal-pool bitmap; write fresh ephemeral objects/checkpoint map; flush dependencies; publish a descriptor-ring NX; flush. Keep bootstrap and the previously active allocator graph unchanged. Reclaim queued pool blocks only after rejecting aliases to the active allocator. Reject invalid ring geometry or overlap with active checkpoint. Bounds/cycle checks prevent malformed allocator state from being interpreted as reusable space.

## Acceptance gates

- Exact prepared I/O trace replay must equal actual apply output.
- Every write/flush prefix and every tested torn NX sector prefix must mount on native macOS without DGX journal recovery; native `fsck_apfs -n` must be clean; original 104 file hashes must match and changed files must show an entire before or after state.
- Linux FUSE and rsync regression outputs must independently pass native macOS fsck, hashes and metadata checks.
- Repeat transactions through ring reuse; native Mac writes followed by Linux writes and native recheck.
- Rust workspace/recovery regression and a production build without fault injection.

These are software image gates. They do not certify USB bridge flush honesty, arbitrary physical torn sectors, controller firmware faults, or real power cuts. No assertion of commercial Mac hardware parity follows from passing image tests.

## Change-after evidence

- Initial allocator CoW: 21/21 actual SIGKILL I/O boundaries pass native fsck, original104 hashes, old/new modified-file content.
- Linux FUSE: 27 committed batches pass. Native Mac fsck and original104 plus changed hashes pass.
- rsync: initial/repeat/replace/link-change/delete all pass. Native Mac fsck, hashes, symlink, mode0640, nanosecondmtime and absence of deleted/temp files pass.
- Multi-operation all-prefix/torn checkpoint matrix and hardened-source reruns completed; final evidence below.
- Production mount remained unchanged during these gates. Publication follows successful checks.

## Additional completed gates

- Final Rust workspace/fixture suite: 296 passed, 0 failed. Command used `--include-ignored --skip buffered_crash_worker --skip buffered_prefix_crash_worker`: the two named entries are child-process entrypoints invoked with environment by the parent crash tests, not standalone tests. An initial indiscriminate include-ignored run invoked them without their required environment and failed; the final correctly orchestrated suite is retained separately. No production test was disabled.
- Fully exercised ARM64 binary SHA-256: `91531db2c4eb1161142ce443ee0e64794128b1b75f959c5f73cdcc005da446bf`. Built offline with Rust 1.92.0, release profile, no fault-injection feature.
- That exact binary passed 27 FUSE committed batches and all five rsync archive phases. Native macOS fsck, original104 and changed hashes, mode0640, nanosecondmtime, symlink and deletion checks passed on the resulting images.
- A 272,629,778-byte import into a fresh 512 MiB APFS image passed native fsck and full SHA, exercising multiple chunk allocation bitmaps and repeated checkpoints. This is cached-image behavior, not a USB throughput benchmark.
- Normal and killed-daemon safe-eject recovery images passed native fsck and original104/handoff hashes (prior formatting-equivalent release build); the direct Mac-first matrix does not use this recovery path.
- Three Mac-native-write → Linux-write → Mac-read/fsck cycles passed on the hardened prototype. An initial host-clock-ahead warning was retained; a bounded 5-second recheck was clean. The portable roundtrip harness also passed all three cycles on the fully exercised release binary.
- Two earlier 430-image matrices passed (318 completed write/flush prefixes plus112 partial NX writes at512-byte boundaries across16 operations). Final shipping-binary matrix receipt is the release gate below.

## Source references and implementation boundary

Apple's [APFS format reference](https://developer.apple.com/support/apple-file-system/Apple-File-System-Reference.pdf) defines the object/checkpoint/spaceman layouts. The [Linux APFS RW repository](https://github.com/linux-apfs/linux-apfs-rw) was consulted for transaction-lifecycle comparison. New Rust allocator code was independently implemented; no GPL-2.0-only kernel source was copied into this GPL-3.0-only project.

## Remaining qualification

Actual cable-pull/power-cut tests, USB controller flush honesty, arbitrary sector tearing/reordering, 1 TB allocator/layout diversity, encrypted/snapshot/shared-file mutations, cross-directory/directory renames and full POSIX compatibility remain unqualified or unsupported. The matrix tests whole completed I/O writes plus partial NX sectors; it does not model every possible physical sector corruption. Mode/mtime roundtrip is checked by the separate rsync gate, not by the matrix's before/after content matcher.

The live Corsair stays on beta.5 until a normal exact-session handoff is executed with sudo. Root authorization is not available in the remote session. Release publication and live device deployment are distinct outcomes.

## Final release gate

The final release implementation passed all **430/430** Mac-first cases: **318 completed I/O prefixes + 112 torn checkpoint writes**, with no DGX recovery. Original104 file hashes matched for every case and QA objects matched a complete before or after state. The image replay was required to equal actual apply output for each of the16 sequential operations.

The published binary is `dccc7cead3774b9b1d247c3ac2e1334fdfa114e44e70dab087f158da52fb3b32`. After testing, the help banner's beta.6 digit was corrected to beta.7. ELF comparison proves `.text` is byte-identical (SHA `d6f7b5e2fa9900bd7301c6d6237892f238c7dd60b6d2fafa2617df297529a0eb`); only the GNU build ID and one `.rodata` byte changed. Both help and version commands now report beta.7. The exact tested and published binary identities are deliberately retained in the receipt.

[Summary receipt](validation/native-cow-beta7.json) · [All430 case receipts and image SHA values](validation/native-cow-beta7-matrix.json).

Changed source: `file.rs` (from the preceding prototype), `txn.rs`, new `space_cow.rs`, and the CLI help version. Tests: exact-I/O matrix, portable Mac/Linux roundtrip and ring-overlap unit coverage. Packaging/version and README/limitations/overview/release documentation updated. No research files, external mail, scheduler, existing Corsair files or live mount were changed.

Reproduction: generate with `python3 tests/verify_native_cow_matrix.py generate --output NEW_DIR --binary target/release/lapfs --fixture fixtures/block-test.dmg.gz` on Linux; transfer that directory and run `python3 tests/verify_native_cow_matrix.py check --output NEW_DIR --expected fixtures/expected.json` on macOS. The portable roundtrip script exposes all host/path operands through `--help`.

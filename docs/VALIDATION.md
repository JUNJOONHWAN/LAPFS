# Validation ledger

## beta.5 compatibility candidate, 2026-09-28

The canonical DGX ARM64 source added FUSE inode lookup/forget eviction, metadata updates and symlink writes. The reproducible `tests/verify_rsync_archive.py` disposable image `rsync -a` matrix passed initial copy, identical repeat, changed-file atomic replacement, symlink-target replacement, and `--delete` of an orphan file, link and nonempty directory. The matrix verified nested files, permissions `0640`, nanosecond mtime, the full symlink target and SHA values. On macOS, `fsck_apfs -n` reported no warnings and all 104 original fixture files plus the new files and link passed independent readback. [Matrix](validation/beta5-rsync-matrix.json) · [Apple fsck](validation/beta5-apple-fsck.log).

The previous 100,000 FUSE path and 4,096 handle caps are removed. A unit test records 125,001 inode entries and reference eviction. The real Corsair beta.5 remount and full read traversal have **not yet passed**; beta.4 remains the current physical mount until an authenticated normal transition is executed. The existing research copy must finish before any transition. No beta.5 physical-write or hardware power-loss result is claimed here.


All listed APFS fixtures are disposable synthetic images. Redacted evidence preserves check names, hashes and counts; private machine paths were replaced with placeholders. No customer/user file contents or device identifiers are included. A small-image timing is not USB throughput.

| Build / layer | Verified scope | Evidence |
|---|---|---|
| beta.2 musl ARM64 candidate | Actual DGX RW FUSE; 12,550,013-byte copy; range updates; two handles; fsync/close; ordinary cp; Unicode paths; truncate/append; file replace/delete; expected unsupported errors | [result](validation/buffered-fuse-beta2.json) |
| Same candidate | FUSE SIGKILL after acknowledged write, before fsync; 502,007-byte accepted data recovered; pending-owner read refused | Same result |
| Same candidate / macOS | Apple fsck clean; all 104 originals and all modified/new files SHA-match | [Apple log](validation/buffered-fuse-beta2-apple-fsck.log) |
| Same candidate / diagnostics | write EOPNOTSUPP, setattr EOPNOTSUPP, unlink EBUSY recorded; normal lookup ENOENT excluded | [events](validation/buffered-fuse-beta2-errors.json) |
| beta.2 logger units | Concurrent records, rotation caps, JSON validity, symlink rejection and external file preservation | [units](validation/logging-unit-tests.log) |
| beta.1 kernel loop | Eight checks including enrolled raw block backend, RW FUSE as invoking user, post-unmount readback, native Apple fsck + 104 original and two added file hashes | [result](validation/kernel-block-beta1.json), [Apple log](validation/kernel-block-beta1-apple-fsck.log) |
| Earlier unchanged buffered core | Eight crash points: queue publish, preparation creation, Applying, apply-write, Committed, queue retirement, partial GC, closed-state recovery | [all eight](validation/crash-eight-points.json) |
| Earlier unchanged journal/range writer | 25 original mutation and 26 range-write failure boundaries restore whole-image hashes | [boundaries](validation/apfs-boundaries.log) |
| Earlier unchanged core | 17 recovery/reader/format regressions; seven range-eligibility cases | [regressions](validation/legacy-regression.log), [range](validation/range-preflight.log) |

The GitHub release uses a new DGX-native GNU/Linux build from the canonical public source. It is not byte-identical to the prior musl candidate. Native release SHA256: `1dfcdb04dc38ae4ede2808e91a1aaf6c73d3bb2ee3c244179599d89b06823b4c`. DGX Rust 1.92.0 offline release build passed, 19 ordinary Rust tests passed (six explicitly image-dependent tests skipped), six Python partition-parser tests passed, and the complete portable Linux FUSE workload passed. Independent Mac verification of that image: Apple fsck clean and all 104 original plus all changed files hash-match. [Native result](validation/dgx-native-beta2.json), [Apple log](validation/dgx-native-beta2-apple-fsck.log). This binary requires glibc >=2.39 and libgcc_s; it dynamically links those libraries and is distinct from the historical musl candidate. Do not transfer physical acceptance between binaries without labeling the scope.

## Reproduce

```bash
cargo test --locked --offline --lib --tests
python3 -m unittest discover -s tests -p 'test_*.py'
python3 tests/verify_linux_fuse.py --binary target/release/lapfs
python3 tests/verify_rsync_archive.py --binary target/release/lapfs
```

The first command explicitly skips image-dependent `#[ignore]` tests. To run the two APFS write-fault tests with the synthetic fixture:

```bash
mkdir -p evidence/fixture-tests
gzip -dc fixtures/block-test.dmg.gz > evidence/fixture-tests/base.dmg
export SPARK_APFS_TEST_IMAGE="$PWD/evidence/fixture-tests/base.dmg"
export SPARK_APFS_TEST_OFFSET=20480
export SPARK_APFS_TEST_OUTPUT="$PWD/evidence/fixture-tests"
cargo test --locked --offline --test apfs_recovery -- --ignored --test-threads=1
```

Buffered roundtrip/guards use the same environment, `cargo test --test buffered <test-name> -- --ignored`. Crash-boundary tests additionally require `--features fault-injection`; never ship that build. The crash worker is a helper launched by its parent test, not an independent test to run arbitrarily.

Linux FUSE output is retained for native Mac validation. `tests/verify_linux_apple.py OUTPUT_DIRECTORY` runs Apple fsck and compares original plus changed hashes after transfer. Keep the image and result.json together. Mac-only scripts create images under evidence/; see each script header for prerequisites. Root loop QA is `sudo python3 scripts/verify-block-device.py` from the packaged distribution, which provides bin/lapfs and fixtures/. This does not write a physical USB device.

## Not established

USB unplug/power loss; TB-scale real-volume writer throughput; full APFS/POSIX support; long-term hardware endurance; independent security/commercial certification. See [limitations and qualification plan](LIMITATIONS.md).

## Historical beta.2 real-volume preflight: failed, original unchanged

After the image tests, a real approximately 1 TB APFS volume was positively identified and an offline **prepare-only** canary transaction was attempted. It exceeded the 32 MiB journal cap before apply. No physical APFS writes were issued. Writable mounting of that volume was not achieved. The underlying writer has catalog-wide metadata work; a tiny payload does not imply a tiny transaction. Increasing a limit alone has not been qualified as a fix. This is a known beta blocker, not an accepted physical-device test.

## beta.3 candidate

The original 32 MiB limit is retained. Incremental metadata updates now pass the previously failing real-volume prepare simulation. A selected physical-volume canary later passed; see [current report](REAL_VOLUME_AFTER.md). Previous evidence above retains its original build identity.

## beta.3 final candidate, 2026-09-28

See [before-impact](REAL_VOLUME_BEFORE.md) and [after-impact](REAL_VOLUME_AFTER.md) for source changes, exact candidate SHA, all current receipts and completed selected-device canary acceptance. The 5,000-original fixture is reproducible on macOS with `python3 tests/make_catalog_fixture_macos.py NEW_OUTPUT_DIRECTORY`. Transfer its image and expected.json to Linux, then run `tests/verify_catalog_cow.py --image large.dmg --expected expected.json --output evidence`. Transfer the resulting large.dmg/result.json back to macOS and run `python3 tests/verify_catalog_apple.py OUTPUT_DIRECTORY EXPECTED_JSON`. This covers all files, not a sample.

## beta.3 selected physical volume acceptance

[Redacted canary receipt](validation/beta3-physical-rw-canary.json): 8,388,617 bytes written/fsynced on the selected USB APFS partition; normal unmount, raw APFS SHA, fresh RW mount/read/delete, and original 24 root entry names verified. [EBUSY transition incident](PHYSICAL_CANARY_INCIDENT.md) records the first interrupted unmount and exact-session continuation. The live physical volume has not been checked by Apple's native fsck and unplug/power-loss have not been simulated.

The exact user-run root loop image also passed [independent Apple fsck and SHA](validation/beta3-kernel-loop-apple.json); it is a synthetic loop image, not the physical USB volume.

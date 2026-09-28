# 0.3.0-beta.15 candidate

- Same-volume file/directory move and rename through FUSE; replaces closed destination files or empty directories.
- Directory inode path cache updates include known descendants and open child handles.
- One queued journal batch protects destination removal plus move; three forced-termination recovery images passed Apple `fsck_apfs -n` and content checks.
- Disposable FUSE workload and macOS fsck/SHA passed. Actual Corsair beta.15 activation and TB-scale catalog move qualification are **pending**. Keep beta.14 mounted until normal safe handoff.
- Pending beta.15 `Move` journal records need beta.15 or later for recovery; normally unmount before downgrade.
- [Change and validation report](docs/CROSS_DIRECTORY_MOVE.md).

# 0.3.0-beta.14

- APFS Fletcher-64: 1024개 word 단위 나머지 연산 · 결과·디스크 형식 동일.
- 체크섬·undo·flush·기록 후 재검증 유지.
- `LAPFS_PROFILE`: `prepare_memory_total` 계측 추가.
- 실제 Corsair 256MiB 순차 쓰기 3회: 112–118MiB/s · 기록량 1.29–1.30배 · SHA 일치. beta.13의 1회 87.61MiB/s와 다른 시점 측정.
- 16MiB/10KiB 27.28MiB/s · 기록량 2.94배. 작은 쓰기 병목 잔존.
- Rust 312개 · Mac APFS 59개 이미지 · Corsair 정상 분리/쓰기 검증. 300–400MB/s와 장기 지속 속도 미달·미검증.
- [전후 영향·검증](docs/CHECKSUM_PERFORMANCE.md).

# 0.3.0-beta.13

- 기본 FUSE 데이터 쓰기: 입력 WAL·디스크 redo 제거 · RAM redo + 영구 undo.
- 원본 CIB·비트맵 기준 빈 블록 undo 생략 · 원래 사용 중인 블록 복구 유지.
- 큰 순차 전송 기록량 약 4.1배 → 1.06배 · 공간 재사용 8회 확인.
- 내부 이미지 1MiB write: beta.12 40.90–58.30 → beta.13 101.50–177.22MB/s · fsync/close 포함 · USB 수치 아님.
- Rust 311 · FUSE/rsync/handoff · Mac 60개 이미지 fsck/SHA 통과.
- 신규 세션 v3 · 미완료 v3 복구는 beta.13 이상 필요 · downgrade 전 정상 분리.
- durable-writes·독립 배치 CLI는 기존 영구 입력/undo/redo 유지.
- 실장치 적용·USB 성능·물리 전원 차단·상용 인증 미완료.
- [변경 전후·측정·제한](docs/WRITE_AMPLIFICATION.md).

# 0.3.0-beta.12

- undo/redo 동기화 병렬 실행 · 양쪽 완료 전 APFS 변경 금지.
- 기본 32MiB 두 묶음 입력 파이프라인 · 단일 저장 worker · fsync/close 오류 전파.
- 조회로 인한 불필요한 배치 분할 방지 · pending read 합성·체크섬 검사.
- Rust 308 · Linux FUSE/rsync/handoff · Mac 46개 이미지 fsck/SHA 통과.
- 내부 이미지 1MiB write: beta.11 29.60–31.28 → beta.12 41.88–48.28MB/s. 작은 write 개선 미확인.
- 실제 Corsair 성능·linux-apfs-rw 비교·물리 전원 차단 검증 미완료.
- beta.11: 중간 개발 후보 · 별도 공개 배포 없음.
- [변경 전후·성능·제한](docs/WRITE_PIPELINE.md).

# 0.3.0-beta.11

- 파생 merged 파일 제거 · 검증된 WAL 데이터 직접 전달.
- 준비 단계 순차 원본 읽기 최대 1MiB 집계 · 임의 읽기 4KiB 유지.
- 40MiB 입력당 중간 쓰기 약 40MiB 감소 · 1MiB write 읽기 호출 22,008 → 11,618.
- Rust 302 · Linux FUSE/rsync · Mac 32개 이미지 fsck/원본·변경 SHA 통과.
- 동시 백업 부하 비교: 큰 파일 속도 개선 미확인. beta.11 실장치 적용·성능 검증 대기.
- [변경 전후·측정·제한](docs/WRITE_IO.md).

# 0.3.0-beta.10

- 8MiB 읽기 캐시·선행 읽기 · write/flush/복구 시 무효화.
- Rust 301 · FUSE/rsync/정상 분리 · Mac 최종 이미지 2개 통과.
- 로컬 40MiB APFS 이미지 1MiB 읽기: beta.9 464–530 → beta.10 818–912MiB/s. USB 수치 아님.
- APFS 쓰기 형식·CoW·복구 저널 변경 없음.
- [성능·변경 전후·제한](docs/READ_AHEAD.md).

# 0.3.0-beta.9

- RAM 입력 묶음 32MiB · 1MiB 연속 journal I/O · ARM SHA-256 가속.
- 페이지별 SHA·로그 영구 저장·APFS CoW·체크포인트 flush 순서 유지.
- 40MiB 로컬 이미지: 10KiB write 11.54–18.43 → 54.06–54.16MiB/s. USB 실측 아님.
- Rust 300 · Mac 복구/복사 27 · 중단 이미지 430 통과 · rsync/이전 세션 호환.
- 정상 분리: grouped/durable 옵션 포함 프로세스 식별 수정 · EBUSY/다른 세션 거부.
- 미동기화 RAM 입력 유실 가능. 실장치 전원 차단·장기 지속 속도·2,500MB/s 미검증.
- [변경 전후 영향·측정 전체·제한](docs/SEQUENTIAL_IO.md).

# 0.3.0-beta.8

- 기본 묶음 쓰기; fsync/close/정상 분리 시 영구 저장. 갑작스러운 분리 시 마지막 미저장 입력 유실 가능.
- `--durable-writes`: 매 write의 로컬 영구 저장 옵션.
- 연속 checksum 로그·작업 수 경계 개선·쓰기 속성 캐시.
- 10KiB 쓰기 시험: beta.7 3.72–3.82 → beta.8 20.24–20.60MiB/s. 로컬 APFS 이미지 기준; USB 수치 아님.
- Rust 298 통과; Mac 27개 이미지 fsck/원본104·변경 파일 SHA 통과.
- v1 복구 호환; v2 세션은 beta.8 이상 필요. 이전 버전으로 내리기 전 beta.8 정상 분리 필수.
- [변경 영향·시험·제한](docs/BUFFERED_THROUGHPUT.md)

# LAPFS v0.3.0-beta.7 — native data and allocator CoW

- Copy touched file data blocks and update extent references rather than overwriting previous-checkpoint data.
- Copy chunk bitmaps and CIBs, rotate internal-pool bitmaps, and preserve the active checkpoint allocator until a new flushed ring checkpoint is published.
- Stop rewriting bootstrap block zero and stop patching previous-checkpoint spaceman free counts.
- Validate internal-pool queue bounds, live allocator aliases, bitmap freelists and checkpoint-ring overlap before publication.
- Add reproducible multi-operation I/O-prefix and torn-NX replay with independent macOS fsck and original-file SHA checks. DGX external recovery is not run on those Mac-first images.
- Retain bounded durable queue, external undo/redo, error logs and safe-eject helper. Unapplied queue input still lives on DGX.

See `docs/NATIVE_COW_IMPACT.md` for final counts, source/build identities and the Mac-native-write/Linux-write roundtrip evidence. This is an experimental prerelease, not commercial or real-power-cut certification. Existing unsupported APFS/POSIX features remain unsupported. The live Corsair mount is a separate deployment gate.

---

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

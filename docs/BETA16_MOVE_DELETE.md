# beta.16 · 이동·삭제 성능

## 변경 전 영향 보고서

| 항목 | 확인 |
|---|---|
| 대상 | DGX LAPFS `fix/cross-directory-move` · APFS 이동/삭제 작성기 · FUSE 세션 |
| 실장치 기준 | Corsair beta.14 롤백 세션 `mount-20260929T012030Z-beta14-rollback-876e06` |
| beta.15 실장치 검사 | 읽기 전용 이동 probe 실패 · `Batch operation limit reached` · 메타데이터 페이지 15,926개 시도 · 원본 쓰기 0 |
| 이동 원인 | `move_entry`가 파일 1개 이동에도 전체 FSTREE를 재구성 |
| 삭제 원인 | `unlink`마다 즉시 flush · 카탈로그/omap/저널 반영 반복 |
| 직접 영향 | FUSE 같은 볼륨 이동, 묶음 쓰기 모드의 연속 파일 삭제, 정상 분리·복구 |
| 영향 제외 | 연구 원본, 백업, GPU 작업, 다른 장치와 스케줄러 |

## 변경 후 영향 보고서 — 후보

| 항목 | 결과 |
|---|---|
| 이동 | 스냅샷 없는 파일·폴더 이동/이름 변경: 기존 `CatalogCow` 경로로 변경된 카탈로그 노드만 복사 · 다단 omap 재구성 지원 |
| 삭제 | 기본 grouped FUSE: 최대 8개 연속 삭제를 하나의 APFS CoW 트랜잭션으로 반영 · 각 삭제 의도는 세션 큐에 저장 |
| 가시성 | 반영 대기 중 삭제 파일은 `getattr`·`read`·`readdir`에서 숨김 · 재생성/빈 폴더 제거 전 flush |
| 내구성 | 배치 준비 전 큐 저장 · 적용 중 강제 종료는 기존 저널 복구 · 안전 분리에서 큐/장치 flush |
| durable 모드 | 삭제별 즉시 반영 유지 |
| 코드 | `vendor/apfs-write/src/file.rs`, `src/apfs_batch.rs`, `src/buffered.rs`, `tests/buffered.rs` |
| 실장치 | 아직 beta.14 · beta.16 읽기 전용 probe와 실장치 canary 대기 |

## 시험 이미지

| 검사 | 결과 |
|---|---|
| 3,000개 파일 APFS 이미지 이동 probe | 23개 작업 · undo/redo 합계 172,032바이트 · 0.029초 · 원본 쓰기 0 |
| 같은 폴더의 비어 있지 않은 디렉터리 이름 변경 | 21개 작업 · 155,648바이트 · 적용 뒤 자식 파일 SHA 일치 · Mac fsck 통과 |
| 같은 이미지에 실제 이동 적용 | `COMMITTED` · 목적 파일 SHA 일치 · 원본 이미지 SHA 불변 · Mac `fsck_apfs -n` 통과 |
| 100개 빈 파일 삭제, 최적화 빌드 비교 | 같은 이미지·DGX·최적화 빌드 각 2회: beta.15 1.646–2.008초 → beta.16 0.331–0.420초 · 대응 회차별 3.92–6.07배 |
| 삭제 후 확인 | 2,900개 남음 · Mac `fsck_apfs -n` 통과 |
| 데이터 파일 삭제 | 1MiB × 8개 삭제 · 원본 파일 남음 · Mac `fsck_apfs -n` 통과 |
| 재귀 폴더 삭제 | 파일 48개 + 폴더 4개 · 0.202초 · 기존 파일 보존 · Mac fsck 통과 |
| 강제 종료 복구 | `state-Applying`, `apply-write`, `state-Committed` 세 경계 · 삭제 3개 복구 후 모두 없음 · Mac fsck 3개 통과 |
| FUSE 회귀 | 이동·교체·기존 104개 SHA·삭제·읽기/쓰기/복구 포함 27개 확인 · Mac fsck 통과 |

## 제한

- 수치는 시험 이미지 100개 삭제를 두 회차 비교한 값이다. TB급 Corsair 삭제 속도와 장기 반복 쓰기는 아직 측정하지 않았다.
- 스냅샷·암호화·공유 파일·열린 파일·비어 있지 않은 폴더 삭제는 지원 범위 밖이다.
- 그룹 모드에서 삭제 응답 뒤 장치 반영 전이면 세션 큐가 권위다. 비정상 종료 시 `mount-recover`가 필요하며, 복구 기록을 삭제하면 안 된다.
- 실장치 읽기 전용 probe, 실제 canary 이동·삭제, 정상 분리 및 재마운트 영수증 전에는 beta.16 실장치 완료 또는 공개 릴리스로 판정하지 않는다.

- 최종 beta.16 바이너리 SHA-256: `be24f39b67416d083c11cf1f0c6d2a71600576e7ab6a5d09f656785bbd9ab9d2`.

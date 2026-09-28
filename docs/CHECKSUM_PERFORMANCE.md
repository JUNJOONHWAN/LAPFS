# beta.14 · 체크섬 계산 최적화

## 변경 전 영향 보고서

| 항목 | 범위 |
|---|---|
| 기준 | DGX canonical `c9987e2` · beta.13 |
| 작업 경로 | DGX `worktrees/write-amplification` |
| 실제 마운트 | Corsair · beta.13 · PID 3532966 · 유지 |
| 호출 경로 | FUSE → Pipeline → Session → APFS 파서·writer → Fletcher-64 |
| 수정 대상 | `vendor/apfs-core/src/checksum.rs` · `src/apfs_batch.rs` 계측 |
| 목표 | 동일 APFS 체크섬 결과 · CPU 계산 비용 감소 |
| 쓰기 영향 | 시험 이미지에 한정 · 실제 Corsair 교체 전 적용 없음 |

## 변경 후 영향 보고서

- Fletcher-64: word마다 수행하던 나머지 연산 → 최대 1024 word마다 수행.
- 계산 상한: 입력 전부 `0xff`에서도 중간 누적값 `< 2^52` · u64 넘침 없음.
- 원래 체크섬 계산과 동일한 모듈러 합 · 끝의 1–3바이트 처리 유지.
- 저장 형식·undo·SHA 검증·flush 순서·기록 후 재검증 변경 없음.
- `LAPFS_PROFILE`: `prepare_memory_total` 계측 추가 · 기본 비활성.
- 추가 파일: 버전 표기·본 보고서·검증 JSON·릴리스 노트.
- GPU·연구 프로세스·스케줄러 변경 없음.

## 제외된 후보

- 트랜잭션 원본 페이지 캐시 4MiB: 반복 읽기 감소, 처리량 향상 입증 실패 → 소스에서 제거.
- 초기 처리량 비교: 지침 파일 검색이 Corsair 하위까지 실행된 부하 혼입 → 예비 자료로만 보존.
- 자체 검색 프로세스 PID 3559477 종료 후 최종 비교 재실행. 다른 작업의 파일 검색은 유지.

## 체크섬 계산

DGX ARM64 · 동일 4088바이트 · 100,000회 · old/new/new/old · release 최적화 · 입력·출력 black_box.

| 버전 | 1회 | 2회 |
|---|---:|---:|
| 이전 | 0.2300초 | 0.2239초 |
| 수정 | 0.0378초 | 0.0422초 |

평균 계산 시간 약 82% 감소 · 약 5.7배 처리량. 파일 전송 배율과 별개.

## 검증

| 항목 | 결과 |
|---|---|
| Rust workspace | 312 통과 |
| 계산 대조 | 0·0xff·고정 난수 · 0–8200바이트 전수 및 16/64KiB 경계 · 24,621개 비교 |
| 기존 복구 | 오류 주입·프로세스 중단·전체 원본 바이트·256 난수 workload 통과 |
| FUSE | 25항목 통과 |
| rsync | 5단계 통과 |
| O_SYNC/O_DSYNC | 실제 daemon SIGKILL 이후 Mac 우선 검사 통과 |
| Mac 기능 이미지 | 55개 fsck clean · 원본 104개/변경 파일 SHA 일치 |
| 최종 바이너리 성능 이미지 | 4개 fsck clean · 파일별 SHA 일치 · 합계 59 이미지 |
| 세션 형식 | v3 유지 · 기존 beta.13 복구 규약 유지 |
| 물리 정전·장치 캐시 소실 | 이번 검증 범위 밖 |

기능 시험은 최종 체크섬 코드로 수행. 이후 변경은 beta.14 버전 문자열·문서뿐. 최종 패키지 바이너리는 별도 성능 이미지 검사로 확인.

## 시험 실행 이력

- 최초 원격 편집 명령의 셸 인용 오류: 실행 전 실패 · 별도 Python 파일로 재실행.
- 체크섬 단독 benchmark의 rustc edition 누락: 컴파일 실패 · 프로젝트와 같은 edition 2021로 수정 후 실행.
- 최초 광범위 AGENTS 파일 탐색의 장시간 실행: 종료 · 해당 구간 처리량은 최종 성능 근거에서 제외.
- 완료된 자체 시험 이미지만 압축 해제 SHA 대조 후 원본 파일 정리 · 사용자 자료 정리 없음.

## 적용

- 실제 Corsair: beta.14 쓰기 마운트 활성화 · 5개 시험 파일 fsync/close/SHA 통과.
- 물리 장치 300–400MB/s: 미확인.
- 새 외부 의존성·라이선스 변경 없음.

## 최종 파일 성능 비교

DGX 내부 ext4 · 1GiB APFS 시험 이미지 · 파일당 256MiB · 동일 난수 입력 · fsync/close 포함. 자체 탐색·압축·다른 DGX 시험 종료 후 실행. 사용자 작업·시스템 캐시는 통제하지 않음. MB/s = 1,000,000바이트/초.

| 쓰기 단위 | beta.13 MB/s | beta.14 MB/s |
|---|---:|---:|
| 10KiB | 78.23 / 71.97 | 151.17 / 49.55 |
| 1MiB | 171.39 / 91.96 | 173.05 / 174.69 |

- 기록량: 양쪽 약 1.06배 · 이번 수정은 기록량 변경 없음.
- 판정: 체크섬 CPU 감소 확인 · 전체 처리량 향상률 확정 불가.
- 변동 근거: 수정본 각 512MiB workload의 `durable_sync` 누적 0.519 / 4.197초. 중첩 계측·동시 동기화 포함, 단계 합계로 총 실행 시간 계산 금지.
- 실제 Corsair beta.13 기존 결과: 256MiB/1MiB 쓰기 약 91.9MB/s · 1.47배. 이번 내부 이미지 값으로 대체 금지.
- 300–400MB/s 목표: 미달·실장치 추가 확인 필요.

[처리량 원본](validation/beta14-throughput.json) · [회귀 시험](validation/beta14-tests.json) · [Mac 기능 검사](validation/beta14-native.json)

## 빌드

- Linux ARM64 · Rust 1.92.0 release · fault-injection 비활성.
- SHA-256: `32430230ccab42b237b9f6deef20c59b2aa79d031d0c1deaca37d11d68e1e16c`.
- 후보 배포 위치: `/home/zooh/Documents/LAPFS/beta-0.3.0-beta.14`.
- 활성화: 기존 beta.13 세션 식별 → 열린 사용자 파일 대기 → 정상 분리 → 새 마운트 → 16/64/256MiB 시험·SHA·기록량 측정.
- 실제 마운트 교체 완료 · 정상 분리·장치 동기화·원래 파일 이름 불변 · 기존 파일 수정 0.
- 공개 릴리스: 별도 배포 영수증에서 확인.

[최종 바이너리 Mac 검사](validation/beta14-native-benchmark.json)

## Corsair 활성화 결과

2026-09-29 KST · beta.14 · 16/64/256/256/256MiB 시험 파일 · fsync/close/SHA 통과. 단위 MiB/s.

| 파일 | 쓰기 단위 | 속도 | OS 기록량 / 입력 |
|---|---:|---:|---:|
| 16MiB | 10KiB | 27.28MiB/s | 2.94배 |
| 64MiB | 1MiB | 92.19MiB/s | 1.59배 |
| 256MiB 1회 | 1MiB | 118.00MiB/s | 1.295배 |
| 256MiB 2회 | 1MiB | 115.74MiB/s | 1.293배 |
| 256MiB 3회 | 1MiB | 112.11MiB/s | 1.297배 |

기존 beta.13의 256MiB/1MiB 1회 결과는 87.61MiB/s · 1.470배. 다른 시점 시험이므로 속도 차이를 체크섬 수정 효과로 단정할 수 없음. beta.14의 3회 시험 내 속도 범위 112–118MiB/s. 300–400MB/s 목표 미달.

정상 분리 `device_sync=completed`, 이전 세션 `pending_bytes=0`, 새 세션 활성, 기존 파일 수정 0. 반복 읽기 SHA 일치. 장기 쓰기·물리 전원 차단·Mac 왕복은 이번 실장치 시험 범위 밖.

[실장치 결과](validation/beta14-physical.json)

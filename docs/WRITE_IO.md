# beta.11 · 쓰기 I/O

## 변경 전 영향 보고서

| 항목 | 범위 |
|---|---|
| 기준 | DGX `1433936` · beta.10 |
| 실장치 이전 검사 | 쓰기 24.9–32.8MB/s · fsync/close 포함 |
| 관측 | 동일 beta.10 로컬 반복 5.94–45.63MiB/s · 느린 실행의 영구 저장 대기 비중 증가 |
| 동시 부하 | 기존 Corsair 압축 백업 · 내부 SSD 99% 이상 사용 · swap/I/O 대기 |
| 수정 대상 | `src/buffered.rs` · `src/apfs_batch.rs` · `src/journal.rs` · 직접 관련 시험·버전·문서 |
| 호출 경로 | FUSE → Session.write/flush → payload 검증 → prepare → journal apply → readback/commit |
| 목표 | 파생 merged 파일 기록·동기화 제거 · 순차 원본 읽기 집계 |
| 데이터 영향 | 별도 시험 이미지 · 기존 Corsair 자료 변경 없음 |
| 실행 영향 | 연구·모델·GPU·스케줄러 변경 없음 · 기존 백업 유지 |

## 변경 후 영향 보고서

| 항목 | 결과 |
|---|---|
| 순차 데이터 전달 | 체크섬 검사된 RAM 데이터 → 준비 단계 직접 전달 |
| 제거 항목 | merged 파일 생성·전체 기록·fsync·재읽기 |
| 복구 기준 | 기존 영구 WAL 유지 · 종료 후 WAL에서 데이터 재구성 |
| 원본 읽기 | 첫 4KiB 읽기 후 연속 접근 확인 시 최대 1MiB 집계 |
| 임의 메타데이터 읽기 | 4KiB 유지 · 무조건 1MiB 선행 읽기 방지 |
| 중복·부분 쓰기 | 최초 undo 유지 · 최신 redo 우선 |
| 읽기 오류 | 선행 구간 오류 시 요청 블록 재검사 · 요청 블록 오류 전파 |
| 저장 순서 | WAL → queue manifest → undo/redo → APFS CoW → checkpoint flush → readback/commit 유지 |
| 호환성 | APFS 블록 형식·세션 버전·CLI 입력 형식 변경 없음 |
| 실장치 반영 | beta.11 정상 교체·성능 확인 대기 |

## 성능

DGX 내부 ext4의 128MiB APFS 이미지 · 파일당 40MiB · 실제 FUSE · SHA 검사 · fsync/close 포함. beta.10 / beta.11 / beta.11 / beta.10 순서. 기존 실장치 백업 동시 실행. 단위 MB/s = 1,000,000바이트/초.

| write 크기 | beta.10 MB/s | beta.11 MB/s |
|---|---|---|
| 10KiB | 15.27 / 22.53 | 34.92 / 24.18 |
| 1MiB | 27.89 / 26.76 | 24.71 / 27.66 |

- 큰 파일 전송 속도 개선: 이번 부하 조건에서 미확인
- 작은 write: 이번 ABBA에서 개선 관측 · 고정 성능 보장 아님
- daemon `wchar`: 약 40MiB 감소/40MiB 입력 · 파생 merged 기록 제거
- 1MiB write의 daemon `syscr`: 22,008 → 11,618
- 메모리 모델: 순차 원본 550블록 읽기 550회 → 4회 · 겹침·미기록 tail·오류 검사
- 최종 속도: 실장치 교체 후 별도 판정
- USB 20Gbps: 원시 링크 속도 · 사용자 파일 2,500MB/s 보장 아님
- 남은 비용: WAL/undo/redo 기록 · APFS 메타데이터 준비 · 동기화 · 재검증

[실측 전체](validation/beta11-throughput.json)

## 검증

| 시험 | 결과 |
|---|---|
| Rust workspace·명시 fixture | 302 통과 |
| 종료 경계 | durable/grouped 23개 · prefix retirement 7개 |
| 손상 로그 | 두 정책 · 잘린 tail/손상 frame/정상 frame 구분 |
| I/O 모델 | 순차 집계 · 겹침 쓰기 · 요청 밖/요청 안 읽기 실패 |
| Linux FUSE | 복사·범위 쓰기·rename·삭제·메타데이터·강제 종료 복구·원본104 SHA |
| rsync | 초기·반복·교체·링크 변경·삭제 5단계 |
| Mac | 복구 30 + FUSE 1 + rsync 1 = 32 이미지 · fsck clean · 원본104/변경 파일 SHA |
| 물리 USB 쓰기 성능 | beta.11 미검증 |
| 물리 전원 차단 | 미검증 |

[시험 요약](validation/beta11-tests.json) · [Mac 결과](validation/beta11-native.json)

## 바이너리

- SHA-256: `476d14b3fd89a59606ce1235d368aa4095f6a1581c8a5f90bb5671cc64c92985`
- 빌드: DGX Linux ARM64 · Rust 1.92.0 · release · fault-injection 비활성
- 시험 빌드: fault-injection 활성 · 배포 제외
- 시험 산출물: `evidence/` · 완료 이미지 gzip·SHA 검증 후 압축 보관
- 한계: SIGKILL은 실제 정전·USB 캐시 소실 재현 아님

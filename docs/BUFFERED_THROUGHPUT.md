# beta.8 · 쓰기 처리량

## 변경 전 영향 보고서

| 항목 | 범위 |
|---|---|
| 기준 | DGX main `f499b6012ff2e276e123e570ff170a1e930aa770` |
| 문제 | 작은 write마다 payload 파일 생성·fsync·디렉터리 fsync·manifest 교체; 63개 작업마다 APFS 반영 |
| 실제 관측 | beta.5 Corsair tar · 약 10KiB 쓰기 · 5초 구간 0.459MiB/s |
| 대상 | `src/buffered.rs`, `src/mount_rw.rs`, `src/main.rs`, 버전·시험·운영 문서 |
| 호출 경로 | FUSE write → Session.write → 입력 로그 → flush → APFS prepare/apply |
| 정책 승인 | 일반 파일시스템 방식 · fsync/close/정상 분리 시 영구 저장 · 마지막 미동기화 입력 유실 허용 |
| 실제 장치 | 진행 중 전송·연구 파일 변경 없음; 원본 재계산 없음 |
| 별도 영향 | 스케줄러·GPU·모델·메일·외부 서비스 변경 없음 |

## 변경 후 영향 보고서

| 항목 | 결과 |
|---|---|
| 기본 정책 | 새 FUSE 세션 `grouped`; 이전 세션 저장 정책 유지 |
| 선택 정책 | `--grouped-writes`, `--durable-writes`; 재개 시 명시 정책 불일치 거부 |
| 기록 형식 | v2 append 로그 · 길이 헤더 checksum · metadata+data checksum |
| 동기화 순서 | 로그 fsync → queue manifest 영구 공개 → APFS 변경 |
| 작업 묶음 | 데이터 4MiB 또는 1,023개 작업 경계; 메모리·디스크 사용량 제한 유지 |
| 경로 검사 | 독점 세션 내 마지막 쓰기 파일 속성/허용 여부 캐시 · APFS 반영 전 무효화 |
| 복구 | v1 입력 호환 · 불완전한 마지막 frame 제외 · 완전한 frame checksum 오류 거부 |
| 로그 정리 | 공유 로그의 모든 참조 종료 후 삭제 · 미완료 복구 기록 유지 |
| APFS 기록 코드 | beta.7 CoW·할당표·체크포인트 구현 변경 없음 |
| 검증 바이너리 | `6cd4bb46f1c91d0761c7adb20bc75823aac9ced65ec97cb865ac79609ef68ba8` |

## 성능

DGX 내부 ext4의 128MiB APFS 시험 이미지 · 파일당 16MiB · 실제 FUSE write/fsync/close 포함 · 파일 SHA 확인 · 순서 old/new/new/old · 두 회씩. `old`: beta.7 durable, `new`: beta.8 grouped. 정책 변경을 포함한 비교이며 알고리즘 단독 효과 아님.

| 쓰기 크기 | beta.7 MiB/s | beta.8 MiB/s |
|---|---|---|
| 10KiB | 3.72 / 3.82 | 20.60 / 20.24 |
| 1MiB | 17.72 / 18.17 | 23.24 / 22.35 |

작은 쓰기 약 5.3~5.5배 개선. 실제 USB 처리량, 원시 장치 속도 대비 손실률, 장기 지속 속도: 미측정. 초기 실험은 시스템 I/O 부하에 따른 편차 포함; 선택적 최대치 인용 없음. '속도 손실 0' 보장 없음.

## 검증

| 시험 | 결과 |
|---|---|
| Rust workspace · fixture · fault-injection | 298 통과; worker 2개는 부모 시험 내부 실행 |
| 강제 종료 | durable 12 / grouped 11 경계, 복구 통과 |
| 잘린 로그·손상 | 두 정책 × 12개 경계/정상/손상 조건; 완전한 손상 frame 거부 |
| 작은 쓰기·교차 파일·덮어쓰기 | 400개 10KiB write의 조기 commit 방지; 공유 로그 참조·재개 후 append 검증 |
| Linux FUSE | 25개 기능 확인 · 26개 commit 배치 · 실제 daemon SIGKILL 후 복구 |
| 이전 세션 | v1 ACK 복구 · 명시 정책 불일치 거부 · 기존 durable 유지 · 새 grouped 기본값 통과 |
| rsync | 초기·반복·교체·링크 변경·삭제 5단계 |
| Mac 독립 검사 | 27개 이미지 전체 fsck clean · 각각 원본 104개 SHA 및 변경 파일 SHA 일치 |
| Mac rsync 메타데이터 | mode 0640 · ns mtime · symlink · 삭제 상태 일치 |
| 문서 | desktop/mobile · 4단계·키보드·URL·no-JS·SVG 범위 검사 |

프로세스 SIGKILL은 호스트 페이지 캐시를 지우지 않음. grouped 모드의 'write 응답 이후 SIGKILL 복구'는 실제 전원 차단 후 입력 보존 증거가 아님. fsync 이전 데이터 유실 허용; APFS 구조 보호와 입력 데이터 영구 저장은 별도 기준.

## 저장 위치·호환

- 입력 로그: 세션 디렉터리 `stream-*.bin`
- 복구 기록: 동일 세션의 `txn-*`, `session.json`
- 오류 로그: 기존 별도 JSONL 경로
- v2 세션: beta.8 이상 복구 필요 · beta.7 이하 재개 거부
- 이전 버전 복귀: beta.8 정상 분리·복구 완료 후 새 세션
- 데이터 상한 4MiB 외 frame metadata·undo/redo·manifest 공간 별도
- 이번 작업의 시험 이미지 95개: gzip 후 원본 SHA 재확인 · 약 12.7GB 회수 · 사용자 연구 파일 삭제 없음

## 근거

- [성능 영수증](validation/beta8-throughput.json)
- [Mac 27개 이미지 영수증](validation/beta8-native.json)
- [시각 문서 검사](validation/beta8-visuals.json)
- `tests/bench_buffered_throughput.py`, `tests/buffered.rs`, `tests/verify_queue_upgrade.py`
- 실제 케이블 분리·전원 차단·USB 브리지 flush 정직성·전체 APFS/POSIX·상용 인증: 미검증

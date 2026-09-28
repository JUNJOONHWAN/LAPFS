# beta.10 · 읽기 처리량

## 변경 전 영향 보고서

| 항목 | 범위 |
|---|---|
| 기준 | DGX main `d7dbf36b4b683c2936b462df792c1ef8265c6c84` · beta.9 |
| 실제 연결 | 커널 speed=20000 · RX/TX 2레인 · UAS · SuperSpeed Plus Gen 2x2 |
| 원시 장치 읽기 | O_DIRECT · 서로 다른 2개 위치 · 각 256MiB · 803 / 1,109MiB/s |
| 기존 파일 읽기 | Corsair 약 83MiB MOV · 50.05 / 54.93MiB/s |
| 한계 | 원시 읽기와 파일 읽기: 서로 다른 위치·요청 경로; 직접적인 성능 배율 비교 불가 |
| 원인 후보 | 작은 FUSE 읽기마다 Session.attr/view · APFS 컨테이너/카탈로그 열기 · 메타데이터 탐색 반복 |
| 변경 대상 | `src/buffered.rs` 읽기 · 캐시 무효화 · `tests/buffered.rs` · 버전·문서 |
| 호출 경로 | FUSE read → Session.read → attr/FsView.read_range |
| 실장치 | 확인용 원시 읽기만 수행; 쓰기·분리·드라이버 변경 없음 |
| 별도 작업 | 연구·GPU·모델·스케줄러 변경 없음 |

## 변경 후 영향 보고서

| 항목 | 결과 |
|---|---|
| 선행 읽기 | 최대 8MiB · 파일 1개 · 구간 1개 · RAM 캐시 |
| 캐시 내용 | APFS 반영 데이터; 응답마다 미반영 입력 overlay 적용 |
| 무효화 | write · flush · 메타데이터 변경에 수반된 flush · 복구/commit |
| 일관성 | 독점 장치 FD 유지 · 다른 파일 접근 시 캐시 교체 |
| 요청 범위 밖 오류 | 선행 읽기만 실패하면 요청된 정확한 범위 재검사; 해당 범위 오류 유지 |
| APFS 기록 | 파일 형식·할당표·CoW·저널·flush 순서 변경 없음 |
| 추가 메모리 | 캐시 데이터 최대 8MiB; 출력 사본·기존 큐/카탈로그 메모리 별도 |
| 적용 조건 | 정상 분리 후 새 프로세스; 현재 세션 정책 유지 |

## 최종 빌드 성능

내부 ext4의 합성 APFS 이미지 · 기존 40MiB 파일 · SHA 계산 포함 · beta.9/beta.10/beta.10/beta.9 순서. 파일 배치 동일. 운영 부하 유지. 짧은 시험이며 OS/SSD 캐시 영향 포함. USB 처리량 수치 아님.

| read 단위 | beta.9 MiB/s | beta.10 MiB/s |
|---|---|---|
| 128KiB | 345.64 / 268.22 | 548.17 / 752.95 |
| 1MiB | 464.22 / 530.35 | 912.03 / 818.49 |

- 같은 APFS 형식·파일 배치에서 코드 변경만으로 개선
- 현재 근거: LAPFS의 요청별 반복 비용 개선 가능
- 미확정: 실제 Corsair beta.10 속도 · 전체 파일 분포 · 단편화 영향 · 장기 지속 속도
- 원시 USB 읽기: 480Mbps 연결이 현재 성능 상한이라는 가설과 불일치
- 2,500MB/s 보장 없음; 읽기 개선을 쓰기 개선으로 해석 금지

## 검증

| 시험 | 결과 |
|---|---|
| Rust workspace + fixture + fault injection | 301 통과; 자식 worker 2개는 부모 시험 내부 실행 |
| 캐시 회귀 | 8MiB 경계 · pending overwrite · commit · append · truncate · rename · unlink/recreate |
| Linux FUSE | 전체 기존 기능 시험 통과 · 원본 104개 파일 SHA |
| rsync | 초기·반복·교체·링크·삭제 통과 |
| 정상 분리 | 기본/grouped/durable/죽은 daemon · busy/다른 세션 거부 |
| macOS 독립 검사 | 최종 FUSE/rsync 이미지 2개 · fsck clean · 원본/변경 파일 SHA·메타데이터 일치 |
| 기존 APFS prefix matrix | beta.9의 430개 결과 유지; 기록 알고리즘 변경 없음; 이번 버전 재실행 없음 |

## 근거

- [최종 성능](validation/beta10-throughput.json)
- [시험 결과](validation/beta10-tests.json)
- [Mac 검사](validation/beta10-native.json)
- 바이너리 SHA: `cbde358805de6893be037eba591484f7083782b41ed98877e85411f7b7c16f8c`
- 원시 장치 확인: 읽기 전용 `dd iflag=direct`, 결과 `/dev/null`; 커널 캐시 비우기·장치 쓰기 없음

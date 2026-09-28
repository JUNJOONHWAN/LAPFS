# beta.9 · 순차 기록

## 변경 전 영향 보고서

| 항목 | 범위 |
|---|---|
| 기준 | DGX main `c9f9041844edb5b9eef094e4c0c0f0b468a5a1bf` · beta.8 |
| 실장치 관측 | Corsair 16MiB / 10KiB write · fsync/close 포함 10.48MiB/s |
| USB 연결 | DGX `lsusb -t`: 20000M/x2 · UAS |
| 병목 | 작은 입력마다 파일 열기·stat·append; 4KiB journal syscall; 작은 배치의 반복 동기화; ARM SHA 가속 비활성 |
| 변경 대상 | 입력 큐 · journal I/O · WriteAt 배치 · SHA 의존성 · 정상 분리 프로세스 식별 · 시험·문서·버전 |
| 호출 경로 | FUSE → Session.write → freeze/actions → prepare → journal apply → readback/commit |
| 쓰기 정책 | 사용자 승인: 일반 파일시스템 방식 · fsync 이전 입력 유실 허용 |
| 실장치 영향 | 새 버전 적용 전 기존 beta.8 마운트 유지; 정상 분리 후 새 세션 필요 |
| 별도 작업 | 연구·모델·GPU·스케줄러·메일 변경 없음 |

## 변경 후 영향 보고서

| 항목 | 결과 |
|---|---|
| 기본 입력 | RAM 체크섬 frame · 데이터 32MiB 또는 8,191개 작업 경계 |
| 영구 저장 | 입력 로그 fsync → queue manifest → undo/redo fsync → APFS CoW → checkpoint flush → readback/commit |
| 로그 입력 | 1MiB BufWriter · flush 후 파일 fsync · 디스크/미기록 버퍼 read-your-writes |
| 로그 검사·적용 | 인접 4KiB 페이지의 1MiB I/O 집계 · 각 페이지 SHA 유지 |
| 순서 | 명시 flush 경계 통과 금지 · 중복/겹침 쓰기 순서 유지 |
| SHA-256 | ARM 하드웨어 가속 · 지원 CPU 런타임 검사 · 소프트웨어 대체 경로 |
| 배치 | 범위 쓰기 32MiB · undo+redo 128MiB · 기존 4MiB/32MiB 세션 재개 호환 |
| 정상 분리 | grouped/durable 옵션 포함 PID 식별 · 다른 세션 거부 · 열린 파일 거부 |
| 복구 | 모드별 종료 경계 · 완전한 frame 손상 거부 · 미완료 기록 보존 |
| 새 의존성 | sha2-asm · cc · find-msvc-tools · shlex; 원본/라이선스 오프라인 번들 |

## 처리량

DGX 내부 ext4의 128MiB 합성 APFS 이미지 · 파일당 40MiB · 32MiB 경계 통과 · 실제 FUSE · fsync/close 포함 · 모든 파일 SHA 확인. 두 버전 모두 grouped 정책. 실행 순서 beta.8/beta.9/beta.9/beta.8.

| write 크기 | beta.8 MiB/s | beta.9 MiB/s |
|---|---|---|
| 10KiB | 11.54 / 18.43 | 54.06 / 54.16 |
| 1MiB | 25.31 / 20.43 | 60.91 / 66.96 |

- 작은 write: 관측 범위 기준 약 2.9–4.7배
- 최종 측정: 다른 LAPFS 시험 동시 실행 없음; 기존 사용자 작업 유지
- 사전 동시 부하 측정: beta.8 9.66–16.48 / beta.9 28.37–51.19MiB/s
- 환경: 내부 SSD 99% 사용 · 메모리 압박·swap 관측 · 고정 부하 환경 아님
- USB 실장치 beta.9 처리량: 적용 후 확인 필요
- 20Gbps: 원시 전송률 2,500MB/s; 사용자 파일 처리량 보장 아님
- 미달 항목: 2,500MB/s 파일 쓰기 · 원시 장치 대비 손실 0 · 장기 지속 성능
- 남은 비용: 입력/merged/undo/redo 추가 기록 · 동기화 · 메타데이터 준비 · 재검증

[두 측정 전체 결과](validation/beta9-throughput.json)

## 검증

| 시험 | 결과 |
|---|---|
| Rust workspace · fixture · fault injection | 300 통과 · 자식 worker 2개: 부모 시험 내 실행 |
| SHA 가속 | empty · abc · 백만 a 표준 벡터 일치 |
| journal | 2MiB 초과 버퍼 교차 읽기 · 겹침 쓰기 · flush 순서 · 실제 집계 I/O 실패 경계 전체 복구 |
| 입력 정책 | durable 12 / grouped 11 종료 경계 · RAM ACK 유실 조건 명시 |
| 손상 로그 | 두 정책 × 12개 조건 · 잘린 tail / 완전한 frame 손상 구분 |
| FUSE | 읽기·쓰기·복사·rename·삭제·메타데이터·원본 104개 SHA · durable daemon SIGKILL 복구 |
| rsync | 초기·반복·교체·링크·삭제 5단계 |
| 이전 세션 | beta.7 v1 · beta.8 v2 durable ACK 재개; 정책 불일치 거부 |
| Mac 복구·FUSE·rsync·40MiB 복사 | 27 이미지 · fsck clean · 원본/변경 파일 SHA · 메타데이터 일치 |
| Mac 체크포인트 중단 | 430 이미지 · 모든 논리 쓰기/flush prefix · torn NX 512바이트 경계 포함 |
| 정상 분리 | 기본·명시 grouped·명시 durable·죽은 durable daemon · EBUSY/다른 세션 거부 |
| 문서 | SVG/PNG · HTML desktop 1440/mobile 390 · 가로 넘침 없음 |

실제 전원 차단·케이블 분리 시험 아님. RAM 입력은 fsync 이전 프로세스 종료에도 유실 가능. `--durable-writes`의 ACK는 내부 로그 영구 저장 기준이며 USB 반영 시점과 다름. 지원되지 않는 APFS 기능은 기존 명시적 거부 유지.

## 바이너리

- 시험 빌드: `308426f80d9977a03b8dab42f843604dc607ad6b33967f1e32ebec446080c752`
- 배포 빌드: `37b1e1fa640e7aea24350bbee8820eed19a927b550ff2dc90bf6a165fa6d1f25`
- 차이: 시험 후 trailing whitespace 정리로 debug 위치/build ID 변경
- 동등성: ELF 적재 섹션 26개 전체 일치; build ID 제외
- [동등성 영수증](validation/beta9-binary-equivalence.json)

## 저장 위치

- RAM: 동기화 전 입력 · 데이터 32MiB + frame metadata
- 내부 세션: `stream-*`, `merged-*`, `txn-*`, `session.json`
- 메모리·디스크: payload 외 metadata/검증 사본/manifest/할당표 비용 별도; 전체 RAM 상한 미인증
- 실패 복구 기록: 자동 제거 없음
- beta.9 새 세션: beta.9 이상 재개 필요; 구버전 복귀 전 정상 분리
- 계측: `LAPFS_PROFILE=1` · 단계별 마이크로초 · 로컬 stderr · 기본 비활성

## 근거

[Mac 27개](validation/beta9-native.json) · [Mac 중단 430개](validation/beta9-matrix.json) · [시험 요약](validation/beta9-tests.json) · [화면 검사](validation/beta9-visuals.json)

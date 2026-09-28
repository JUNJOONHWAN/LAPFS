# beta.15 · 같은 볼륨의 폴더 간 이동

## 변경 전 영향 보고서

| 항목 | 확인 |
|---|---|
| 기준 | DGX canonical main `8b6eb356d6317acf16fc4b7259929cd5839c656a` · beta.14 |
| 실제 Corsair | beta.14 쓰기 FUSE 세션 PID 394314 · 소스 변경 중 유지 |
| 증상 | `Session::rename`이 다른 부모 경로를 `EXDEV`로 거부 |
| 호출 경로 | FUSE rename → Pipeline → Session → queued APFS Action → vendored CoW writer → journal apply/recover |
| 직접 영향 | 파일·폴더 이동, 대상 교체, 열린 자식 파일 경로 캐시, 중단 복구 |
| 영향 제외 | 연구 원본·백업·GPU 작업·스케줄러·다른 저장 장치 |

## 변경 후 영향 보고서

| 항목 | 결과 |
|---|---|
| 수정 파일 | `src/apfs_batch.rs`, `src/buffered.rs`, `src/inode_paths.rs`, `src/mount_rw.rs` |
| 동작 | 같은 APFS 볼륨 내 파일·폴더 이동, 같은/다른 폴더 rename, 대상 파일·빈 폴더 교체 |
| 거부 | 자기 자손으로 폴더 이동, 비어 있지 않은 대상 폴더, 파일/폴더 종류 불일치, 열린 대상, 변경 불가 inode |
| 복구 | 교체 시 대상 제거와 이동을 하나의 durable queue로 발행; APFS catalog 변경은 기존 CoW/undo/redo/flush/recheck 적용 |
| 캐시 | 이동 전 자식 경로 계획·할당, 성공 후 알려진 모든 descendant inode 경로 갱신 |
| 호환성 | 새 `Move` queue 기록은 beta.15 이상에서 복구; 구버전으로 되돌리기 전 정상 분리 필수 |
| 실제 Corsair | beta.14 유지 · beta.15 활성화/실장치 파일 이동 시험 대기 |

## 검증

- DGX Rust workspace + fault-injection 테스트 통과.
- 별도 128MiB APFS 시험 이미지: 파일·폴더 이동, inode/내용 유지, 파일·빈 폴더 교체, 파일/폴더 종류 오류.
- 실제 Linux FUSE 시험: `os.rename`/`os.replace`, 이동 전 열린 자식 파일 핸들 읽기, 기존 104개 파일 SHA, 일반 읽기·쓰기·복구 작업 통과.
- macOS 읽기 전용 마운트 + `fsck_apfs -n`: 이동 이미지 2개, FUSE 이미지 1개, forced SIGKILL 복구 이미지 3개 clean. 이동 후 파일 SHA 확인.
- 강제 종료 지점 `state-Applying`, `apply-write`, `state-Committed` 각각 복구 뒤 새 대상 내용과 Mac fsck 통과.
- 실제 Corsair의 폴더 간 이동·큰 카탈로그 저널량·속도는 아직 미검증. 활성화 성공 전 상용/실장치 완료 주장 금지.

## 제한

- 현재 APFS `move_entry`는 카탈로그를 수집·재작성한다. 실제 TB급 볼륨에서 이동 시간 및 저널 cap을 시험해야 한다.
- 물리 전원 차단, 거짓 장치 flush, 여러 APFS 형식/USB 브리지 조합은 이 시험으로 검증되지 않는다.

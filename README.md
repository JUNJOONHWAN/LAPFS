<p align="center"><img src="docs/assets/lapfs-banner.svg" alt="LAPFS — buffered APFS access for Linux ARM64" width="100%"></p>

<p align="center">
  <a href="https://github.com/JUNJOONHWAN/LAPFS/releases"><img alt="Beta" src="https://img.shields.io/badge/release-0.3.0--beta.7-f5b84b"></a>
  <img alt="Platform" src="https://img.shields.io/badge/target-DGX%20Spark%20%2F%20Linux%20ARM64-72d6c9">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-GPL--3.0--only-829bff"></a>
</p>

# LAPFS

**Linux에서 APFS를 읽고 씁니다. 작은 영구 큐와 데이터·할당표 CoW로 중단 시 일관성을 보강한 공개 베타입니다.**

Designed for DGX Spark / GB10: readable APFS volumes, bounded durable write buffering, explicit recovery, and separate error logs. The canonical source and release build are maintained on DGX; macOS provides independent APFS validation.

| 430 / 430 | 296 | 3회 |
|:---:|:---:|:---:|
| Mac 중단 이미지 검사 | Rust 시험 통과 | Mac ↔ Linux 왕복 |

> [!IMPORTANT]
> **beta.7은 파일 데이터와 공간 할당표까지 CoW로 기록합니다.** 새 체크포인트를 공개하기 전 기존 활성 상태를 보존하도록 바꿨습니다. 위 숫자는 이미지·소프트웨어 시험 결과이며, 실제 USB 전원 차단이나 상용 드라이버 수준의 인증을 뜻하지 않습니다. 중요한 데이터의 유일한 사본에 사용하지 마세요.

**[HTML 가이드 다운로드 · 오프라인 단계별 보기](https://github.com/JUNJOONHWAN/LAPFS/releases/download/v0.3.0-beta.7/LAPFS-beta7-architecture.html)** · [HTML 소스](docs/architecture.html) · [검증 영수증](docs/validation/native-cow-beta7.json)

[시작하기](#빠른-시작) · [구조](#구조) · [중단 시 동작](#중단-시-동작) · [사양](#목표와-현재-사양) · [지원 제한](docs/LIMITATIONS.md) · [정상 분리·복구](docs/RECOVERY.md) · [English](docs/OVERVIEW.md)

## 왜 만들었나

APFS 외장 저장장치를 Linux ARM64에서 다루되, 내부 디스크에 전체 드라이브나 큰 파일을 복제하지 않는 것이 목표입니다. 쓰기 성공 응답 전에 입력을 영구 저장하고, 작은 배치로 반영하고, 중단된 작업의 복구 기록을 유지합니다.

## 구조

![DGX 내부 영구 큐와 USB APFS의 CoW·체크포인트 반영 경계를 보여주는 구조도](docs/assets/lapfs-architecture.svg)

[SVG 원본](docs/assets/lapfs-architecture.svg) · [PNG 그림](docs/assets/lapfs-architecture.png)

<details>
<summary>구조도 텍스트 소스 · Mermaid</summary>

```mermaid
flowchart LR
    App["cp / rsync -a / 일반 파일 I/O"] --> FUSE["읽기·쓰기 FUSE\ndirect I/O"]
    FUSE --> Queue["DGX 내부 ext4/XFS\n영구 입력 큐 · 기본 4 MiB"]
    Queue --> Trigger["4 MiB / fsync / close\n메타데이터 변경 / 정상 해제"]
    Trigger --> Journal["외부 undo + redo\nflush · 블록 재검증"]
    Journal --> COW["파일 데이터 · catalog · extent CoW"]
    COW --> Space["할당 bitmap · CIB · 내부 pool bitmap CoW"]
    Space --> Barrier["참조 블록 flush"]
    Barrier --> Checkpoint["새 ring 체크포인트 기록 · flush"]
    Checkpoint --> APFS["지원 조건을 통과한 APFS"]
    Queue -. "중단 후 재개" .-> Recover["mount-recover"]
    Recover --> Journal
    FUSE -. "작업·errno" .-> Log["별도 JSONL 오류 로그\n회전 보관 약 8 MiB"]
```

</details>

| 응답 시점 | 보장하는 범위 |
|---|---|
| `write()` 성공 | 입력 payload와 큐 메타데이터가 **DGX 영구 저장소에 저장**됨. USB에 모두 반영됐다는 뜻은 아님 |
| `fsync()` / close-flush 성공 | 해당 시점까지 큐 반영, 장치 flush, 변경 블록 읽기 재검증, commit 기록 완료 |
| 중단 / 오류 | 이전 또는 새 체크포인트 보존을 목표로 CoW 반영. 미반영 입력과 복구 기록은 DGX에 보존하며, 검증 범위는 아래 보고서 참고 |
| 정상 해제 | 마지막 반영 후 소유권 해제. 해제 오류가 나면 복구 필요 |

**beta.7은 파일 데이터와 공간 할당표까지 CoW로 기록합니다.** 새 체크포인트를 공개하기 전 기존 활성 체크포인트를 보존하고, bootstrap 블록과 이전 spaceman을 덮어쓰지 않습니다. DGX 저널 복구 없이 Mac에서 검사한 중단·부분 NX 기록 이미지의 결과는 [검증 보고서](docs/NATIVE_COW_IMPACT.md)에 있습니다. 외부 큐에만 저장된 최신 입력은 USB에 없을 수 있으며, 실제 전원 차단·USB 캐시 신뢰성은 아직 인증하지 않았습니다. 정상 분리 절차를 계속 사용하세요.

## 중단 시 동작

![새 NX 공개 전에는 이전 체크포인트, 공개 중에는 이전 또는 새 체크포인트, 최종 flush 뒤에는 새 체크포인트를 사용한다](docs/assets/lapfs-checkpoint.svg)

- **공개 전:** 기존 활성 체크포인트를 유지합니다. DGX 큐에만 있는 입력은 USB에 없을 수 있습니다.
- **공개 중:** 유효한 checksum을 가진 이전 또는 새 체크포인트로 열립니다.
- **반영 완료:** 새 상태를 사용합니다. 이전 체크포인트는 영구 백업이나 스냅샷이 아닙니다.

실제 사용에서는 `scripts/safe-eject.py`로 정상 해제하세요. 이 그림은 지원 형식과 올바른 장치 flush 동작을 전제로 합니다. [구현·중단 시험의 정확한 범위](docs/NATIVE_COW_IMPACT.md)

## 목표와 현재 사양

| 항목 | 현재 beta.7 | 목표 / 남은 검증 |
|---|---|---|
| 기준 실행 환경 | DGX Spark / GB10, Linux ARM64, FUSE3 | 다른 배포판·USB 브리지 조합 검증 |
| 읽기 | 일반 파일, 디렉터리, 링크 조회, 큰 파일 범위 읽기 | 암호화·압축 스트리밍 확대 |
| 쓰기 | 생성·복사·범위 수정·append·파일 삭제·mkdir·빈 폴더 삭제(rmdir)·같은 폴더 파일 rename/replace·심볼릭 링크 생성/삭제·chmod·atime/mtime | 더 넓은 POSIX/APFS 기능 |
| 쓰기 큐 | 기본 데이터 상한 **4 MiB** | 처리량·동시 작업 성능 측정 |
| 배치 복구 데이터 | undo + redo **32 MiB** 상한 | 대형 catalog에서 지원 범위 검증 |
| 내부 저장공간 | **1 GiB 여유 + 96 MiB 작업 여유 검사** | 최저공간·장기 반복 부하 실측 |
| 메모리 | 전체 파일/드라이브 staging 없음 | catalog·프로세스 총 RAM 상한은 미인증 |
| 오류 로그 | 2 MiB × 현재1 + 보관3, 약 **8 MiB** | 현장 장애 분류 확장 |
| 성능 | 합성 이미지: 5,000개 원본 보존 + 생성/변경/삭제 121.9초 | USB 3.2 지속 처리량 수치 **미제시** |
| macOS | 독립 `fsck_apfs` / 파일 SHA 검증 | macOS FUSE 제품 제공 안 함 |

공간 숫자는 각 계층의 제한이며 전체 작업 공간의 고정 사용량이나 파일시스템 전체 용량 보장은 아닙니다. 실패한 세션/복구 기록은 자동 삭제하지 않습니다.

## 빠른 시작

Linux ARM64, Rust/Cargo (DGX 검증: Rust 1.92.0), C 링커, Python 3, FUSE3(`/dev/fuse`, `fusermount3`)가 필요합니다. 먼저 **함께 제공한 합성 이미지**로 시험하세요.

```bash
git clone https://github.com/JUNJOONHWAN/LAPFS.git
cd LAPFS
cargo build --locked --offline --release --bin lapfs
./target/release/lapfs --version

# 새 시험 이미지 생성 → 실제 RW FUSE 기능/오류/강제종료 복구 검증
python3 tests/verify_linux_fuse.py --binary target/release/lapfs
python3 tests/verify_rsync_archive.py --binary target/release/lapfs
```

배포 바이너리는 GNU/Linux ARM64, **glibc 2.39 이상과 libgcc_s**가 필요합니다. 다른 환경은 소스 빌드를 사용하세요.

이 검사는 `evidence/`에 별도 이미지를 만들고 결과를 남깁니다. 영구 큐 때문에 해당 위치는 **로컬 ext4/XFS**여야 합니다. 관리자 권한 없이 동작하는 FUSE 설정과 1 GiB 이상의 여유가 필요합니다. fixture 압축 해제 크기는 128 MiB입니다.

<details>
<summary><b>이미지 직접 마운트</b></summary>

```bash
mkdir -p work/mount
# 새 파일에만 압축을 풉니다. 기존 작업 이미지를 덮어쓰지 마세요.
python3 - <<'PY'
import gzip, shutil
with gzip.open('fixtures/block-test.dmg.gz', 'rb') as src, open('work/test.dmg', 'xb') as dst:
    shutil.copyfileobj(src, dst)
PY
# 제공 fixture의 APFS GPT 파티션 오프셋만 20480입니다.
./target/release/lapfs mount-rw work/test.dmg 20480 work/mount work/session-01
```

위 명령은 전경 실행합니다. 다른 터미널에서 파일을 쓰고, 열린 파일을 닫은 뒤 `fusermount3 -u work/mount`로 해제합니다. 정상 종료된 session 디렉터리는 새 마운트에 재사용하지 않습니다.
</details>

실제 장치는 by-id·컨테이너 UUID 등록 및 별도 내부 디스크 복구 경로가 필요합니다. [운영 및 복구 설명](docs/RECOVERY.md)을 먼저 확인하세요. 자동으로 드라이브를 검색해 쓰기 권한을 여는 명령은 제공하지 않습니다.

## 검증 상태

| beta.7 시험 | 결과와 범위 |
|---|---|
| **중단 이미지 430 / 430** | 318개 I/O 경계 + 112개 부분 NX 기록. DGX 복구 없이 Mac fsck, 원본 104개 SHA와 변경 대상의 이전/새 상태 확인 |
| **Rust 296개** | 단위·회귀·실제 이미지 복구 시험. 자식 프로세스 전용 2개 항목은 부모 crash 시험에서 실행 |
| **Mac ↔ Linux 3회** | Mac 기록 → Linux 수정 → Mac fsck·내용·권한 검사 |
| **FUSE / rsync** | 27개 commit 배치와 rsync 5단계. Mac에서 SHA·mode·나노초 mtime·링크·삭제 결과 확인 |
| **272,629,778바이트 파일** | 새 512 MiB APFS 이미지에서 여러 할당 영역 기록. Mac fsck·전체 SHA 일치 |
| **미검증** | 실제 전원 차단·USB 브리지 캐시 신뢰성·TB급 장기 부하·전체 APFS/POSIX 호환 |

[변경 전후 보고서](docs/NATIVE_COW_IMPACT.md) · [바이너리·시험 영수증](docs/validation/native-cow-beta7.json) · [430개 사례 전부](docs/validation/native-cow-beta7-matrix.json)

<details>
<summary>이전 베타의 물리 장치·호환성 검사</summary>

beta.5의 선택한 Corsair 장치에서는 rsync 5단계, 다섯 연구 루트의 이름 순회(디렉터리 17,368개·파일 397,593개·링크 388개), 이전 EIO 795개 경로 재검사를 통과했습니다. beta.3/4에서는 약 1 TB 장치의 8 MiB 파일 쓰기·fsync·해제·재마운트·삭제를 확인했습니다.

이는 해당 빌드·장치의 기록이며 beta.7의 물리 전원 차단 시험이 아닙니다. [빌드별 전체 기록](docs/VALIDATION.md) · [beta.6 정상 분리 시험](docs/HANDOFF_AFTER.md)

</details>

## 현재 지원하지 않는 것

- 암호화 볼륨 쓰기, 다중 볼륨 컨테이너, 스냅샷, CAB 간접 할당 구조 등 지원하지 않는 allocator layout.
- clone/hardlink/shared/compressed/sparse/특수 파일 변경, 다른 소유자로의 chown 및 미검증 xattr/ACL 변경. `rsync -a`는 일반 파일·디렉터리·심볼릭 링크와 현재 사용자 소유권 범위에서 검증했습니다.
- 디렉터리 rename과 비어 있지 않은 폴더의 직접 제거, 다른 폴더 간 rename, 열린 파일 삭제·열린 목적지 교체. 빈 폴더의 `rmdir`은 지원합니다.
- 다른 소유자로의 `chown`, 임의 xattr/ACL 변경, hardlink 생성, writable mmap. 일반 파일 `chmod`·atime/mtime과 심볼릭 링크 생성/삭제는 지원합니다.
- 새 크기 **8 MiB 초과 truncate**. 큰 파일 복사·append·범위 쓰기의 파일 크기 제한과는 다릅니다.
- `cp -a`, 데이터베이스, 임의 앱 저장 프로토콜 전체의 호환성 보장.

## 문서와 소스

```text
src/                 CLI · 읽기 · 영구 큐 · 복구 저널 · FUSE · 오류 로그
vendor/              수정된 APFS crates와 fuser
dependencies/       Cargo.lock의 registry 의존성 원본
tests/              Rust 회귀 / Linux FUSE / macOS 독립 검증
fixtures/           합성 APFS 이미지와 예상 파일 해시
docs/               HTML 가이드 · SVG 구조도 · 복구 · 제한 · 시험 증거
```

## 라이선스와 출처

**GPL-3.0-only** — [LICENSE](LICENSE). APFS 구현은 [apfs-explorer](https://github.com/enesilhaydin/apfs-explorer)의 고정 커밋을 기반으로 수정했습니다. FUSE 계층은 MIT 라이선스의 [fuser](https://github.com/cberner/fuser)를 사용합니다. [변경·출처 기록](UPSTREAM.md), [전체 의존성 고지](THIRD_PARTY_NOTICES.md)를 함께 제공합니다.

재배포 시 해당 라이선스·저작권 고지와 대응 소스를 보존하세요. 이 프로젝트는 Apple 또는 NVIDIA와 제휴하거나 이들의 인증을 받은 제품이 아닙니다.

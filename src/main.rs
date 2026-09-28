use anyhow::{bail, Context, Result};
use spark_apfs_safe::{
    apfs_batch,
    journal::{read_receipt, resume_cleanup, Image, Journal, State, DEFAULT_CAP, DEFAULT_RESERVE},
};
use std::path::Path;

fn main() {
    let op = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    if !matches!(
        op.as_str(),
        "help" | "--help" | "-h" | "version" | "--version"
    ) {
        if let Err(e) = spark_apfs_safe::error_log::init() {
            eprintln!("오류 로그 초기화 실패: {e:#}");
            std::process::exit(1);
        }
    }
    if let Err(e) = run() {
        spark_apfs_safe::error_log::event("error", &op, None, &format!("{e:#}"));
        eprintln!("중단: {e:#}\n미완료 작업의 복구 기록을 삭제하지 마세요.");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let arg = |n: usize| {
        a.get(n)
            .map(String::as_str)
            .context("Missing argument; run help")
    };
    match a.get(1).map(String::as_str).unwrap_or("help") {
        "help" | "--help" | "-h" => println!(
            r#"LAPFS 0.3.0-beta.2 — APFS 읽기 및 영구 버퍼 기반 쓰기 마운트

읽기: TARGET은 이미지 또는 Linux APFS 파티션(/dev/sda2 등)
inspect TARGET OFFSET_BYTES
ls TARGET OFFSET_BYTES APFS_PATH [VOLUME_INDEX]
read TARGET OFFSET_BYTES APFS_PATH [VOLUME_INDEX]
digest TARGET OFFSET_BYTES APFS_PATH [VOLUME_INDEX]
export TARGET OFFSET_BYTES APFS_PATH LOCAL_DESTINATION [VOLUME_INDEX]
mount-ro TARGET OFFSET_BYTES EMPTY_MOUNTPOINT [VOLUME_INDEX]
mount-rw TARGET OFFSET_BYTES EMPTY_MOUNTPOINT SESSION_DIR
mount-recover SESSION_DIR

배치 쓰기: TARGET은 분리된 이미지 또는 명시적으로 등록한 장치 설명 파일
prepare TARGET OFFSET_BYTES BATCH_JSON NEW_JOURNAL_DIR [CAP_MIB=128] [RESERVE_MIB=1024]
apply JOURNAL_DIR
recover JOURNAL_DIR
status JOURNAL_DIR
cleanup JOURNAL_DIR
import TARGET OFFSET_BYTES SOURCE APFS_PATH NEW_JOB_DIR
import-plan TARGET OFFSET_BYTES SOURCE APFS_PATH NEW_JOB_DIR
resume JOB_DIR [MAX_CHUNKS=0]

Linux 장치 등록 (sudo 필요, 실제 장치에는 쓰지 않음):
device-enroll /dev/disk/by-id/DEVICE-partN EXPECTED_CONTAINER_UUID
등록 결과의 target 경로를 배치 쓰기에 사용. 저널/job도 같은 등록 폴더 아래에 생성.
APFS 파티션은 OFFSET_BYTES=0. 전체 GPT 이미지는 APFS 파티션의 바이트 오프셋 지정.
mount-ro는 전경 실행; 해제는 fusermount3 -u MOUNTPOINT. mount-rw는 영구 대기열에 모아서 쓰는 제한된 베타. fsync/close/4MiB 시 APFS 반영.
암호화 읽기 및 압축 파일 스트리밍은 미지원. 쓰기는 단일 볼륨/스냅샷 없음 등 조건 검사.
import는 4 MiB 단위로 기록·검증·복구하며 기존 목적 파일을 덮어쓰지 않음.
PREPARED는 반영 완료가 아님. COMMITTED 이후에만 해당 배치 반영 완료.
베타: 실제 USB 전원 차단 내구성은 아직 인증되지 않음. 오류 시 복구 기록 보존.
"#
        ),
        "--version" | "version" => println!("LAPFS {}", env!("CARGO_PKG_VERSION")),
        "device-enroll" => {
            #[cfg(target_os = "linux")]
            println!(
                "{}",
                spark_apfs_safe::physical::enroll(Path::new(arg(2)?), arg(3)?)?
            );
            #[cfg(not(target_os = "linux"))]
            bail!("Device enrollment is Linux-only");
        }
        "ls" => println!(
            "{}",
            serde_json::to_string_pretty(&spark_apfs_safe::reader::list(
                Path::new(arg(2)?),
                arg(3)?.parse()?,
                arg(4)?,
                a.get(5).map(|v| v.parse()).transpose()?
            )?)?
        ),
        "export" => println!(
            "{}",
            spark_apfs_safe::reader::export(
                Path::new(arg(2)?),
                arg(3)?.parse()?,
                arg(4)?,
                Path::new(arg(5)?),
                a.get(6).map(|v| v.parse()).transpose()?
            )?
        ),
        "mount-recover" => println!(
            "{}",
            spark_apfs_safe::buffered::recover(Path::new(arg(2)?))?
        ),
        "mount-rw" => {
            #[cfg(target_os = "linux")]
            spark_apfs_safe::mount_rw::mount(
                Path::new(arg(2)?),
                arg(3)?.parse()?,
                Path::new(arg(4)?),
                Path::new(arg(5)?),
            )?;
            #[cfg(not(target_os = "linux"))]
            bail!("mount-rw requires the Linux build");
        }
        "mount-ro" => {
            #[cfg(target_os = "linux")]
            spark_apfs_safe::mount::mount(
                Path::new(arg(2)?),
                arg(3)?.parse()?,
                Path::new(arg(4)?),
                a.get(5).map(|v| v.parse()).transpose()?,
            )?;
            #[cfg(not(target_os = "linux"))]
            bail!("mount-ro is provided by the Linux ARM64 build; on macOS use the native APFS driver");
        }
        "inspect" => println!(
            "{}",
            serde_json::to_string_pretty(&apfs_batch::inspect(
                Path::new(arg(2)?),
                arg(3)?.parse()?
            )?)?
        ),
        "prepare" => {
            let batch_path = Path::new(arg(4)?);
            anyhow::ensure!(
                batch_path.metadata()?.len() <= 1024 * 1024,
                "Batch description too large"
            );
            let batch =
                serde_json::from_slice::<Vec<apfs_batch::Action>>(&std::fs::read(batch_path)?)?;
            let mib = |n: usize, default: u64| -> Result<u64> {
                match a.get(n) {
                    Some(v) => v
                        .parse::<u64>()?
                        .checked_mul(1024 * 1024)
                        .context("Size overflow"),
                    None => Ok(default),
                }
            };
            let p = apfs_batch::prepare(
                Path::new(arg(2)?),
                Path::new(arg(5)?),
                arg(3)?.parse()?,
                &batch,
                mib(6, DEFAULT_CAP)?,
                mib(7, DEFAULT_RESERVE)?,
            )?;
            println!(
                "PREPARED: 원본 무변경. 복구 데이터가 준비됐습니다. {}",
                p.display()
            );
        }
        "status" => {
            let p = Path::new(arg(2)?);
            if p.join("receipt.json").exists() {
                println!("{}", read_receipt(p)?);
            } else {
                let j = Journal::open(p)?;
                println!(
                    "{}",
                    serde_json::json!({"state": j.state, "image": j.manifest.identity.path, "journal_data_bytes": j.manifest.undo_len+j.manifest.redo_len, "cap_bytes": j.manifest.cap, "operations": j.manifest.ops.len()})
                );
            }
        }
        "apply" | "recover" => {
            let mut j = Journal::open(Path::new(arg(2)?))?;
            let mut image = Image::open(&j.manifest.identity.path, true)?;
            image.check_identity(&j.manifest.identity, a[1] == "apply")?;
            if j.state == State::Prepared && a[1] == "apply" && image.check_journal(&j.dir).is_err()
            {
                image.bind_journal(&j.dir)?;
            }
            if !matches!(j.state, State::RolledBack | State::Prepared) || a[1] == "apply" {
                image.check_journal(&j.dir)?;
            }
            if a[1] == "apply" {
                j.apply(&mut image)?;
            } else {
                j.recover(&mut image)?;
            }
            image.release_journal(&j.dir)?;
            match j.state { State::Committed => println!("COMMITTED: 대상 반영·flush·블록 재검증 완료. USB 전원 차단 안전성 인증을 뜻하지 않습니다."), _ => println!("{:?}: 복구 상태 확인 완료.", j.state) }
        }
        "cleanup" => {
            if Path::new(arg(2)?).join("receipt.json").exists() {
                resume_cleanup(Path::new(arg(2)?))?;
                println!("완료 기록 정리를 재개했습니다.");
                return Ok(());
            }
            let mut j = Journal::open(Path::new(arg(2)?))?;
            anyhow::ensure!(
                matches!(j.state, State::Committed | State::RolledBack),
                "Transaction is not terminal"
            );
            let image = Image::open(&j.manifest.identity.path, false)?;
            image.check_identity(&j.manifest.identity, false)?;
            image.release_journal(&j.dir)?;
            j.cleanup()?;
            println!("완료 영수증을 남기고 복구 데이터 공간을 반환했습니다.");
        }
        "import" | "import-plan" => {
            spark_apfs_safe::transfer::create(
                Path::new(arg(2)?),
                arg(3)?.parse()?,
                Path::new(arg(4)?),
                arg(5)?,
                Path::new(arg(6)?),
                16 * 1024 * 1024,
                DEFAULT_RESERVE,
            )?;
            if a[1] == "import" {
                println!(
                    "{}",
                    spark_apfs_safe::transfer::resume(Path::new(arg(6)?), 0)?
                );
            } else {
                println!("Import plan stored; resume JOB_DIR to begin");
            }
        }
        "resume" => println!(
            "{}",
            spark_apfs_safe::transfer::resume(
                Path::new(arg(2)?),
                a.get(3).map(|v| v.parse()).transpose()?.unwrap_or(0)
            )?
        ),
        "digest" => {
            let (size, sha) = spark_apfs_safe::reader::copy(
                &mut spark_apfs_safe::reader::open(
                    Path::new(arg(2)?),
                    arg(3)?.parse()?,
                    a.get(5).map(|v| v.parse()).transpose()?,
                )?,
                arg(4)?,
                &mut std::io::sink(),
            )?;
            println!("{}", serde_json::json!({"bytes":size,"sha256":sha}));
        }
        "read" => {
            spark_apfs_safe::reader::copy(
                &mut spark_apfs_safe::reader::open(
                    Path::new(arg(2)?),
                    arg(3)?.parse()?,
                    a.get(5).map(|v| v.parse()).transpose()?,
                )?,
                arg(4)?,
                &mut std::io::stdout().lock(),
            )?;
        }
        _ => bail!("Unknown command; run help"),
    }
    Ok(())
}

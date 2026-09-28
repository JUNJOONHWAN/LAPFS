use spark_apfs_safe::buffered::handoff_ready;
use spark_apfs_safe::{
    buffered::{Session, GROUP_BYTES},
    journal::hash,
};
use std::path::PathBuf;
fn fixture() -> (PathBuf, u64, PathBuf) {
    let source = PathBuf::from(std::env::var("SPARK_APFS_TEST_IMAGE").unwrap());
    let offset = std::env::var("SPARK_APFS_TEST_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let out = PathBuf::from(std::env::var("SPARK_APFS_TEST_OUTPUT").unwrap());
    (source, offset, out)
}
#[test]
#[ignore = "requires explicit disposable APFS fixture"]
fn handoff_refuses_live_writer_then_recovers_and_reopens() {
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new()
        .prefix("handoff-")
        .tempdir_in(out)
        .unwrap()
        .keep();
    let image = work.join("native.dmg");
    let other = work.join("wrong.dmg");
    std::fs::copy(&source, &image).unwrap();
    std::fs::copy(&source, &other).unwrap();
    let dir = work.join("session");
    let mut session = Session::start(&image, offset, &dir, GROUP_BYTES, 0).unwrap();
    session.create("/handoff-test.txt").unwrap();
    session
        .write("/handoff-test.txt", 0, b"cross-os-handoff")
        .unwrap();
    assert!(handoff_ready(&dir, &image, offset).is_err());
    drop(session);
    assert!(handoff_ready(&dir, &other, offset).is_err());
    let receipt = handoff_ready(&dir, &image, offset).unwrap();
    assert_eq!(receipt["status"], "ready_to_disconnect");
    assert_eq!(receipt["session"]["pending_bytes"], 0);
    assert_eq!(receipt["session"]["closed"], true);
    assert_eq!(receipt["external_owner"], "absent");
    let mut bytes = Vec::new();
    let (_, digest) = spark_apfs_safe::reader::copy(
        &mut spark_apfs_safe::reader::open(&image, offset, None).unwrap(),
        "/handoff-test.txt",
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, b"cross-os-handoff");
    assert_eq!(digest, hash(b"cross-os-handoff"));
    assert_eq!(
        handoff_ready(&dir, &image, offset).unwrap()["status"],
        "ready_to_disconnect"
    );
    std::fs::write(
        work.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("HANDOFF_NATIVE_EVIDENCE={}", work.display());
}
#[test]
#[ignore = "requires explicit disposable APFS fixture"]
fn buffered_range_queue_roundtrip() {
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new()
        .prefix("buffered-roundtrip-")
        .tempdir_in(out)
        .unwrap()
        .keep();
    let image = work.join("native.dmg");
    std::fs::copy(source, &image).unwrap();
    let mut s = Session::start(&image, offset, &work.join("session"), GROUP_BYTES, 0).unwrap();
    s.create("/buffered.bin").unwrap();
    let mut expected = vec![];
    for n in 0..83 {
        let data: Vec<u8> = (0..131071).map(|i| ((i + n * 17) % 251) as u8).collect();
        let off = expected.len() as u64;
        s.write("/buffered.bin", off, &data).unwrap();
        expected.extend(data);
        assert_eq!(s.attr("/buffered.bin").unwrap().size, expected.len() as u64);
        assert_eq!(
            s.read("/buffered.bin", off, 131071).unwrap(),
            expected[off as usize..]
        );
    }
    assert!(s.pending_bytes() < GROUP_BYTES);
    s.flush().unwrap();
    for (off, len) in [(13, 9011), (4091, 131100), (expected.len() - 123, 5001)] {
        let data = vec![0xA6; len];
        s.write("/buffered.bin", off as u64, &data).unwrap();
        if expected.len() < off + len {
            expected.resize(off + len, 0);
        }
        expected[off..off + len].copy_from_slice(&data);
    }
    s.flush().unwrap();
    for start in (0..expected.len()).step_by(65536) {
        let end = (start + 65536).min(expected.len());
        assert_eq!(
            s.read("/buffered.bin", start as u64, end - start).unwrap(),
            expected[start..end]
        );
    }
    s.create("/small.txt").unwrap();
    s.write("/small.txt", 0, b"uncommitted durable queue")
        .unwrap();
    assert!(spark_apfs_safe::reader::open(&image, offset, None).is_err());
    drop(s);
    let mut s = Session::resume(&work.join("session")).unwrap();
    assert_eq!(
        s.read("/small.txt", 0, 999).unwrap(),
        b"uncommitted durable queue"
    );
    s.flush().unwrap();
    s.truncate("/small.txt", 3).unwrap();
    s.write("/small.txt", 3, b"aligned? no").unwrap();
    s.flush().unwrap();
    s.rename("/small.txt", "/renamed.txt", false).unwrap();
    s.create("/replace.txt").unwrap();
    s.write("/replace.txt", 0, b"old").unwrap();
    s.flush().unwrap();
    s.rename("/renamed.txt", "/replace.txt", true).unwrap();
    assert_eq!(s.read("/replace.txt", 0, 100).unwrap(), b"uncaligned? no");
    s.create("/delete.txt").unwrap();
    s.unlink("/delete.txt").unwrap();
    s.mkdir("/rw-dir").unwrap();
    s.mkdir("/rw-dir/nested").unwrap();
    assert!(s.rmdir("/rw-dir").is_err());
    s.rmdir("/rw-dir/nested").unwrap();
    s.rmdir("/rw-dir").unwrap();
    assert!(s.attr("/rw-dir").is_err());
    assert!(s
        .write("/buffered.bin", expected.len() as u64 + 100, b"hole")
        .is_err());
    s.close().unwrap();
    drop(s);
    std::fs::write(work.join("expected.json"),serde_json::to_vec_pretty(&serde_json::json!({"buffered.bin":{"bytes":expected.len(),"sha256":hash(&expected)},"replace.txt":{"bytes":b"uncaligned? no".len(),"sha256":hash(b"uncaligned? no")}})).unwrap()).unwrap();
    println!("BUFFERED_NATIVE_EVIDENCE={}", work.display());
}
#[test]
#[ignore = "process helper; requires supplied fixture"]
fn buffered_crash_worker() {
    let image = PathBuf::from(std::env::var("LAPFS_WORKER_IMAGE").unwrap());
    let dir = PathBuf::from(std::env::var("LAPFS_WORKER_SESSION").unwrap());
    let offset = std::env::var("SPARK_APFS_TEST_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let mut s = Session::start(&image, offset, &dir, GROUP_BYTES, 0).unwrap();
    s.write("/crash.txt", 13, b"ACKNOWLEDGED-RANGE").unwrap();
    s.flush().unwrap();
    s.close().unwrap();
}
#[test]
#[ignore = "requires fault-injection build and disposable fixture"]
fn buffered_crash_boundaries() {
    assert!(cfg!(feature = "fault-injection"));
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new()
        .prefix("buffered-crash-")
        .tempdir_in(out)
        .unwrap()
        .keep();
    let base = work.join("base.dmg");
    std::fs::copy(source, &base).unwrap();
    let mut s = Session::start(&base, offset, &work.join("init"), GROUP_BYTES, 0).unwrap();
    s.create("/crash.txt").unwrap();
    s.write("/crash.txt", 0, &vec![7; 8197]).unwrap();
    s.close().unwrap();
    drop(s);
    let points = [
        "mount-wal-published",
        "prepare-dir-created",
        "state-Applying",
        "apply-write",
        "state-Committed",
        "mount-queue-retired",
        "mount-gc-entry",
        "mount-closed",
    ];
    let mut results = vec![];
    for point in points {
        let image = work.join(format!("{point}.dmg"));
        std::fs::copy(&base, &image).unwrap();
        let session = work.join(point);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "buffered_crash_worker",
                "--nocapture",
            ])
            .env("LAPFS_WORKER_IMAGE", &image)
            .env("LAPFS_WORKER_SESSION", &session)
            .env("SPARK_APFS_KILL_AT", point)
            .status()
            .unwrap();
        assert!(!status.success(), "fault point was not reached: {point}");
        spark_apfs_safe::buffered::recover(&session).unwrap();
        let actual = spark_apfs_safe::apfs_batch::read_file(&image, offset, "/crash.txt").unwrap();
        let mut expected = vec![7; 8197];
        expected[13..13 + b"ACKNOWLEDGED-RANGE".len()].copy_from_slice(b"ACKNOWLEDGED-RANGE");
        assert_eq!(actual, expected);
        results.push(serde_json::json!({"point":point,"image":image,"sha256":hash(&actual),"bytes":actual.len()}));
    }
    std::fs::write(
        work.join("results.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    println!("BUFFERED_CRASH_EVIDENCE={}", work.display());
}
#[test]
#[ignore = "requires disposable fixture"]
fn buffered_guards_keep_target_unchanged() {
    let (source, offset, out) = fixture();
    let tmp = tempfile::tempdir_in(out).unwrap();
    let image = tmp.path().join("guards.dmg");
    std::fs::copy(&source, &image).unwrap();
    assert!(Session::start(&image, offset, &tmp.path().join("no-space"), 4096, u64::MAX).is_err());
    assert!(!tmp.path().join("no-space").exists());
    let dir = tmp.path().join("session");
    let mut s = Session::start(&image, offset, &dir, 4096, 0).unwrap();
    s.create("/guard.txt").unwrap();
    assert!(s.write("/guard.txt", 0, &vec![0; 4097]).is_err());
    assert!(s.mkdir("/../bad").is_err());
    assert!(Session::start(&image, offset, &tmp.path().join("second"), 4096, 0).is_err());
    s.write("/guard.txt", 0, b"durably queued").unwrap();
    let before = hash(&std::fs::read(&image).unwrap());
    drop(s);
    let other = tmp.path().join("other.dmg");
    std::fs::copy(&source, &other).unwrap();
    assert!(Session::resume_target(&dir, &other, offset).is_err());
    assert_eq!(hash(&std::fs::read(&image).unwrap()), before);
    let payload = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("payload-")
        })
        .unwrap();
    let original = std::fs::read(&payload).unwrap();
    std::fs::write(&payload, vec![0; original.len()]).unwrap();
    assert!(Session::resume(&dir).is_err());
    assert_eq!(hash(&std::fs::read(&image).unwrap()), before);
    std::fs::write(payload, original).unwrap();
    let mut s = Session::resume(&dir).unwrap();
    assert_eq!(s.read("/guard.txt", 0, 100).unwrap(), b"durably queued");
    s.close().unwrap();
    drop(s);
    assert_eq!(
        spark_apfs_safe::apfs_batch::read_file(&image, offset, "/guard.txt").unwrap(),
        b"durably queued"
    );
}

#[test]
#[ignore = "process helper; requires supplied fixture"]
fn buffered_prefix_crash_worker() {
    let image = PathBuf::from(std::env::var("LAPFS_WORKER_IMAGE").unwrap());
    let dir = PathBuf::from(std::env::var("LAPFS_WORKER_SESSION").unwrap());
    let offset = std::env::var("SPARK_APFS_TEST_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let mut s = Session::start(&image, offset, &dir, GROUP_BYTES, 0).unwrap();
    s.write("/crash.txt", 13, b"FIRST-GROUP").unwrap();
    s.write("/crash.txt", 2001, b"SECOND-GROUP").unwrap();
    s.close().unwrap();
}
#[test]
#[ignore = "requires fault-injection build and disposable fixture"]
fn buffered_prefix_retirement_crash_boundaries() {
    assert!(cfg!(feature = "fault-injection"));
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new()
        .prefix("prefix-crash-")
        .tempdir_in(out)
        .unwrap()
        .keep();
    let base = work.join("base.dmg");
    std::fs::copy(source, &base).unwrap();
    let mut s = Session::start(&base, offset, &work.join("init"), GROUP_BYTES, 0).unwrap();
    s.create("/crash.txt").unwrap();
    s.write("/crash.txt", 0, &vec![7; 8197]).unwrap();
    s.close().unwrap();
    drop(s);
    let mut results = vec![];
    for point in [
        "prepare-dir-created",
        "state-Applying",
        "apply-write",
        "state-Committed",
        "mount-queue-retired",
        "mount-gc-entry",
        "mount-closed",
    ] {
        let image = work.join(format!("{point}.dmg"));
        std::fs::copy(&base, &image).unwrap();
        let session = work.join(point);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "buffered_prefix_crash_worker",
                "--nocapture",
            ])
            .env("LAPFS_WORKER_IMAGE", &image)
            .env("LAPFS_WORKER_SESSION", &session)
            .env("SPARK_APFS_KILL_AT", point)
            .status()
            .unwrap();
        assert!(!status.success(), "fault point not reached: {point}");
        spark_apfs_safe::buffered::recover(&session).unwrap();
        let actual = spark_apfs_safe::apfs_batch::read_file(&image, offset, "/crash.txt").unwrap();
        let mut expected = vec![7; 8197];
        expected[13..24].copy_from_slice(b"FIRST-GROUP");
        expected[2001..2013].copy_from_slice(b"SECOND-GROUP");
        assert_eq!(actual, expected);
        results.push(serde_json::json!({"point":point,"image":image,"sha256":hash(&actual),"bytes":actual.len()}));
    }
    std::fs::write(
        work.join("results.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
    println!("PREFIX_CRASH_EVIDENCE={}", work.display());
}

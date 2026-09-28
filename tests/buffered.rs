use spark_apfs_safe::buffered::handoff_ready;
use spark_apfs_safe::{
    buffered::{Session, WritePolicy, GROUP_BYTES},
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
fn cross_directory_move_preserves_data_and_inode() {
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new().prefix("cross-move-").tempdir_in(out).unwrap().keep();
    let image = work.join("native.dmg");
    std::fs::copy(source, &image).unwrap();
    let mut s = Session::start(&image, offset, &work.join("session"), GROUP_BYTES, 0).unwrap();
    s.mkdir("/move-from").unwrap();
    s.mkdir("/move-to").unwrap();
    s.create("/move-from/data.bin").unwrap();
    let bytes: Vec<u8> = (0..262_144).map(|n| (n % 251) as u8).collect();
    s.write("/move-from/data.bin", 0, &bytes).unwrap();
    s.flush().unwrap();
    let inode = s.attr("/move-from/data.bin").unwrap().inode;
    s.rename("/move-from/data.bin", "/move-to/data.bin", false).unwrap();
    assert!(s.attr("/move-from/data.bin").is_err());
    assert_eq!(s.attr("/move-to/data.bin").unwrap().inode, inode);
    assert_eq!(s.read("/move-to/data.bin", 0, bytes.len()).unwrap(), bytes);
    s.mkdir("/move-from/tree").unwrap();
    s.create("/move-from/tree/child").unwrap();
    s.write("/move-from/tree/child", 0, b"child data").unwrap();
    s.flush().unwrap();
    let child_ino = s.attr("/move-from/tree/child").unwrap().inode;
    s.rename("/move-from/tree", "/move-to/tree", false).unwrap();
    assert_eq!(s.attr("/move-to/tree/child").unwrap().inode, child_ino);
    assert_eq!(s.read("/move-to/tree/child", 0, 32).unwrap(), b"child data");
    assert!(s.rename("/move-to/tree", "/move-to/tree/child/loop", false).is_err());
    s.create("/move-from/replace-src").unwrap();
    s.write("/move-from/replace-src", 0, b"new payload").unwrap();
    s.create("/move-to/replace-dst").unwrap();
    s.write("/move-to/replace-dst", 0, b"old payload").unwrap();
    s.flush().unwrap();
    s.rename("/move-from/replace-src", "/move-to/replace-dst", true).unwrap();
    assert_eq!(s.read("/move-to/replace-dst", 0, 32).unwrap(), b"new payload");
    s.mkdir("/move-from/empty-dir").unwrap();
    s.mkdir("/move-to/empty-dir").unwrap();
    s.rename("/move-from/empty-dir", "/move-to/empty-dir", true).unwrap();
    assert!(s.attr("/move-from/empty-dir").is_err());
    assert!(s.attr("/move-to/empty-dir").unwrap().is_dir);
    assert!(s.rename("/move-to/tree", "/move-to/replace-dst", true).is_err());
    assert_eq!(s.read("/move-to/replace-dst", 0, 32).unwrap(), b"new payload");
    s.close().unwrap();
    drop(s);
    assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image, offset, "/move-to/data.bin").unwrap(), bytes);
    assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image, offset, "/move-to/tree/child").unwrap(), b"child data");
    std::fs::write(work.join("expected.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "image": image,
        "data_sha256": hash(&bytes),
        "data_inode": inode,
        "child_inode": child_ino
    })).unwrap()).unwrap();
    println!("CROSS_MOVE_NATIVE_EVIDENCE={}", work.display());
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
#[ignore = "process helper; requires supplied fixture"]
fn cross_directory_move_crash_worker() {
    let image = PathBuf::from(std::env::var("LAPFS_WORKER_IMAGE").unwrap());
    let dir = PathBuf::from(std::env::var("LAPFS_WORKER_SESSION").unwrap());
    let offset = std::env::var("SPARK_APFS_TEST_OFFSET").unwrap().parse().unwrap();
    let mut s = Session::start(&image, offset, &dir, GROUP_BYTES, 0).unwrap();
    s.rename("/move-source/data", "/move-target/data", true).unwrap();
    s.close().unwrap();
}

#[test]
#[ignore = "requires fault-injection build and disposable APFS fixture"]
fn cross_directory_replace_recovers_at_commit_boundaries() {
    assert!(cfg!(feature = "fault-injection"));
    use std::os::unix::process::ExitStatusExt;
    let (source, offset, out) = fixture();
    let work = tempfile::Builder::new().prefix("cross-move-crash-").tempdir_in(out).unwrap().keep();
    let base = work.join("base.dmg");
    std::fs::copy(source, &base).unwrap();
    let mut s = Session::start(&base, offset, &work.join("init"), GROUP_BYTES, 0).unwrap();
    s.mkdir("/move-source").unwrap();
    s.mkdir("/move-target").unwrap();
    s.create("/move-source/data").unwrap();
    s.write("/move-source/data", 0, b"new content").unwrap();
    s.create("/move-target/data").unwrap();
    s.write("/move-target/data", 0, b"old content").unwrap();
    s.close().unwrap();drop(s);
    for point in ["state-Applying", "apply-write", "state-Committed"] {
        let image=work.join(format!("{point}.dmg"));std::fs::copy(&base,&image).unwrap();
        let dir=work.join(point);
        let status=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored","--exact","cross_directory_move_crash_worker","--nocapture"])
            .env("LAPFS_WORKER_IMAGE",&image).env("LAPFS_WORKER_SESSION",&dir)
            .env("SPARK_APFS_KILL_AT",point).status().unwrap();
        assert_eq!(status.signal(),Some(libc::SIGKILL),"fault not reached: {point}");
        spark_apfs_safe::buffered::recover(&dir).unwrap();
        assert!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/move-source/data").is_err());
        assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/move-target/data").unwrap(),b"new content");
    }
    println!("CROSS_MOVE_CRASH_EVIDENCE={}",work.display());
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
    let policy=if std::env::var("LAPFS_WORKER_GROUPED").as_deref()==Ok("1") { WritePolicy::Grouped } else { WritePolicy::Durable };
    let mut s = Session::start_with_policy(&image, offset, &dir, GROUP_BYTES, 0, policy).unwrap();
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
        "mount-stream-written",
        "mount-stream-synced",
        "mount-stream-flushed",
        "mount-stream-frozen",
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
    for grouped in [false,true] {
    for point in points {
        if grouped && point=="mount-stream-synced" {continue;}
        let label=format!("{}-{point}",if grouped {"grouped"} else {"durable"});
        let image = work.join(format!("{label}.dmg"));
        std::fs::copy(&base, &image).unwrap();
        let session = work.join(&label);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "buffered_crash_worker",
                "--nocapture",
            ])
            .env("LAPFS_WORKER_GROUPED",if grouped {"1"} else {"0"})
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
        if grouped && point == "mount-wal-published" { expected = vec![7; 8197]; }
        assert_eq!(actual, expected);
        results.push(serde_json::json!({"point":label,"write_policy":if grouped {"grouped"} else {"durable"},"image":image,"sha256":hash(&actual),"bytes":actual.len()}));
    }
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
                .starts_with("stream-")
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

#[test]
#[ignore = "requires disposable fixture"]
fn stream_small_writes_recovery_and_shared_payload_retirement() {
    stream_recovery_case(WritePolicy::Durable);
    stream_recovery_case(WritePolicy::Grouped);
}
fn stream_recovery_case(policy: WritePolicy) {
    let (source, offset, out) = fixture();
    let work = tempfile::tempdir_in(out).unwrap();
    let image = work.path().join("native.dmg");
    std::fs::copy(source, &image).unwrap();
    let dir = work.path().join("session");
    let mut s = Session::start_with_policy(&image, offset, &dir, GROUP_BYTES, 0, policy).unwrap();
    s.create("/stream-a").unwrap(); s.create("/stream-b").unwrap();
    let before = s.status()["committed_batches"].as_u64().unwrap();
    let mut expected = Vec::new();
    for i in 0..400 {
        let data=vec![(i%251) as u8; 10240];
        s.write("/stream-a", expected.len() as u64, &data).unwrap(); expected.extend(data);
    }
    assert_eq!(s.status()["committed_batches"].as_u64().unwrap(),before);
    // Non-adjacent writes share a stream: retiring the first range must not
    // delete the backing file needed by the next transaction.
    s.write("/stream-b",0,b"second-file").unwrap();
    s.write("/stream-a",17,b"overlap").unwrap(); expected[17..24].copy_from_slice(b"overlap");
    if policy == WritePolicy::Grouped { s.flush().unwrap(); }
    drop(s);
    let mut s=Session::resume(&dir).unwrap();
    assert_eq!(s.read("/stream-a",0,expected.len()).unwrap(),expected);
    // Append after recovery must drain the frozen prior queue first.
    s.write("/stream-b",11,b"-resumed").unwrap();
    s.close().unwrap();drop(s);
    assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/stream-a").unwrap(),expected);
    assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/stream-b").unwrap(),b"second-file-resumed");
}

#[test]
#[ignore = "requires disposable fixture"]
fn stream_torn_tail_and_corruption() {
    let (source, offset, out)=fixture();
    // Header/data/digest cut positions, followed by intact and corrupt frames.
    for case in 0..24 {
        let mode=case%12; let policy=if case<12 {WritePolicy::Durable} else {WritePolicy::Grouped};
        let work=tempfile::tempdir_in(&out).unwrap();let image=work.path().join("native.dmg");
        std::fs::copy(&source,&image).unwrap();let dir=work.path().join("session");
        let mut s=Session::start_with_policy(&image,offset,&dir,GROUP_BYTES,0,WritePolicy::Durable).unwrap();
        s.create("/stream-test").unwrap();s.write("/stream-test",0,b"durable-prefix").unwrap();
        let path=std::fs::read_dir(&dir).unwrap().map(|e|e.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with("stream-")).unwrap();
        let first=std::fs::metadata(&path).unwrap().len() as usize;
        s.write("/stream-test",14,b"tail-payload").unwrap(); drop(s);
        if policy == WritePolicy::Grouped {
            let manifest=dir.join("session.json");
            let mut env:serde_json::Value=serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
            let payload=env["payload"].as_str().unwrap().replace("\"write_policy\":\"durable\"", "\"write_policy\":\"grouped\"");
            env["sha256"]=hash(payload.as_bytes()).into();env["payload"]=payload.into();
            std::fs::write(manifest,serde_json::to_vec(&env).unwrap()).unwrap();
        }
        let mut bytes=std::fs::read(&path).unwrap();let len=bytes.len();
        if mode<8 {
            let keep=[0,1,7,8,71,72,(len-first)/2,len-first-1][mode];
            bytes.truncate(first+keep);
        } else if mode>8 {
            let ix=match mode {9=>first,10=>first+73,_=>len-1};bytes[ix]^=1;
        }
        std::fs::write(&path,&bytes).unwrap();
        let before=hash(&std::fs::read(&image).unwrap());
        let result=Session::resume(&dir);
        if mode>8 { assert!(result.is_err());assert_eq!(hash(&std::fs::read(&image).unwrap()),before);continue; }
        let mut s=result.unwrap();
        let expected=if mode==8 {b"durable-prefixtail-payload".as_slice()} else {b"durable-prefix".as_slice()};
        assert_eq!(s.read("/stream-test",0,100).unwrap(),expected);
        s.close().unwrap();
    }
}

#[test]
#[ignore = "requires explicit disposable APFS fixture"]
fn read_ahead_invalidation_and_boundaries() {
    let (source, offset, out) = fixture();
    let work = tempfile::tempdir_in(out).unwrap();
    let image = work.path().join("native.dmg"); std::fs::copy(source, &image).unwrap();
    let mut s = Session::start_with_policy(&image, offset, &work.path().join("session"), GROUP_BYTES, 0, WritePolicy::Grouped).unwrap();
    s.create("/cache-test").unwrap();
    let mut bytes = vec![41u8; 9*1024*1024+17];
    s.write("/cache-test",0,&bytes).unwrap(); s.flush().unwrap();
    assert_eq!(s.read("/cache-test",0,37).unwrap(),bytes[..37]);
    let boundary = 8*1024*1024-11;
    assert_eq!(s.read("/cache-test",boundary as u64,47).unwrap(),bytes[boundary..boundary+47]);
    s.write("/cache-test",17,b"changed").unwrap(); bytes[17..24].copy_from_slice(b"changed");
    assert_eq!(s.read("/cache-test",0,40).unwrap(),bytes[..40]);
    s.flush().unwrap(); assert_eq!(s.read("/cache-test",0,40).unwrap(),bytes[..40]);
    s.write("/cache-test",bytes.len() as u64,b"tail").unwrap();
    assert_eq!(s.read("/cache-test",bytes.len() as u64,20).unwrap(),b"tail");
    s.truncate("/cache-test",31).unwrap(); assert_eq!(s.read("/cache-test",0,100).unwrap(),bytes[..31]);
    s.rename("/cache-test","/renamed-cache",false).unwrap();
    assert!(s.read("/cache-test",0,10).is_err());
    assert_eq!(s.read("/renamed-cache",0,100).unwrap(),bytes[..31]);
    s.unlink("/renamed-cache").unwrap(); s.create("/renamed-cache").unwrap();
    s.write("/renamed-cache",0,b"replacement").unwrap();
    assert_eq!(s.read("/renamed-cache",0,100).unwrap(),b"replacement");
    s.close().unwrap();
}

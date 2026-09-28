#![cfg(feature="fault-injection")]
use spark_apfs_safe::{buffered::{Session,WritePolicy},pipeline::Pipeline,journal::hash};
use std::{path::PathBuf,sync::mpsc,time::Duration};
use std::os::unix::process::ExitStatusExt;
const GROUP:u64=4*1024*1024;
fn fixture()->(PathBuf,u64,PathBuf) {
 (std::env::var("SPARK_APFS_TEST_IMAGE").unwrap().into(),std::env::var("SPARK_APFS_TEST_OFFSET").unwrap().parse().unwrap(),std::env::var("SPARK_APFS_TEST_OUTPUT").unwrap().into())
}
#[test]
#[ignore="explicit disposable fixture"]
fn input_overlaps_storage_and_fsync_drains_both_buffers() {
 let (source,offset,out)=fixture();let work=tempfile::Builder::new().prefix("pipeline-overlap-").tempdir_in(out).unwrap().keep();let image=work.join("native.dmg");std::fs::copy(source,&image).unwrap();
 let s=Session::start_with_policy(&image,offset,&work.join("session"),GROUP,0,WritePolicy::Grouped).unwrap();let mut p=Pipeline::new(s).unwrap();
 p.create("/pipeline.bin").unwrap();p.writable("/pipeline.bin").unwrap();
 let release=p.test_pause_worker().unwrap();
 let mut expected=vec![0x5a;GROUP as usize];p.write("/pipeline.bin",0,&expected).unwrap();
 let tail=vec![0x3c;2*1024*1024];p.write("/pipeline.bin",GROUP,&tail).unwrap();expected.extend(tail);
 // These calls completed while the storage thread was held at the test gate.
 assert_eq!(p.attr("/pipeline.bin").unwrap().size,expected.len() as u64);
 let (started,entered)=mpsc::channel();let (done,wait)=mpsc::channel();
 let thread=std::thread::spawn(move || {started.send(()).unwrap();p.flush().unwrap();done.send(()).unwrap();p});
 entered.recv().unwrap();assert!(wait.recv_timeout(Duration::from_millis(30)).is_err(),"fsync acknowledged a held storage worker");
 release.send(()).unwrap();wait.recv_timeout(Duration::from_secs(120)).unwrap();let mut p=thread.join().unwrap();
 assert_eq!(p.read("/pipeline.bin",0,expected.len()).unwrap(),expected);
 let before_queries=std::fs::read(&image).unwrap();
 p.write("/pipeline.bin",4091,b"overlap across block").unwrap();expected[4091..4091+20].copy_from_slice(b"overlap across block");
 p.write("/pipeline.bin",73,b"second").unwrap();expected[73..79].copy_from_slice(b"second");
 p.attr("/").unwrap();p.list("/").unwrap();p.space().unwrap();
 assert_eq!(p.read("/pipeline.bin",0,expected.len()).unwrap(),expected);
 assert_eq!(std::fs::read(&image).unwrap(),before_queries,"observers must not commit the volatile input buffer");
 p.rename("/pipeline.bin","/pipeline-renamed.bin",false).unwrap();assert!(p.attr("/pipeline.bin").is_err());
 p.truncate("/pipeline-renamed.bin",5*1024*1024).unwrap();expected.truncate(5*1024*1024);
 p.create("/other.bin").unwrap();p.write("/other.bin",0,b"other file").unwrap();
 p.write("/pipeline-renamed.bin",17,b"new").unwrap();expected[17..20].copy_from_slice(b"new");
 p.close().unwrap();drop(p);
 assert_eq!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/pipeline-renamed.bin").unwrap(),expected);
 std::fs::write(work.join("result.json"),serde_json::to_vec_pretty(&serde_json::json!({"image":image,"expected":{"pipeline-renamed.bin":{"bytes":expected.len(),"sha256":hash(&expected)},"other.bin":{"bytes":10,"sha256":hash(b"other file")}}})).unwrap()).unwrap();
 println!("PIPELINE_OVERLAP_EVIDENCE={}",work.display());
}
#[test]
#[ignore="explicit disposable fixture"]
fn worker_error_is_reported_by_fsync_and_poisoned_ack_path() {
 let(source,offset,out)=fixture();let tmp=tempfile::tempdir_in(out).unwrap();let image=tmp.path().join("native.dmg");std::fs::copy(source,&image).unwrap();let dir=tmp.path().join("session");
 let s=Session::start_with_policy(&image,offset,&dir,GROUP,0,WritePolicy::Grouped).unwrap();let mut p=Pipeline::new(s).unwrap();p.create("/error.bin").unwrap();p.test_fail_next_batch();
 p.write("/error.bin",0,&vec![3;GROUP as usize]).unwrap();
 assert!(p.flush().is_err());assert!(p.write("/error.bin",0,b"must not ACK").is_err());assert!(p.attr("/error.bin").is_err());assert!(p.close().is_err());drop(p);
 spark_apfs_safe::buffered::recover(&dir).unwrap();assert!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/error.bin").unwrap().is_empty());
}
#[test]
#[ignore="child process only"]
fn pipeline_crash_worker() {
 let (_,offset,_)=fixture();let image:PathBuf=std::env::var("LAPFS_WORKER_IMAGE").unwrap().into();let dir:PathBuf=std::env::var("LAPFS_WORKER_SESSION").unwrap().into();
 let s=Session::start_with_policy(&image,offset,&dir,GROUP,0,WritePolicy::Grouped).unwrap();let mut p=Pipeline::new(s).unwrap();
 p.write("/crash.txt",0,&vec![0x5a;GROUP as usize]).unwrap();p.write("/crash.txt",GROUP,&[0x3c;1024]).unwrap();p.close().unwrap();
}
#[test]
#[ignore="explicit disposable fixture and fault injection"]
fn pipeline_process_crashes_preserve_committed_or_recoverable_groups() {
 let(source,offset,out)=fixture();let work=tempfile::Builder::new().prefix("pipeline-crash-").tempdir_in(out).unwrap().keep();let base=work.join("base.dmg");std::fs::copy(source,&base).unwrap();
 let mut s=Session::start_with_policy(&base,offset,&work.join("init"),GROUP,0,WritePolicy::Grouped).unwrap();s.create("/crash.txt").unwrap();s.write("/crash.txt",0,&[7;4096]).unwrap();s.close().unwrap();drop(s);
 let base_bytes=std::fs::read(&base).unwrap();let mut rows=vec![];
 for point in ["pipeline-input-accepted","pipeline-batch-start","group-intent","prepare-dir-created","group-prepared","state-Applying","apply-write","state-Committed","group-retired","pipeline-batch-complete"] {
  let image=work.join(format!("{point}.dmg"));std::fs::copy(&base,&image).unwrap();let dir=work.join(point);
  let status=std::process::Command::new(std::env::current_exe().unwrap()).args(["--ignored","--exact","pipeline_crash_worker","--nocapture"]).env("LAPFS_WORKER_IMAGE",&image).env("LAPFS_WORKER_SESSION",&dir).env("SPARK_APFS_KILL_AT",point).status().unwrap();assert_eq!(status.signal(),Some(libc::SIGKILL),"{point} not reached");
  let before_recovery=work.join(format!("{point}-before-recovery.dmg"));std::fs::copy(&image,&before_recovery).unwrap();
  let mut originally_free=std::collections::BTreeSet::new();
  for e in std::fs::read_dir(&dir).unwrap().flatten() {
   let m=e.path().join("manifest.json");if m.is_file(){let j=spark_apfs_safe::journal::Journal::open(&e.path()).unwrap();originally_free.extend(j.manifest.free_blocks.clone());}
  }
  spark_apfs_safe::buffered::recover(&dir).unwrap();let actual=spark_apfs_safe::apfs_batch::read_file(&image,offset,"/crash.txt").unwrap();let expected=if matches!(point,"state-Committed"|"group-retired"|"pipeline-batch-complete") {vec![0x5a;GROUP as usize]} else {vec![7;4096]};assert_eq!(actual,expected,"{point}");if !matches!(point,"state-Committed"|"group-retired"|"pipeline-batch-complete") {let recovered=std::fs::read(&image).unwrap();for (i,(a,b)) in recovered.chunks(4096).zip(base_bytes.chunks(4096)).enumerate(){if a!=b {assert!(originally_free.contains(&((i*4096) as u64)),"allocated bytes changed after rollback: {point} block {i}");}}}
  rows.push(serde_json::json!({"point":point,"image":image,"before_recovery_image":before_recovery,"bytes":actual.len(),"sha256":hash(&actual)}));
 }
 std::fs::write(work.join("results.json"),serde_json::to_vec_pretty(&rows).unwrap()).unwrap();println!("PIPELINE_CRASH_EVIDENCE={}",work.display());
}

#[test]
#[ignore="explicit disposable fixture"]
fn worker_disconnect_never_acknowledges_more_writes() {
 let(source,offset,out)=fixture();let tmp=tempfile::tempdir_in(out).unwrap();let image=tmp.path().join("native.dmg");std::fs::copy(source,&image).unwrap();let dir=tmp.path().join("session");
 let s=Session::start_with_policy(&image,offset,&dir,GROUP,0,WritePolicy::Grouped).unwrap();let mut p=Pipeline::new(s).unwrap();p.create("/panic.bin").unwrap();p.writable("/panic.bin").unwrap();
 assert!(p.test_panic_worker().is_err());assert!(p.write("/panic.bin",0,b"must not ACK").is_err());assert!(p.flush().is_err());assert!(p.close().is_err());drop(p);
 spark_apfs_safe::buffered::recover(&dir).unwrap();assert!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/panic.bin").unwrap().is_empty());
}

#[test]
#[ignore="explicit disposable fixture"]
fn volatile_read_checksum_failure_stops_later_acknowledgements() {
 let(source,offset,out)=fixture();let tmp=tempfile::tempdir_in(out).unwrap();let image=tmp.path().join("native.dmg");std::fs::copy(source,&image).unwrap();let dir=tmp.path().join("session");
 let s=Session::start_with_policy(&image,offset,&dir,GROUP,0,WritePolicy::Grouped).unwrap();let mut p=Pipeline::new(s).unwrap();p.create("/checksum.bin").unwrap();p.write("/checksum.bin",0,b"volatile data").unwrap();p.test_corrupt_pending();
 assert!(p.read("/checksum.bin",0,13).is_err());assert!(p.write("/checksum.bin",0,b"must not ACK").is_err());assert!(p.flush().is_err());drop(p);
 spark_apfs_safe::buffered::recover(&dir).unwrap();assert!(spark_apfs_safe::apfs_batch::read_file(&image,offset,"/checksum.bin").unwrap().is_empty());
}

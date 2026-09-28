//! Bounded diagnostic log, independent of the durable APFS recovery journal.
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};
const LIMIT: u64 = 2 * 1024 * 1024;
const KEEP: usize = 3;
static LOGGER: OnceLock<Logger> = OnceLock::new();
static WARNED: AtomicBool = AtomicBool::new(false);
pub struct Logger {
    dir: PathBuf,
    limit: u64,
}
fn owned_file(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = f.metadata()?;
    ensure!(
        m.is_file()
            && m.uid() == unsafe { libc::geteuid() }
            && m.nlink() == 1
            && m.mode() & 0o077 == 0,
        "Insecure diagnostic log file"
    );
    Ok(f)
}
impl Logger {
    pub fn open(dir: &Path, limit: u64) -> Result<Self> {
        if !dir.exists() {
            fs::DirBuilder::new().recursive(true).create(dir)?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let m = fs::symlink_metadata(dir)?;
        ensure!(
            m.is_dir()
                && !m.file_type().is_symlink()
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o077 == 0,
            "Diagnostic log directory must be private and owned by this user"
        );
        let logger = Self {
            dir: fs::canonicalize(dir)?,
            limit,
        };
        owned_file(&logger.dir.join("errors.lock"))?;
        owned_file(&logger.dir.join("errors.jsonl"))?;
        Ok(logger)
    }
    fn append(&self, bytes: &[u8]) -> Result<()> {
        // Open a distinct lock FD each time: flock then serializes threads AND processes.
        let lock = owned_file(&self.dir.join("errors.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let active = self.dir.join("errors.jsonl");
        let mut f = owned_file(&active)?;
        if f.metadata()?.len().saturating_add(bytes.len() as u64) > self.limit {
            for i in 1..=KEEP {
                let p = self.dir.join(format!("errors.jsonl.{i}"));
                if p.symlink_metadata().is_ok() {
                    owned_file(&p)?;
                }
            }
            for i in (1..=KEEP).rev() {
                let to = self.dir.join(format!("errors.jsonl.{i}"));
                let from = if i == 1 {
                    active.clone()
                } else {
                    self.dir.join(format!("errors.jsonl.{}", i - 1))
                };
                if from.exists() {
                    fs::rename(from, to)?;
                }
            }
            f = owned_file(&active)?;
            File::open(&self.dir)?.sync_all()?;
        }
        f.write_all(bytes)?;
        f.sync_data()?;
        Ok(())
    }
    pub fn record(
        &self,
        level: &str,
        operation: &str,
        errno: Option<i32>,
        message: &str,
    ) -> Result<()> {
        let mut end = message.len().min(4096);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let event = serde_json::json!({"time_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),"timezone":"UTC","pid":std::process::id(),"level":level,"operation":operation,"errno":errno,"message":&message[..end],"message_truncated":end < message.len()});
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        self.append(&bytes)
    }
}
pub fn init() -> Result<()> {
    let dir = if unsafe { libc::geteuid() } == 0 {
        PathBuf::from("/var/log/lapfs")
    } else {
        PathBuf::from(std::env::var_os("HOME").context("HOME unavailable for diagnostic log")?)
            .join(".local/state/lapfs")
    };
    let logger = Logger::open(&dir, LIMIT)?;
    logger.record(
        "info",
        "process-start",
        None,
        "LAPFS diagnostic logging initialized",
    )?;
    LOGGER
        .set(logger)
        .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    Ok(())
}
pub fn event(level: &str, op: &str, errno: Option<i32>, message: &str) {
    if let Some(l) = LOGGER.get() {
        if let Err(e) = l.record(level, op, errno, message) {
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "LAPFS diagnostic log unavailable; original operation result preserved: {e:#}"
                );
            }
        }
    }
}
pub fn expected_lookup_miss(op: &str, errno: i32) -> bool {
    op == "lookup" && errno == libc::ENOENT
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_and_concurrent_records() {
        let d = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let log = std::sync::Arc::new(Logger::open(d.path(), 2048).unwrap());
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let l = log.clone();
                std::thread::spawn(move || {
                    for _ in 0..40 {
                        l.record("error", "write", Some(libc::ENOSPC), "disk full\n한글")
                            .unwrap();
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        for n in [
            "errors.jsonl",
            "errors.jsonl.1",
            "errors.jsonl.2",
            "errors.jsonl.3",
        ] {
            let b = fs::read(d.path().join(n)).unwrap();
            assert!(b.len() <= 2048);
            for line in b.split(|x| *x == b'\n').filter(|x| !x.is_empty()) {
                let v: serde_json::Value = serde_json::from_slice(line).unwrap();
                assert_eq!(v["operation"], "write");
            }
        }
        assert!(!d.path().join("errors.jsonl.4").exists());
    }
    #[test]
    fn rejects_links_and_preserves_error_classification() {
        let d = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let outside = d.path().join("outside");
        fs::write(&outside, b"preserve").unwrap();
        std::os::unix::fs::symlink(&outside, d.path().join("errors.jsonl")).unwrap();
        assert!(Logger::open(d.path(), LIMIT).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"preserve");
        assert!(expected_lookup_miss("lookup", libc::ENOENT));
        assert!(!expected_lookup_miss("write", libc::ENOENT));
        assert!(!expected_lookup_miss("lookup", libc::EIO));
    }
}

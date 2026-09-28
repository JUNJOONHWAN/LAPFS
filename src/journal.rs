//! Offline image transactions. Physical devices are deliberately rejected.
//! PREPARED is durable before any target write. Incomplete APPLYING/RECOVERING
//! always rolls back; a COMMITTED transaction is never implicitly rolled back.
use anyhow::{bail, ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const BLOCK: usize = 4096;
const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
pub const DEFAULT_CAP: u64 = 128 * 1024 * 1024;
pub const DEFAULT_RESERVE: u64 = 1024 * 1024 * 1024;

/// Test binaries only: kill the process at a durable protocol boundary.
pub(crate) fn fault_point(name: &str) {
    #[cfg(feature = "fault-injection")]
    if std::env::var("SPARK_APFS_KILL_AT").ok().as_deref() == Some(name) {
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
    }
    let _ = name;
}
pub fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

pub trait Device {
    fn len(&self) -> u64;
    fn read(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()>;
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
}

impl<D: Device + ?Sized> Device for &mut D {
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn read(&mut self, o: u64, b: &mut [u8]) -> Result<()> {
        (**self).read(o, b)
    }
    fn write(&mut self, o: u64, b: &[u8]) -> Result<()> {
        (**self).write(o, b)
    }
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub path: PathBuf,
    pub size: u64,
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    pub mtime_ns: i64,
    pub ctime: i64,
    pub ctime_ns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
}

pub struct Image {
    file: File,
    pub identity: Identity,
}
impl Image {
    pub fn open(path: &Path, writable: bool) -> Result<Self> {
        #[cfg(target_os = "linux")]
        if crate::physical::is_descriptor(path) {
            let (file, identity) = crate::physical::open(path, writable)?;
            return Ok(Self { file, identity });
        }
        let path = fs::canonicalize(path)?;
        let meta = fs::metadata(&path)?;
        ensure!(
            meta.is_file(),
            "Only offline regular image files are supported; raw USB writes are locked out"
        );
        ensure!(
            meta.len() > 0 && meta.len() % BLOCK as u64 == 0,
            "Image must have a nonzero 4096-byte-aligned size"
        );
        ensure_not_attached(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(writable)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        lock(&file)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.len() > 0 && meta.len() % BLOCK as u64 == 0,
            "Target changed type/size"
        );
        ensure!(
            meta.nlink() == 1,
            "Hard-linked images are rejected: path-based persistent ownership would be ambiguous"
        );
        let identity = Identity {
            path,
            size: meta.len(),
            dev: meta.dev(),
            ino: meta.ino(),
            mtime: meta.mtime(),
            mtime_ns: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_ns: meta.ctime_nsec(),
            generation: None,
        };
        Ok(Self { file, identity })
    }
    pub fn check_identity(&self, expected: &Identity, pristine: bool) -> Result<()> {
        ensure!(
            self.identity.path == expected.path
                && self.identity.dev == expected.dev
                && self.identity.ino == expected.ino
                && self.identity.size == expected.size,
            "Wrong/replaced target image; refusing writes"
        );
        if pristine {
            ensure!(
                &self.identity == expected,
                "Image changed since preparation; prepare a fresh batch"
            );
        }
        Ok(())
    }
    pub(crate) fn refresh_identity(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.identity.generation.is_some() {
            self.identity = crate::physical::identity_for_fd(&self.identity.path, &self.file)?;
            return Ok(());
        }
        let m = self.file.metadata()?;
        self.identity.mtime = m.mtime();
        self.identity.mtime_ns = m.mtime_nsec();
        self.identity.ctime = m.ctime();
        self.identity.ctime_ns = m.ctime_nsec();
        Ok(())
    }
    fn owner_path(&self) -> Result<PathBuf> {
        let name = self
            .identity
            .path
            .file_name()
            .context("Image filename missing")?
            .to_str()
            .context("Image filename must be UTF-8")?;
        Ok(self
            .identity
            .path
            .with_file_name(format!(".{name}.spark-apfs-owner.json")))
    }
    pub fn ensure_no_pending(&self) -> Result<()> {
        ensure!(
            !self.owner_path()?.exists(),
            "An earlier batch owns this image; inspect/apply/recover that journal first"
        );
        Ok(())
    }
    pub fn bind_journal(&self, dir: &Path) -> Result<()> {
        self.ensure_no_pending()?;
        let owner = self.owner_path()?;
        publish(
            owner.parent().unwrap(),
            owner.file_name().unwrap().to_str().unwrap(),
            &fs::canonicalize(dir)?,
        )
    }
    pub fn check_journal(&self, dir: &Path) -> Result<()> {
        let owner: PathBuf = unseal(&self.owner_path()?)?;
        ensure!(
            owner == fs::canonicalize(dir)?,
            "Image belongs to a different journal"
        );
        Ok(())
    }
    pub fn release_journal(&self, dir: &Path) -> Result<()> {
        if !self.owner_path()?.exists() {
            return Ok(());
        }
        self.check_journal(dir)?;
        let p = self.owner_path()?;
        fs::remove_file(&p)?;
        sync_dir(p.parent().unwrap())
    }
}
impl Device for Image {
    fn len(&self) -> u64 {
        self.identity.size
    }
    fn read(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        bounds(self.len(), offset, bytes.len())?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(bytes)?;
        Ok(())
    }
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<()> {
        bounds(self.len(), offset, bytes.len())?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(bytes)?;
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        durable_sync(&self.file)?;
        Ok(())
    }
}
fn ensure_not_attached(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/hdiutil")
            .arg("info")
            .output()?;
        ensure!(
            output.status.success(),
            "Cannot establish whether image is attached"
        );
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((key, value)) = line.split_once(':') {
                if key.trim() == "image-path" {
                    ensure!(
                        Path::new(value.trim()) != path,
                        "Detach this image before writing or recovering"
                    );
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for entry in fs::read_dir("/sys/block")? {
            let entry = entry?;
            let backing = entry.path().join("loop/backing_file");
            if backing.exists() {
                let name = fs::read_to_string(backing)?;
                let name = PathBuf::from(format!("/{}", name.trim().trim_start_matches('/')));
                ensure!(
                    name != path,
                    "Image is attached to a loop device; detach first"
                );
            }
        }
    }
    Ok(())
}
fn bounds(size: u64, off: u64, len: usize) -> Result<()> {
    ensure!(
        off.checked_add(len as u64).is_some_and(|n| n <= size),
        "Out-of-range I/O"
    );
    Ok(())
}
pub(crate) fn lock(file: &File) -> Result<()> {
    // flock is advisory; this tool requires an offline image under its exclusive ownership.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .context("Another process owns the transaction/image");
    }
    Ok(())
}
pub(crate) fn durable_sync(file: &File) -> Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("Full durable flush failed; preserve journal");
    }
    Ok(())
}
pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    durable_sync(&File::open(path)?)?;
    Ok(())
}
fn open_new(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    sha256: String,
    payload: String,
}
fn sealed<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let payload = serde_json::to_string(value)?;
    Ok(serde_json::to_vec(&Envelope {
        version: 1,
        sha256: hash(payload.as_bytes()),
        payload,
    })?)
}
pub(crate) fn unseal<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(
        f.metadata()?.is_file() && f.metadata()?.len() <= MAX_MANIFEST,
        "Invalid metadata file"
    );
    let mut b = Vec::new();
    f.read_to_end(&mut b)?;
    let env: Envelope = serde_json::from_slice(&b).context("Corrupt metadata; preserve journal")?;
    ensure!(
        env.version == 1 && hash(env.payload.as_bytes()) == env.sha256,
        "Metadata checksum/version mismatch"
    );
    Ok(serde_json::from_str(&env.payload)?)
}
pub(crate) fn publish<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    let data = sealed(value)?;
    ensure!(data.len() as u64 <= MAX_MANIFEST, "Metadata limit exceeded");
    // A prior interrupted publication may have left a temp file. It is never authoritative.
    let tmp = dir.join(format!("{name}.next"));
    if tmp.exists() {
        ensure!(
            fs::symlink_metadata(&tmp)?.is_file(),
            "Unexpected metadata temp type"
        );
        fs::remove_file(&tmp)?;
    }
    let mut f = open_new(&tmp)?;
    f.write_all(&data)?;
    durable_sync(&f)?;
    fs::rename(&tmp, dir.join(name))?;
    sync_dir(dir)
}
pub(crate) fn free_bytes(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(p.as_ptr(), s.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let s = unsafe { s.assume_init() };
    Ok((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    pub offset: u64,
    pub len: usize,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
    Write { target: u64, blob: Blob },
    Flush,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub identity: Identity,
    pub cap: u64,
    pub undo: BTreeMap<u64, Blob>,
    pub ops: Vec<Op>,
    pub undo_len: u64,
    pub redo_len: u64,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum State {
    Building,
    Prepared,
    Applying,
    Recovering,
    RolledBack,
    Committed,
}

pub struct Journal {
    pub dir: PathBuf,
    _lock: File,
    undo: File,
    redo: File,
    pub manifest: Manifest,
    pub state: State,
}
impl Journal {
    pub fn open(dir: &Path) -> Result<Self> {
        let dir = fs::canonicalize(dir)?;
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("lock"))?;
        lock(&guard)?;
        let state: State = unseal(&dir.join("state.json"))?;
        ensure!(
            state != State::Building,
            "Preparation incomplete; original was not modified by this transaction"
        );
        let manifest: Manifest = unseal(&dir.join("manifest.json"))?;
        let undo = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("undo.bin"))?;
        let redo = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("redo.bin"))?;
        let mut j = Self {
            dir,
            _lock: guard,
            undo,
            redo,
            manifest,
            state,
        };
        j.validate()?;
        Ok(j)
    }
    pub fn set_state(&mut self, state: State) -> Result<()> {
        publish(&self.dir, "state.json", &state)?;
        fault_point(&format!("state-{state:?}"));
        self.state = state;
        Ok(())
    }
    fn validate(&mut self) -> Result<()> {
        ensure!(
            self.undo.metadata()?.is_file() && self.redo.metadata()?.is_file(),
            "Journal data must be regular files"
        );
        ensure!(
            self.undo.metadata()?.len() == self.manifest.undo_len
                && self.redo.metadata()?.len() == self.manifest.redo_len,
            "Journal data length mismatch"
        );
        ensure!(
            self.manifest
                .undo_len
                .checked_add(self.manifest.redo_len)
                .is_some_and(|n| n <= self.manifest.cap),
            "Invalid journal budget"
        );
        let mut offsets = Vec::new();
        for (target, blob) in &self.manifest.undo {
            validate_page(*target, blob, self.manifest.identity.size)?;
            offsets.push(blob.offset);
            read_blob(&mut self.undo, blob)?;
        }
        offsets.sort_unstable();
        for (i, off) in offsets.iter().enumerate() {
            ensure!(
                *off == i as u64 * BLOCK as u64,
                "Undo coverage/order mismatch"
            );
        }
        ensure!(
            offsets.len() as u64 * BLOCK as u64 == self.manifest.undo_len,
            "Unaccounted undo bytes"
        );
        let mut redo_offset = 0;
        for op in &self.manifest.ops {
            if let Op::Write { target, blob } = op {
                validate_page(*target, blob, self.manifest.identity.size)?;
                ensure!(
                    self.manifest.undo.contains_key(target),
                    "Redo has no undo coverage"
                );
                ensure!(blob.offset == redo_offset, "Redo offset discontinuity");
                redo_offset += BLOCK as u64;
                read_blob(&mut self.redo, blob)?;
            }
        }
        ensure!(
            redo_offset == self.manifest.redo_len,
            "Unaccounted redo bytes"
        );
        Ok(())
    }
    pub fn apply<D: Device>(&mut self, dev: &mut D) -> Result<()> {
        ensure!(
            self.state == State::Prepared,
            "Apply requires PREPARED; incomplete writes require recover"
        );
        self.validate()?;
        ensure!(dev.len() == self.manifest.identity.size, "Wrong image size");
        let mut page = vec![0; BLOCK];
        for (target, blob) in &self.manifest.undo {
            dev.read(*target, &mut page)?;
            ensure!(hash(&page) == blob.sha256, "Target changed before apply");
        }
        self.set_state(State::Applying)?;
        // From here any error retains APPLYING and the complete undo log.
        for (step, op) in self.manifest.ops.iter().enumerate() {
            match op {
                Op::Write { target, blob } => {
                    dev.write(*target, &read_blob(&mut self.redo, blob)?)?;
                    fault_point("apply-write");
                }
                Op::Flush => dev.flush()?,
            }
            // Test-only: model every persisted prefix without running recovery.
            #[cfg(feature = "fault-injection")]
            if std::env::var("SPARK_APFS_KILL_AFTER_APPLY_OP")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                == Some(step + 1)
            {
                unsafe { libc::kill(libc::getpid(), libc::SIGKILL); }
            }
            #[cfg(not(feature = "fault-injection"))]
            let _ = step;
        }
        dev.flush()?;
        let mut last = BTreeMap::new();
        for op in &self.manifest.ops {
            if let Op::Write { target, blob } = op {
                last.insert(*target, blob.clone());
            }
        }
        for (target, blob) in last {
            dev.read(target, &mut page)?;
            ensure!(
                hash(&page) == blob.sha256,
                "Post-write verification failed; recover required"
            );
        }
        self.set_state(State::Committed)
    }
    pub fn recover<D: Device>(&mut self, dev: &mut D) -> Result<()> {
        self.validate()?;
        ensure!(dev.len() == self.manifest.identity.size, "Wrong image size");
        match self.state {
            State::Committed => bail!("Already COMMITTED; automatic rollback is forbidden"),
            State::RolledBack => return Ok(()),
            State::Prepared => return self.set_state(State::RolledBack),
            State::Applying | State::Recovering => (),
            State::Building => bail!("Incomplete preparation"),
        }
        self.set_state(State::Recovering)?;
        // Target remains offline. Descending order restores bootstrap block 0 last.
        for (target, blob) in self.manifest.undo.iter().rev() {
            dev.write(*target, &read_blob(&mut self.undo, blob)?)?;
            fault_point("recover-write");
        }
        dev.flush()?;
        let mut page = vec![0; BLOCK];
        for (target, blob) in &self.manifest.undo {
            dev.read(*target, &mut page)?;
            ensure!(
                hash(&page) == blob.sha256,
                "Rollback verification failed; keep journal and retry"
            );
        }
        self.set_state(State::RolledBack)
    }
    pub fn cleanup(&mut self) -> Result<()> {
        ensure!(
            matches!(self.state, State::Committed | State::RolledBack),
            "Only terminal journals may be cleaned"
        );
        let receipt = serde_json::json!({"state": self.state, "image": self.manifest.identity.path, "manifest_sha256": hash(&sealed(&self.manifest)?), "released_data_bytes": self.manifest.undo_len+self.manifest.redo_len});
        // Publish the receipt before deletion. A crash leaves a terminal receipt
        // and zero or more known data files; cleanup can resume using the receipt.
        publish(&self.dir, "receipt.json", &receipt)?;
        fault_point("cleanup-receipt");
        for name in ["undo.bin", "redo.bin", "manifest.json"] {
            fs::remove_file(self.dir.join(name))?;
        }
        sync_dir(&self.dir)
    }
}
pub fn read_receipt(dir: &Path) -> Result<serde_json::Value> {
    unseal(&dir.join("receipt.json"))
}
pub fn resume_cleanup(dir: &Path) -> Result<()> {
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("lock"))?;
    lock(&guard)?;
    let receipt = read_receipt(dir)?;
    ensure!(
        matches!(receipt["state"].as_str(), Some("Committed" | "RolledBack")),
        "Receipt is not terminal"
    );
    for name in ["undo.bin", "redo.bin", "manifest.json"] {
        let p = dir.join(name);
        match fs::symlink_metadata(&p) {
            Ok(m) => {
                ensure!(m.is_file(), "Unexpected journal data type");
                fs::remove_file(p)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    sync_dir(dir)
}
fn validate_page(target: u64, blob: &Blob, size: u64) -> Result<()> {
    ensure!(
        target % BLOCK as u64 == 0 && blob.len == BLOCK,
        "Invalid block alignment"
    );
    bounds(size, target, blob.len)
}
fn read_blob(file: &mut File, blob: &Blob) -> Result<Vec<u8>> {
    ensure!(blob.len == BLOCK, "Unexpected block size");
    bounds(file.metadata()?.len(), blob.offset, blob.len)?;
    let mut b = vec![0; blob.len];
    file.seek(SeekFrom::Start(blob.offset))?;
    file.read_exact(&mut b)?;
    ensure!(
        hash(&b) == blob.sha256,
        "Corrupt journal block; refusing target writes"
    );
    Ok(b)
}

/// Stages every write into a bounded disk log. Target is read-only during planning.
pub struct Overlay<D: Device> {
    base: D,
    dir: PathBuf,
    guard: File,
    undo: File,
    redo: File,
    manifest: Manifest,
    latest: BTreeMap<u64, Blob>,
    reserve: u64,
}
impl<D: Device> Overlay<D> {
    pub fn new(
        base: D,
        identity: Identity,
        dir: &Path,
        cap: u64,
        reserve: u64,
        description: String,
    ) -> Result<Self> {
        ensure!(
            cap >= 2 * BLOCK as u64 && cap <= 1024 * 1024 * 1024,
            "Journal cap must be 8 KiB..1 GiB"
        );
        ensure!(identity.size == base.len(), "Identity size mismatch");
        let parent = fs::canonicalize(dir.parent().context("Journal needs a parent directory")?)?;
        #[cfg(target_os = "linux")]
        if identity.generation.is_some() {
            crate::physical::validate_spool(&identity.path, &parent)?;
        }
        let dir = parent.join(dir.file_name().context("Journal needs a name")?);
        ensure!(
            free_bytes(&parent)? >= reserve.saturating_add(1024 * 1024),
            "Insufficient spool headroom; original unchanged"
        );
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        sync_dir(&parent)?;
        fault_point("prepare-dir-created");
        let guard = open_new(&dir.join("lock"))?;
        lock(&guard)?;
        let undo = open_new(&dir.join("undo.bin"))?;
        let redo = open_new(&dir.join("redo.bin"))?;
        publish(&dir, "state.json", &State::Building)?;
        Ok(Self {
            base,
            dir,
            guard,
            undo,
            redo,
            latest: BTreeMap::new(),
            reserve,
            manifest: Manifest {
                identity,
                cap,
                undo: BTreeMap::new(),
                ops: Vec::new(),
                undo_len: 0,
                redo_len: 0,
                description,
            },
        })
    }
    fn space(&self, additional: u64) -> Result<()> {
        let used = self.manifest.undo_len + self.manifest.redo_len;
        ensure!(
            used.checked_add(additional)
                .is_some_and(|x| x <= self.manifest.cap),
            "Journal cap reached; original unchanged, use a smaller batch"
        );
        ensure!(
            self.manifest.ops.len() < 16000,
            "Batch operation limit reached; original unchanged"
        );
        ensure!(
            free_bytes(&self.dir)?
                >= self
                    .reserve
                    .saturating_add(additional)
                    .saturating_add(MAX_MANIFEST),
            "Spool low-space guard; original unchanged"
        );
        Ok(())
    }
    pub fn statistics(&self) -> serde_json::Value {
        serde_json::json!({"undo_bytes":self.manifest.undo_len,"redo_bytes":self.manifest.redo_len,"journal_bytes":self.manifest.undo_len+self.manifest.redo_len,"operations":self.manifest.ops.len(),"unique_blocks":self.manifest.undo.len()})
    }
    pub fn finish(self) -> Result<(D, PathBuf)> {
        ensure!(!self.manifest.undo.is_empty(), "Empty transaction");
        durable_sync(&self.undo)?;
        durable_sync(&self.redo)?;
        // BTreeMap order is independent from first-touch order. Check append
        // coverage without copying or rewriting the undo data.
        let mut entries: Vec<_> = self.manifest.undo.values().cloned().collect();
        entries.sort_by_key(|b| b.offset);
        let mut expected = 0;
        for b in entries {
            ensure!(b.offset == expected, "Undo append gap");
            expected += BLOCK as u64;
        }
        publish(&self.dir, "manifest.json", &self.manifest)?;
        publish(&self.dir, "state.json", &State::Prepared)?;
        drop(self.guard);
        Ok((self.base, self.dir))
    }
}
impl<D: Device> Device for Overlay<D> {
    fn len(&self) -> u64 {
        self.base.len()
    }
    fn read(&mut self, off: u64, out: &mut [u8]) -> Result<()> {
        bounds(self.len(), off, out.len())?;
        let mut done = 0;
        while done < out.len() {
            let at = off + done as u64;
            let pageoff = at / BLOCK as u64 * BLOCK as u64;
            let start = (at - pageoff) as usize;
            let count = (BLOCK - start).min(out.len() - done);
            let page = if let Some(blob) = self.latest.get(&pageoff) {
                read_blob(&mut self.redo, blob)?
            } else {
                let mut p = vec![0; BLOCK];
                self.base.read(pageoff, &mut p)?;
                p
            };
            out[done..done + count].copy_from_slice(&page[start..start + count]);
            done += count;
        }
        Ok(())
    }
    fn write(&mut self, off: u64, input: &[u8]) -> Result<()> {
        bounds(self.len(), off, input.len())?;
        let mut done = 0;
        while done < input.len() {
            let at = off + done as u64;
            let pageoff = at / BLOCK as u64 * BLOCK as u64;
            let start = (at - pageoff) as usize;
            let count = (BLOCK - start).min(input.len() - done);
            let fresh = !self.manifest.undo.contains_key(&pageoff);
            self.space(if fresh {
                2 * BLOCK as u64
            } else {
                BLOCK as u64
            })?;
            if fresh {
                let mut original = vec![0; BLOCK];
                self.base.read(pageoff, &mut original)?;
                let blob = Blob {
                    offset: self.manifest.undo_len,
                    len: BLOCK,
                    sha256: hash(&original),
                };
                self.undo.seek(SeekFrom::End(0))?;
                self.undo.write_all(&original)?;
                self.manifest.undo_len += BLOCK as u64;
                self.manifest.undo.insert(pageoff, blob);
            }
            let mut page = vec![0; BLOCK];
            self.read(pageoff, &mut page)?;
            page[start..start + count].copy_from_slice(&input[done..done + count]);
            let blob = Blob {
                offset: self.manifest.redo_len,
                len: BLOCK,
                sha256: hash(&page),
            };
            self.redo.seek(SeekFrom::End(0))?;
            self.redo.write_all(&page)?;
            self.manifest.redo_len += BLOCK as u64;
            self.manifest.ops.push(Op::Write {
                target: pageoff,
                blob: blob.clone(),
            });
            self.latest.insert(pageoff, blob);
            done += count;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        self.space(0)?;
        self.manifest.ops.push(Op::Flush);
        Ok(())
    }
}

/// Discard only a preparation that never acquired permission to apply.
/// The caller holds the target image lock and verifies unchanged identity.
pub(crate) fn discard_building(dir: &Path, image: &Image) -> Result<()> {
    image.ensure_no_pending()?;
    discard_building_owned(dir, false)
}
/// Caller holds the target FD and a verified enclosing session owner.
pub(crate) fn discard_building_owned(dir: &Path, allow_missing_state: bool) -> Result<()> {
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("lock"))?;
    lock(&guard)?;
    if dir.join("state.json").exists() {
        let state: State = unseal(&dir.join("state.json"))?;
        ensure!(state == State::Building, "Only BUILDING can be discarded");
    } else {
        ensure!(allow_missing_state, "Missing preparation state");
    }
    let allowed = [
        "lock",
        "state.json",
        "state.json.next",
        "manifest.json",
        "manifest.json.next",
        "undo.bin",
        "redo.bin",
    ];
    for e in fs::read_dir(dir)? {
        let e = e?;
        ensure!(
            allowed.contains(&e.file_name().to_str().unwrap_or("")) && e.file_type()?.is_file(),
            "Unexpected preparation file; preserve it"
        );
    }
    for e in fs::read_dir(dir)? {
        fs::remove_file(e?.path())?;
    }
    fs::remove_dir(dir)?;
    sync_dir(dir.parent().context("No journal parent")?)
}

//! Durable bounded write queue shared by writable FUSE and crash-recovery CLI.
//! One Image FD stays exclusively owned throughout the session. Acknowledged
//! data is in immutable fsynced payloads plus a checksummed durable manifest.
use crate::{
    apfs_batch::{self, Action, Adapter},
    journal::{self, Identity, Image, Journal, State},
};
use anyhow::{ensure, Context, Result};
use apfs::{Attr, FsView};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const GROUP_BYTES: u64 = 4 * 1024 * 1024;
pub const JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
struct Payload {
    name: String,
    bytes: u64,
    sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    action: Action,
    payload: Option<Payload>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    identity: Identity,
    offset: u64,
    group_bytes: u64,
    cap: u64,
    reserve: u64,
    next_blob: u64,
    sequence: u64,
    queue: Vec<Pending>,
    active: Option<String>,
    #[serde(default)]
    active_count: Option<usize>,
    garbage: Vec<String>,
    closed: bool,
}
pub struct Session {
    image: Image,
    dir: PathBuf,
    _lock: File,
    record: Record,
    poisoned: bool,
    lookup_cache: apfs::LookupCache,
}
struct CachedView<'a> {
    view: FsView<Adapter<&'a mut Image>>,
    cache: &'a mut apfs::LookupCache,
}
impl<'a> std::ops::Deref for CachedView<'a> {
    type Target = FsView<Adapter<&'a mut Image>>;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}
impl<'a> std::ops::DerefMut for CachedView<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.view
    }
}
impl Drop for CachedView<'_> {
    fn drop(&mut self) {
        self.view.exchange_lookup_cache(self.cache);
    }
}
fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        && s != "."
        && s != ".."
}
fn fail_errno(errno: i32) -> anyhow::Error {
    std::io::Error::from_raw_os_error(errno).into()
}
fn new_file(p: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)?)
}
impl Session {
    pub fn start(
        target: &Path,
        offset: u64,
        dir: &Path,
        group_bytes: u64,
        reserve: u64,
    ) -> Result<Self> {
        ensure!(
            (4096..=GROUP_BYTES).contains(&group_bytes),
            "Buffer threshold must be 4096..4194304 bytes"
        );
        let mut image = Image::open(target, true)?;
        image.ensure_no_pending()?;
        ensure!(
            image.identity.generation.is_none() || offset == 0,
            "Enrolled partition requires offset 0"
        );
        let len = apfs_batch::container_range(&mut image, offset)?;
        let mut dev = Adapter::new(&mut image, offset, len);
        let c = apfs_core::container::Container::open(&mut dev)
            .map_err(|e| anyhow::anyhow!("Container: {e:?}"))?;
        ensure!(
            c.superblock.fs_oids.len() == 1,
            "Writable mount requires one APFS volume"
        );
        let mut view = FsView::open(dev)?;
        ensure!(
            !view.volume_has_snapshots()?,
            "Snapshots are not supported by the writable beta"
        );
        let (v, _) = view.read_vsb_omap_raw()?;
        let u = |o| u64::from_le_bytes(v[o..o + 8].try_into().unwrap());
        ensure!(
            u(264) & 1 == 1 && u(56) & !0x9 == 0 && u(160) == 0 && u(168) == 0,
            "Unsupported/encrypted/reverting APFS volume"
        );
        drop(view);
        let parent = fs::canonicalize(dir.parent().context("Session needs a parent")?)?;
        #[cfg(target_os = "linux")]
        {
            if image.identity.generation.is_some() {
                crate::physical::validate_spool(&image.identity.path, &parent)?;
            }
            let mut st = std::mem::MaybeUninit::<libc::statfs>::uninit();
            let c = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes())?;
            if unsafe { libc::statfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            ensure!(
                [0xef53, 0x58465342].contains(&(unsafe { st.assume_init() }.f_type as u64)),
                "Writable mount queue requires persistent ext4/XFS storage"
            );
        }
        ensure!(
            journal::free_bytes(&parent)? >= reserve.saturating_add(96 * 1024 * 1024),
            "Insufficient durable queue headroom"
        );
        let dir = parent.join(dir.file_name().context("Session needs a name")?);
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        journal::sync_dir(&parent)?;
        let lock = new_file(&dir.join("lock"))?;
        journal::lock(&lock)?;
        let record = Record {
            version: 1,
            identity: image.identity.clone(),
            offset,
            group_bytes,
            cap: JOURNAL_BYTES,
            reserve,
            next_blob: 0,
            sequence: 0,
            queue: vec![],
            active: None,
            active_count: None,
            garbage: vec![],
            closed: false,
        };
        journal::publish(&dir, "session.json", &record)?;
        image.bind_journal(&dir)?;
        Ok(Self {
            image,
            dir,
            _lock: lock,
            record,
            poisoned: false,
            lookup_cache: Default::default(),
        })
    }
    pub fn resume_target(dir: &Path, target: &Path, offset: u64) -> Result<Self> {
        let record: Record = journal::unseal(&dir.join("session.json"))?;
        ensure!(
            record.identity.path == fs::canonicalize(target)? && record.offset == offset,
            "Requested target/offset differs from mount session"
        );
        Self::resume(dir)
    }
    pub fn resume(dir: &Path) -> Result<Self> {
        let dir = fs::canonicalize(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("lock"))?;
        journal::lock(&lock)?;
        let record: Record = journal::unseal(&dir.join("session.json"))?;
        ensure!(
            record.version == 1,
            "Session is closed or has an unsupported version"
        );
        ensure!(
            (4096..=GROUP_BYTES).contains(&record.group_bytes)
                && record.cap == JOURNAL_BYTES
                && record.queue.len() <= 64,
            "Invalid mount session budget"
        );
        ensure!(
            record
                .active_count
                .is_none_or(|n| record.active.is_some() && n > 0 && n <= record.queue.len()),
            "Invalid active queue prefix"
        );
        let image = Image::open(&record.identity.path, true)?;
        image.check_identity(&record.identity, false)?;
        if !record.closed {
            image.check_journal(&dir)?;
        }
        #[cfg(target_os = "linux")]
        if image.identity.generation.is_some() {
            crate::physical::validate_spool(&image.identity.path, &dir)?;
        }
        let mut s = Self {
            image,
            dir,
            _lock: lock,
            record,
            poisoned: false,
            lookup_cache: Default::default(),
        };
        s.validate_queue()?;
        s.settle_active()?;
        s.image.check_identity(&s.record.identity, true)?;
        s.garbage_collect()?;
        s.remove_orphan_payloads()?;
        if s.record.closed {
            s.image.release_journal(&s.dir)?;
        }
        Ok(s)
    }
    fn save(&self) -> Result<()> {
        journal::publish(&self.dir, "session.json", &self.record)
    }
    fn ready(&self) -> Result<()> {
        ensure!(
            !self.poisoned && !self.record.closed,
            "Mount session stopped; preserve it and run mount-recover"
        );
        Ok(())
    }
    fn view(&mut self) -> Result<CachedView<'_>> {
        self.ready()?;
        let len = apfs_batch::container_range(&mut self.image, self.record.offset)?;
        let mut view = FsView::open(Adapter::new(&mut self.image, self.record.offset, len))?;
        view.exchange_lookup_cache(&mut self.lookup_cache);
        Ok(CachedView {
            view,
            cache: &mut self.lookup_cache,
        })
    }
    pub fn attr(&mut self, path: &str) -> Result<Attr> {
        crate::reader::validate_path(path)?;
        let mut a = self
            .view()?
            .getattr(path)?
            .ok_or_else(|| fail_errno(libc::ENOENT))?;
        for q in &self.record.queue {
            if let Action::WriteAt {
                path: p, offset, ..
            } = &q.action
            {
                if p == path {
                    a.size = a
                        .size
                        .max(offset + q.payload.as_ref().context("Missing range payload")?.bytes);
                }
            }
        }
        Ok(a)
    }
    pub fn list(&mut self, path: &str) -> Result<Vec<apfs_core::catalog::DirEntry>> {
        // readdir already has inode and type in the directory record. Avoid
        // statting every child again for every kernel directory page.
        Ok(self.view()?.read_dir(path)?)
    }
    pub fn readlink(&mut self, path: &str) -> Result<Vec<u8>> {
        self.view()?
            .read_symlink(path)?
            .ok_or_else(|| fail_errno(libc::EINVAL))
    }
    pub fn read(&mut self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        ensure!(size <= 8 * 1024 * 1024, "Read request exceeds bound");
        let a = self.attr(path)?;
        if offset >= a.size {
            return Ok(vec![]);
        }
        let n = (a.size - offset).min(size as u64) as usize;
        let mut bytes = self.view()?.read_range(path, offset, n)?;
        bytes.resize(n, 0);
        for q in &self.record.queue {
            if let Action::WriteAt {
                path: p, offset: o, ..
            } = &q.action
            {
                if p != path {
                    continue;
                }
                let blob = q.payload.as_ref().context("Missing payload")?;
                let first = offset.max(*o);
                let last = (offset + n as u64).min(o + blob.bytes);
                if first < last {
                    let data = self.payload(blob)?;
                    bytes[(first - offset) as usize..(last - offset) as usize]
                        .copy_from_slice(&data[(first - o) as usize..(last - o) as usize]);
                }
            }
        }
        Ok(bytes)
    }
    pub fn writable(&mut self, path: &str) -> Result<()> {
        let a = self.attr(path)?;
        if a.mode & 0xf000 != 0x8000 || !self.view()?.range_writable_file(path)? {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        Ok(())
    }
    pub fn pending_bytes(&self) -> u64 {
        self.record
            .queue
            .iter()
            .filter_map(|q| q.payload.as_ref())
            .map(|p| p.bytes)
            .sum()
    }
    fn payload(&self, p: &Payload) -> Result<Vec<u8>> {
        ensure!(
            plain_name(&p.name) && p.name.starts_with("payload-") && p.bytes <= GROUP_BYTES,
            "Invalid payload description"
        );
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(&p.name))?;
        ensure!(
            f.metadata()?.is_file() && f.metadata()?.len() == p.bytes,
            "Payload size changed"
        );
        let mut b = vec![];
        f.read_to_end(&mut b)?;
        ensure!(
            journal::hash(&b) == p.sha256,
            "Corrupt queued write; preserve recovery files"
        );
        Ok(b)
    }
    fn validate_queue(&self) -> Result<()> {
        ensure!(
            self.pending_bytes() <= self.record.group_bytes,
            "Queue exceeds bound"
        );
        for q in &self.record.queue {
            if let Some(p) = &q.payload {
                self.payload(p)?;
                let source = match &q.action {
                    Action::WriteAt { source, .. } | Action::Put { source, .. } => source,
                    _ => anyhow::bail!("Unexpected queued payload"),
                };
                ensure!(*source == self.dir.join(&p.name), "Payload path mismatch");
            } else {
                ensure!(
                    !matches!(
                        q.action,
                        Action::WriteAt { .. } | Action::Put { .. } | Action::Append { .. }
                    ),
                    "Missing durable payload"
                );
            }
        }
        Ok(())
    }
    fn enqueue(&mut self, mut action: Action, data: Option<&[u8]>) -> Result<()> {
        self.ready()?;
        let result = (|| {
            ensure!(self.record.queue.len() < 64, "Queue operation bound");
            ensure!(
                journal::free_bytes(&self.dir)?
                    >= self.record.reserve.saturating_add(96 * 1024 * 1024),
                "Queue low-space guard"
            );
            let payload = if let Some(data) = data {
                ensure!(
                    self.pending_bytes() + data.len() as u64 <= self.record.group_bytes,
                    "Queue byte bound"
                );
                let name = format!("payload-{:016}.bin", self.record.next_blob);
                self.record.next_blob += 1;
                let p = self.dir.join(&name);
                let mut f = new_file(&p)?;
                f.write_all(data)?;
                journal::durable_sync(&f)?;
                journal::sync_dir(&self.dir)?;
                match &mut action {
                    Action::WriteAt { source, .. } | Action::Put { source, .. } => *source = p,
                    _ => anyhow::bail!("Unexpected payload action"),
                }
                Some(Payload {
                    name,
                    bytes: data.len() as u64,
                    sha256: journal::hash(data),
                })
            } else {
                None
            };
            self.record.queue.push(Pending { action, payload });
            self.save()?;
            journal::fault_point("mount-wal-published");
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn write(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<()> {
        self.ready()?;
        self.writable(path)?;
        if offset > self.attr(path)?.size {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        ensure!(
            offset
                .checked_add(data.len() as u64)
                .is_some_and(|n| n <= i64::MAX as u64),
            "Write offset overflow"
        );
        if data.is_empty() {
            return Ok(());
        }
        if data.len() as u64 > self.record.group_bytes {
            return Err(fail_errno(libc::EFBIG));
        }
        if self.pending_bytes() + data.len() as u64 > self.record.group_bytes
            || self.record.queue.len() >= 63
        {
            self.flush()?;
        }
        self.enqueue(
            Action::WriteAt {
                source: PathBuf::new(),
                path: path.into(),
                offset,
            },
            Some(data),
        )?;
        if self.pending_bytes() >= self.record.group_bytes {
            self.flush()?;
        }
        Ok(())
    }
    pub fn create(&mut self, path: &str) -> Result<()> {
        self.parent(path)?;
        self.flush()?;
        if self.view()?.getattr(path)?.is_some() {
            return Err(fail_errno(libc::EEXIST));
        }
        self.enqueue(
            Action::Put {
                source: PathBuf::new(),
                path: path.into(),
            },
            Some(&[]),
        )?;
        self.flush()
    }
    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        self.parent(path)?;
        if self.view()?.getattr(path)?.is_some() {
            return Err(fail_errno(libc::EEXIST));
        }
        self.flush()?;
        self.enqueue(Action::Mkdir { path: path.into() }, None)?;
        self.flush()
    }
    fn parent(&mut self, path: &str) -> Result<()> {
        crate::reader::validate_path(path)?;
        let (p, n) = path.rsplit_once('/').context("Invalid path")?;
        ensure!(!n.is_empty() && n.len() <= 255, "Invalid entry name");
        let a = self.attr(if p.is_empty() { "/" } else { p })?;
        if !a.is_dir {
            return Err(fail_errno(libc::ENOTDIR));
        }
        if a.bsd_flags & 0x00060006 != 0 {
            return Err(fail_errno(libc::EPERM));
        }
        Ok(())
    }
    pub fn truncate(&mut self, path: &str, size: u64) -> Result<()> {
        self.writable(path)?;
        if size > apfs_batch::MAX_INPUT {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        self.flush()?;
        self.enqueue(
            Action::Truncate {
                path: path.into(),
                size,
            },
            None,
        )?;
        self.flush()
    }
    pub fn unlink(&mut self, path: &str) -> Result<()> {
        self.writable(path)?;
        self.flush()?;
        self.enqueue(Action::Remove { path: path.into() }, None)?;
        self.flush()
    }
    pub fn rename(&mut self, path: &str, destination: &str, replace: bool) -> Result<()> {
        if path == destination {
            return Ok(());
        }
        self.writable(path)?;
        self.parent(destination)?;
        let (parent, _) = path.rsplit_once('/').context("Bad rename source")?;
        let (dest_parent, name) = destination
            .rsplit_once('/')
            .context("Bad rename destination")?;
        if parent != dest_parent {
            return Err(fail_errno(libc::EXDEV));
        }
        let target = self.view()?.getattr(destination)?;
        if target.is_some_and(|a| a.inode == self.attr(path).map(|a| a.inode).unwrap_or(u64::MAX)) {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        let exists = target.is_some();
        if exists {
            if !replace {
                return Err(fail_errno(libc::EEXIST));
            }
            self.writable(destination)?;
        }
        self.flush()?;
        // One durable queue publication, so a crash cannot acknowledge or replay
        // only the deletion half of an atomic replace.
        if exists {
            self.record.queue.push(Pending {
                action: Action::Remove {
                    path: destination.into(),
                },
                payload: None,
            });
        }
        self.enqueue(
            Action::Rename {
                path: path.into(),
                name: name.into(),
            },
            None,
        )?;
        self.flush()
    }
    fn actions(&self) -> Result<(Vec<Action>, usize)> {
        self.validate_queue()?;
        // Coalesce adjacent sequential writes; APFS metadata is updated once
        // per collected range instead of once per small application write.
        let mut actions = vec![];
        let mut i = 0;
        while i < self.record.queue.len() {
            let q = &self.record.queue[i];
            if let Action::WriteAt { path, offset, .. } = &q.action {
                let mut data = self.payload(q.payload.as_ref().unwrap())?;
                let mut j = i + 1;
                while j < self.record.queue.len() {
                    let n = &self.record.queue[j];
                    match &n.action {
                        Action::WriteAt {
                            path: p, offset: o, ..
                        } if p == path && *o == *offset + data.len() as u64 => {
                            data.extend(self.payload(n.payload.as_ref().unwrap())?);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                let p = self.dir.join(format!("merged-{i:04}.bin"));
                if p.exists() {
                    ensure!(
                        fs::symlink_metadata(&p)?.is_file(),
                        "Unexpected merged payload"
                    );
                    fs::remove_file(&p)?;
                }
                let mut f = new_file(&p)?;
                f.write_all(&data)?;
                journal::durable_sync(&f)?;
                actions.push(Action::WriteAt {
                    source: p,
                    path: path.clone(),
                    offset: *offset,
                });
                // A write group is its own bounded recovery transaction. Do
                // not multiply whole-volume metadata cost by unrelated writes.
                return Ok((actions, j));
            } else {
                actions.push(q.action.clone());
                i += 1;
            }
        }
        Ok((actions, i))
    }
    pub fn flush(&mut self) -> Result<()> {
        self.ready()?;
        let result = self.flush_inner();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn flush_inner(&mut self) -> Result<()> {
        self.settle_active()?;
        while !self.record.queue.is_empty() {
            let (actions, count) = self.actions()?;
            let name = format!("txn-{:016}", self.record.sequence);
            self.record.active = Some(name.clone());
            self.record.active_count = Some(count);
            self.save()?;
            apfs_batch::prepare_held(
                &mut self.image,
                &self.dir.join(&name),
                self.record.offset,
                &actions,
                self.record.cap,
                self.record.reserve,
            )?;
            self.settle_active()?;
        }
        Ok(())
    }
    fn settle_active(&mut self) -> Result<()> {
        let Some(name) = self.record.active.clone() else {
            return Ok(());
        };
        ensure!(
            plain_name(&name) && name.starts_with("txn-"),
            "Invalid active journal path"
        );
        let p = self.dir.join(&name);
        if !p.exists() {
            self.record.active = None;
            self.record.active_count = None;
            self.save()?;
            return Ok(());
        }
        let state: Option<State> = if p.join("state.json").exists() {
            Some(journal::unseal(&p.join("state.json"))?)
        } else {
            None
        };
        if state.is_none() || state == Some(State::Building) {
            self.image.check_journal(&self.dir)?;
            self.image.check_identity(&self.record.identity, true)?;
            journal::discard_building_owned(&p, true)?;
            self.record.active = None;
            self.record.active_count = None;
            self.save()?;
            return Ok(());
        }
        let mut j = Journal::open(&p)?;
        self.image.check_identity(&j.manifest.identity, false)?;
        match j.state {
            State::Prepared => j.apply(&mut self.image)?,
            State::Applying | State::Recovering => {
                j.recover(&mut self.image)?;
            }
            State::Committed | State::RolledBack => (),
            State::Building => unreachable!(),
        }
        let committed = j.state == State::Committed;
        drop(j);
        self.image.refresh_identity()?;
        self.record.identity = self.image.identity.clone();
        self.record.garbage.push(name);
        self.record.active = None;
        // Legacy sessions omitted this field and committed the complete queue.
        let count = self
            .record
            .active_count
            .take()
            .unwrap_or(self.record.queue.len());
        ensure!(
            count > 0 && count <= self.record.queue.len(),
            "Invalid committed queue prefix"
        );
        self.record.sequence += 1;
        if committed {
            self.record.garbage.extend(
                self.record
                    .queue
                    .iter()
                    .take(count)
                    .filter_map(|q| q.payload.as_ref().map(|p| p.name.clone())),
            );
            self.record.queue.drain(..count);
        }
        self.save()?;
        journal::fault_point("mount-queue-retired");
        self.garbage_collect()
    }
    fn garbage_collect(&mut self) -> Result<()> {
        for name in &self.record.garbage {
            ensure!(plain_name(name), "Invalid cleanup path");
            let p = self.dir.join(name);
            if !p.exists() {
                continue;
            }
            if name.starts_with("txn-") {
                // Check terminal state BEFORE any data removal. The enclosing
                // manifest already durably records whether the queue was retired.
                if p.join("state.json").exists() {
                    let st: State = journal::unseal(&p.join("state.json"))?;
                    ensure!(
                        matches!(st, State::Committed | State::RolledBack),
                        "Refuse pending journal cleanup"
                    );
                }
                for f in fs::read_dir(&p)? {
                    let f = f?;
                    ensure!(f.file_type()?.is_file(), "Unexpected journal entry");
                    fs::remove_file(f.path())?;
                    journal::fault_point("mount-gc-entry");
                }
                fs::remove_dir(&p)?;
            } else {
                ensure!(
                    name.starts_with("payload-") && fs::symlink_metadata(&p)?.is_file(),
                    "Invalid payload cleanup"
                );
                fs::remove_file(p)?;
            }
        }
        self.record.garbage.clear();
        self.save()?;
        self.remove_orphan_payloads()?;
        journal::sync_dir(&self.dir)
    }
    fn remove_orphan_payloads(&self) -> Result<()> {
        for e in fs::read_dir(&self.dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("merged-")
                || (name.starts_with("payload-")
                    && !self
                        .record
                        .queue
                        .iter()
                        .any(|q| q.payload.as_ref().is_some_and(|p| p.name == name)))
            {
                ensure!(e.file_type()?.is_file(), "Unexpected orphan type");
                fs::remove_file(e.path())?;
            }
        }
        journal::sync_dir(&self.dir)
    }
    pub fn close(&mut self) -> Result<()> {
        self.flush()?;
        self.record.closed = true;
        self.save()?;
        journal::fault_point("mount-closed");
        self.image.release_journal(&self.dir)?;
        self.remove_orphan_payloads()?;
        Ok(())
    }
    pub fn matches_target(&self, target: &Path, offset: u64) -> Result<()> {
        ensure!(
            fs::canonicalize(target)? == self.record.identity.path && offset == self.record.offset,
            "Requested target/offset differs from mount session"
        );
        Ok(())
    }
    pub fn space(&mut self) -> Result<(u64, u64)> {
        let len = apfs_batch::container_range(&mut self.image, self.record.offset)?;
        let pending = self.pending_bytes().div_ceil(4096);
        let txn = apfs_write::txn::Transaction::begin(Adapter::new(
            &mut self.image,
            self.record.offset,
            len,
        ))?;
        Ok((len / 4096, txn.sm.free_count.saturating_sub(pending)))
    }
    pub fn is_closed(&self) -> bool {
        self.record.closed
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"session":self.dir,"pending_bytes":self.pending_bytes(),"operations":self.record.queue.len(),"committed_batches":self.record.sequence,"group_bytes":self.record.group_bytes,"closed":self.record.closed})
    }
}
/// Drains a crashed session and releases persistent ownership. The target must
/// be offline; Image::open and the session lock enforce single-process access.
pub fn recover(dir: &Path) -> Result<serde_json::Value> {
    let mut s = Session::resume(dir)?;
    if !s.is_closed() {
        s.flush()?;
        s.close()?;
    }
    Ok(s.status())
}

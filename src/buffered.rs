//! Bounded write queue with grouped and per-write durable policies shared by writable FUSE and crash-recovery CLI.
//! One Image FD stays exclusively owned throughout the session. Acknowledged
//! data is durable per write only in Durable mode. Grouped mode becomes durable
//! at flush/close/group boundaries; incomplete unsynced tail writes may be lost.
use crate::{
    apfs_batch::{self, Action, Adapter},
    journal::{self, Device, Identity, Image, Journal, State},
};
use anyhow::{ensure, Context, Result};
use apfs::{Attr, FsView};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePolicy {
    #[default]
    Durable,
    Grouped,
}
pub const GROUP_BYTES: u64 = 32 * 1024 * 1024;
const MAX_QUEUE: usize = 8192;
const MAX_FRAME_META: usize = 32768;
const MAX_STREAM: u64 = GROUP_BYTES + MAX_QUEUE as u64 * (MAX_FRAME_META as u64 + 136);
pub const JOURNAL_BYTES: u64 = 128 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
struct Payload {
    name: String,
    bytes: u64,
    sha256: String,
    #[serde(default)]
    file_offset: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    action: Action,
    payload: Option<Payload>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    version: u32,
    #[serde(default)]
    stream: Option<String>,
    #[serde(default)]
    write_policy: WritePolicy,
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
    // Valid only while our exclusive session has made no APFS mutation.
    write_cache: Option<(String, Attr)>,
    stream_buffer: Vec<u8>,
    verified_stream: Option<(String, Vec<u8>)>,
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
        Self::start_with_policy(target, offset, dir, group_bytes, reserve, WritePolicy::Durable)
    }
    pub fn start_with_policy(
        target: &Path, offset: u64, dir: &Path, group_bytes: u64, reserve: u64, write_policy: WritePolicy,
    ) -> Result<Self> {
        ensure!(
            (4096..=GROUP_BYTES).contains(&group_bytes),
            "Buffer threshold must be 4096..33554432 bytes"
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
            version: 2,
            stream: None,
            write_policy,
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
            write_cache: None,
            stream_buffer: Vec::new(),
            verified_stream: None,
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
            matches!(record.version, 1 | 2),
            "Session is closed or has an unsupported version"
        );
        ensure!(
            (4096..=GROUP_BYTES).contains(&record.group_bytes)
                && matches!(record.cap, 33554432 | JOURNAL_BYTES)
                && record.queue.len() <= MAX_QUEUE,
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
            write_cache: None,
            stream_buffer: Vec::new(),
            verified_stream: None,
        };
        s.replay_stream()?;
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
        self.ready()?;
        crate::reader::validate_path(path)?;
        if let Some((cached, attr)) = &self.write_cache {
            if cached == path { return Ok(*attr); }
        }
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
        self.ready()?;
        if self.write_cache.as_ref().is_some_and(|(p, _)| p == path) { return Ok(()); }
        let a = self.attr(path)?;
        if a.mode & 0xf000 != 0x8000 || !self.view()?.range_writable_file(path)? {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        self.write_cache = Some((path.into(), a));
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
            plain_name(&p.name)
                && (p.name.starts_with("payload-") || p.name.starts_with("stream-"))
                && p.bytes <= GROUP_BYTES,
            "Invalid payload description"
        );
        if let Some((name, data)) = &self.verified_stream {
            if name == &p.name {
                let off = p.file_offset.context("Missing verified stream offset")? as usize;
                let bytes = data.get(off..off + p.bytes as usize).context("Invalid verified stream range")?;
                ensure!(journal::hash(bytes) == p.sha256, "Corrupt verified stream payload");
                return Ok(bytes.to_vec());
            }
        }
        if self.record.stream.as_ref() == Some(&p.name) && !self.stream_buffer.is_empty() {
            let off = p.file_offset.context("Missing stream offset")? as usize;
            let bytes = self.stream_buffer.get(off..off + p.bytes as usize).context("Invalid memory stream range")?;
            ensure!(journal::hash(bytes) == p.sha256, "Corrupt memory stream payload");
            return Ok(bytes.to_vec());
        }
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(&p.name))?;
        ensure!(
            f.metadata()?.is_file()
                && match p.file_offset {
                    Some(off) => off
                        .checked_add(p.bytes)
                        .is_some_and(|end| end <= f.metadata().map(|m| m.len()).unwrap_or(0)),
                    None => f.metadata()?.len() == p.bytes,
                },
            "Payload size changed"
        );
        let mut b = vec![];
        f.seek(SeekFrom::Start(p.file_offset.unwrap_or(0)))?;
        f.take(p.bytes).read_to_end(&mut b)?;
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

    // One append per write; Durable adds a barrier per acknowledged write. A durable session
    // references the stream before its first frame. Frames include both metadata
    // and bytes in their checksum; only an incomplete final frame is discarded.
    fn append_stream(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<()> {
        if self.record.stream.is_none() {
            ensure!(
                self.record.queue.is_empty(),
                "Stream requires drained queue"
            );
            let name = format!("stream-{:016}.bin", self.record.next_blob);
            self.record.next_blob += 1;
            let file = new_file(&self.dir.join(&name))?;
            journal::durable_sync(&file)?;
            journal::sync_dir(&self.dir)?;
            self.record.version = 2; // Older readers must refuse this format.
            self.record.stream = Some(name);
            self.save()?;
        }
        if self.record.write_policy == WritePolicy::Durable || self.stream_buffer.is_empty() {
            ensure!(journal::free_bytes(&self.dir)? >= self.record.reserve.saturating_add(96 * 1024 * 1024), "Queue low-space guard");
        }
        let name = self.record.stream.clone().unwrap();
        let meta = serde_json::to_vec(&(path, offset))?;
        ensure!(
            meta.len() <= MAX_FRAME_META,
            "Stream metadata exceeds bound"
        );
        let mut frame = Vec::with_capacity(72 + meta.len() + data.len() + 64);
        frame.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let header_digest = journal::hash(&frame);
        frame.extend_from_slice(header_digest.as_bytes());
        frame.extend_from_slice(&meta);
        frame.extend_from_slice(data);
        let digest = journal::hash(&frame);
        frame.extend_from_slice(digest.as_bytes());
        let start;
        if self.record.write_policy == WritePolicy::Grouped {
            start = self.stream_buffer.len() as u64;
            ensure!(start + frame.len() as u64 <= MAX_STREAM, "Stream exceeds bound");
            self.stream_buffer.extend_from_slice(&frame);
        } else {
            let mut file = OpenOptions::new().read(true).append(true).custom_flags(libc::O_NOFOLLOW).open(self.dir.join(&name))?;
            ensure!(file.metadata()?.is_file(), "Invalid stream type");
            start = file.metadata()?.len();
            ensure!(start + frame.len() as u64 <= MAX_STREAM, "Stream exceeds bound");
            file.write_all(&frame)?;
            journal::fault_point("mount-stream-written");
            journal::durable_sync(&file)?;
            journal::fault_point("mount-stream-synced");
        }
        self.record.queue.push(Pending {
            action: Action::WriteAt {
                source: self.dir.join(&name),
                path: path.into(),
                offset,
            },
            payload: Some(Payload {
                name,
                bytes: data.len() as u64,
                sha256: journal::hash(data),
                file_offset: Some(start + 72 + meta.len() as u64),
            }),
        });
        journal::fault_point("mount-wal-published");
        Ok(())
    }
    fn freeze_stream(&mut self) -> Result<()> {
        if let Some(name) = self.record.stream.as_ref() {
            // Persist all frames before publishing references or changing APFS.
            let mut file = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW).open(self.dir.join(name))?;
            if !self.stream_buffer.is_empty() {
                ensure!(file.metadata()?.is_file() && file.metadata()?.len() == 0, "Memory stream backing file changed");
                ensure!(journal::free_bytes(&self.dir)? >= self.record.reserve.saturating_add(self.stream_buffer.len() as u64), "Stream low-space guard");
                file.write_all(&self.stream_buffer)?;
                journal::fault_point("mount-stream-written");
            }
            journal::durable_sync(&file)?;
            journal::fault_point("mount-stream-flushed");
            if !self.stream_buffer.is_empty() {
                let mut verified = vec![0; self.stream_buffer.len()];
                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut verified)?;
                ensure!(verified == self.stream_buffer, "Stream readback mismatch");
                self.verified_stream = Some((name.clone(), verified));
            }
            self.record.stream = None;
            self.save()?; // Queue references become durable before APFS changes.
            self.stream_buffer.clear();
            journal::fault_point("mount-stream-frozen");
        }
        Ok(())
    }
    fn replay_stream(&mut self) -> Result<()> {
        let Some(name) = self.record.stream.clone() else {
            return Ok(());
        };
        ensure!(
            self.record.version == 2
                && !self.record.closed
                && self.record.active.is_none()
                && self.record.queue.is_empty(),
            "Invalid stream state"
        );
        ensure!(
            plain_name(&name) && name.starts_with("stream-"),
            "Invalid stream path"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir.join(&name))?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= MAX_STREAM,
            "Invalid stream size"
        );
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes.len() - pos < 72 {
                break;
            }
            ensure!(
                journal::hash(&bytes[pos..pos + 8]).as_bytes() == &bytes[pos + 8..pos + 72],
                "Corrupt stream header; preserve recovery files"
            );
            let meta_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let data_len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            ensure!(
                meta_len > 0
                    && meta_len <= MAX_FRAME_META
                    && data_len > 0
                    && data_len as u64 <= self.record.group_bytes,
                "Invalid stream frame bounds"
            );
            let end = pos + 72 + meta_len + data_len;
            if end + 64 > bytes.len() {
                break;
            }
            ensure!(
                journal::hash(&bytes[pos..end]).as_bytes() == &bytes[end..end + 64],
                "Corrupt stream frame; preserve recovery files"
            );
            let (path, offset): (String, u64) =
                serde_json::from_slice(&bytes[pos + 72..pos + 72 + meta_len])?;
            crate::reader::validate_path(&path)?;
            ensure!(
                offset
                    .checked_add(data_len as u64)
                    .is_some_and(|n| n <= i64::MAX as u64),
                "Stream offset overflow"
            );
            ensure!(
                self.record.queue.len() < MAX_QUEUE
                    && self.pending_bytes() + data_len as u64 <= self.record.group_bytes,
                "Stream queue exceeds bound"
            );
            let data = &bytes[pos + 72 + meta_len..end];
            self.record.queue.push(Pending {
                action: Action::WriteAt {
                    source: self.dir.join(&name),
                    path,
                    offset,
                },
                payload: Some(Payload {
                    name: name.clone(),
                    bytes: data_len as u64,
                    sha256: journal::hash(data),
                    file_offset: Some((pos + 72 + meta_len) as u64),
                }),
            });
            pos = end + 64;
        }
        self.freeze_stream()
    }
    fn enqueue(&mut self, mut action: Action, data: Option<&[u8]>) -> Result<()> {
        self.ready()?;
        self.freeze_stream()?;
        let result = (|| {
            ensure!(self.record.queue.len() < MAX_QUEUE, "Queue operation bound");
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
                    file_offset: None,
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
            || self.record.queue.len() >= MAX_QUEUE - 1
        {
            self.flush()?;
        }
        if self.record.stream.is_none() && !self.record.queue.is_empty() {
            self.flush()?;
        }
        let result = self.append_stream(path, offset, data);
        if result.is_err() {
            self.poisoned = true;
        }
        result?;
        if let Some((cached, attr)) = &mut self.write_cache {
            if cached == path { attr.size = attr.size.max(offset + data.len() as u64); }
        }
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
        let a = self.attr(path)?;
        if a.mode & 0xf000 == 0xa000 {
            self.parent(path)?;
            if a.bsd_flags & 0x00060006 != 0 {
                return Err(fail_errno(libc::EPERM));
            }
        } else {
            self.writable(path)?;
        }
        self.flush()?;
        self.enqueue(Action::Remove { path: path.into() }, None)?;
        self.flush()
    }
    pub fn symlink(&mut self, path: &str, target: &[u8]) -> Result<()> {
        self.parent(path)?;
        if self.view()?.getattr(path)?.is_some() {
            return Err(fail_errno(libc::EEXIST));
        }
        if target.is_empty() || target.len() > 4096 || target.contains(&0) {
            return Err(fail_errno(libc::EINVAL));
        }
        self.flush()?;
        self.enqueue(
            Action::Symlink {
                path: path.into(),
                target: target.to_vec(),
            },
            None,
        )?;
        self.flush()
    }
    pub fn rmdir(&mut self, path: &str) -> Result<()> {
        let a = self.attr(path)?;
        if !a.is_dir || a.mode & 0xf000 != 0x4000 {
            return Err(fail_errno(libc::ENOTDIR));
        }
        if !self.list(path)?.is_empty() {
            return Err(fail_errno(libc::ENOTEMPTY));
        }
        self.parent(path)?;
        self.flush()?;
        self.enqueue(Action::Rmdir { path: path.into() }, None)?;
        self.flush()
    }
    pub fn set_attrs(
        &mut self,
        path: &str,
        mode: Option<u16>,
        atime_ns: Option<u64>,
        mtime_ns: Option<u64>,
    ) -> Result<()> {
        let a = self.attr(path)?;
        if a.bsd_flags & 0x00060006 != 0 {
            return Err(fail_errno(libc::EPERM));
        }
        if !matches!(a.mode & 0xf000, 0x4000 | 0x8000 | 0xa000) {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        if mode.is_some() && a.mode & 0xf000 == 0xa000 {
            return Err(fail_errno(libc::EOPNOTSUPP));
        }
        let mode = mode.filter(|m| a.mode & 0o7777 != *m);
        let atime_ns = atime_ns.filter(|t| a.access_time != *t);
        let mtime_ns = mtime_ns.filter(|t| a.mod_time != *t);
        if mode.is_none() && atime_ns.is_none() && mtime_ns.is_none() {
            return Ok(());
        }
        self.flush()?;
        self.enqueue(
            Action::SetAttrs {
                path: path.into(),
                mode,
                atime_ns,
                mtime_ns,
            },
            None,
        )?;
        self.flush()
    }
    pub fn rename(&mut self, path: &str, destination: &str, replace: bool) -> Result<()> {
        if path == destination {
            return Ok(());
        }
        let source = self.attr(path)?;
        if source.mode & 0xf000 == 0xa000 {
            self.parent(path)?;
            if source.bsd_flags & 0x00060006 != 0 {
                return Err(fail_errno(libc::EPERM));
            }
        } else {
            self.writable(path)?;
        }
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
            if target.unwrap().mode & 0xf000 == 0xa000 {
                self.parent(destination)?;
                if target.unwrap().bsd_flags & 0x00060006 != 0 {
                    return Err(fail_errno(libc::EPERM));
                }
            } else {
                self.writable(destination)?;
            }
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
        let _profile = journal::Phase::new("actions");
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
        let _profile = journal::Phase::new("buffer_flush");
        self.write_cache = None;
        self.freeze_stream()?;
        self.settle_active()?;
        while !self.record.queue.is_empty() {
            let (actions, count) = self.actions()?;
            let name = format!("txn-{:016}", self.record.sequence);
            self.record.active = Some(name.clone());
            self.record.active_count = Some(count);
            self.save()?;
            let prepare_profile = journal::Phase::new("prepare_total");
            apfs_batch::prepare_held(
                &mut self.image,
                &self.dir.join(&name),
                self.record.offset,
                &actions,
                self.record.cap,
                self.record.reserve,
            )?;
            drop(prepare_profile);
            self.settle_active()?;
        }
        Ok(())
    }
    fn settle_active(&mut self) -> Result<()> {
        let _profile = journal::Phase::new("settle_total");
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
            let retired: Vec<String> = self
                .record
                .queue
                .iter()
                .take(count)
                .filter_map(|q| q.payload.as_ref().map(|p| p.name.clone()))
                .collect();
            self.record.queue.drain(..count);
            for name in retired {
                if !self
                    .record
                    .queue
                    .iter()
                    .any(|q| q.payload.as_ref().is_some_and(|p| p.name == name))
                    && !self.record.garbage.contains(&name)
                {
                    self.record.garbage.push(name);
                }
            }
        }
        self.save()?;
        journal::fault_point("mount-queue-retired");
        self.garbage_collect()
    }
    fn garbage_collect(&mut self) -> Result<()> {
        let _profile = journal::Phase::new("garbage_collect");
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
                    (name.starts_with("payload-") || name.starts_with("stream-"))
                        && fs::symlink_metadata(&p)?.is_file(),
                    "Invalid payload cleanup"
                );
                fs::remove_file(p)?;
            }
        }
        if self.verified_stream.as_ref().is_some_and(|(name,_)| self.record.garbage.contains(name)) {
            self.verified_stream = None;
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
                || ((name.starts_with("payload-") || name.starts_with("stream-"))
                    && self.record.stream.as_ref() != Some(&name)
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
        // A clean handoff needs a final device flush even when the last
        // transaction already drained the queue before unmount.
        self.image.flush()?;
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
    pub fn write_policy(&self) -> WritePolicy { self.record.write_policy }
    pub fn is_closed(&self) -> bool {
        self.record.closed
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"session":self.dir,"write_policy":self.record.write_policy,"pending_bytes":self.pending_bytes(),"operations":self.record.queue.len(),"committed_batches":self.record.sequence,"group_bytes":self.record.group_bytes,"closed":self.record.closed})
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

/// Complete a stopped mount session and verify that the APFS source is safe to
/// hand to another OS. An active FUSE mount retains the session/device locks
/// and is refused; the caller must first perform a normal unmount.
pub fn handoff_ready(dir: &Path, target: &Path, offset: u64) -> Result<serde_json::Value> {
    let mut session = Session::resume_target(dir, target, offset)?;
    if !session.is_closed() {
        session.close()?;
    }
    ensure!(
        session.record.queue.is_empty()
            && session.record.active.is_none()
            && session.record.garbage.is_empty()
            && session.pending_bytes() == 0,
        "Handoff refused: queue or recovery journal remains"
    );
    session.image.flush()?;
    session.image.ensure_no_pending()?;
    let status = session.status();
    drop(session);
    #[cfg(target_os = "linux")]
    let source = if crate::physical::is_descriptor(target) {
        crate::physical::source_path(target)?
    } else {
        target.to_path_buf()
    };
    #[cfg(not(target_os = "linux"))]
    let source = target.to_path_buf();
    let identity = crate::reader::inspect(&source, offset)?;
    Ok(serde_json::json!({
        "status": "ready_to_disconnect",
        "session": status,
        "source_identity": identity,
        "device_sync": "completed",
        "external_owner": "absent"
    }))
}

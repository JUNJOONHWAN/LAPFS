//! Buffered writable FUSE beta. The kernel never acknowledges volatile writeback;
//! direct I/O requests are acknowledged only after Session's durable WAL publish.
use crate::buffered::{Session, GROUP_BYTES};
use anyhow::{ensure, Context, Result};
use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, MountOption, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use std::os::unix::ffi::OsStrExt;
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const TTL: Duration = Duration::ZERO;
fn kind(a: &apfs::Attr) -> FileType {
    match a.mode & 0xf000 {
        0x4000 => FileType::Directory,
        0xa000 => FileType::Symlink,
        _ => FileType::RegularFile,
    }
}
fn ino(a: &apfs::Attr) -> u64 {
    if a.inode == 2 {
        1
    } else {
        a.inode
    }
}
fn unix_nanos(value: TimeOrNow) -> Result<u64> {
    let time = match value {
        TimeOrNow::Now => SystemTime::now(),
        TimeOrNow::SpecificTime(time) => time,
    };
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    duration
        .as_nanos()
        .try_into()
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EOVERFLOW).into())
}
fn errno(op: &str, e: anyhow::Error) -> i32 {
    let code = e
        .chain()
        .find_map(|c| {
            c.downcast_ref::<std::io::Error>()
                .and_then(|e| e.raw_os_error())
        })
        .unwrap_or_else(|| {
            if e.to_string().contains("space") || e.to_string().contains("cap reached") {
                libc::ENOSPC
            } else {
                libc::EIO
            }
        });
    if !crate::error_log::expected_lookup_miss(op, code) {
        crate::error_log::event("error", op, Some(code), &format!("{e:#}"));
        eprintln!("LAPFS {op}: {e:#}");
    }
    code
}
struct Host {
    shutdown_failed: Arc<AtomicBool>,
    session: Session,
    paths: crate::inode_paths::InodePaths,
    handles: HashMap<u64, (u64, i32)>,
    next: u64,
    uid: u32,
    gid: u32,
}
impl Host {
    fn path(&self, id: u64) -> Result<String> {
        self.paths
            .get(id)
            .map(str::to_owned)
            .context("Unknown inode")
    }
    fn child(&self, id: u64, n: &OsStr) -> Result<String> {
        let n = n.to_str().context("Invalid UTF-8 name")?;
        ensure!(
            !n.is_empty() && n != "." && n != ".." && !n.contains('/') && n.len() <= 255,
            "Invalid name"
        );
        Ok(format!("{}/{}", self.path(id)?.trim_end_matches('/'), n))
    }
    fn remember(&mut self, id: u64, p: String) -> Result<()> {
        self.paths.remember(id, p)
    }
    fn attr(&mut self, p: &str) -> Result<FileAttr> {
        let a = self.session.attr(p)?;
        let a = if let Some(canonical) = self.paths.get(ino(&a)) {
            if canonical != p {
                self.session.attr(canonical)?
            } else {
                a
            }
        } else {
            a
        };
        Ok(FileAttr {
            ino: ino(&a),
            size: a.size,
            blocks: a.size.div_ceil(512),
            atime: UNIX_EPOCH + Duration::from_nanos(a.access_time),
            mtime: UNIX_EPOCH + Duration::from_nanos(a.mod_time),
            ctime: UNIX_EPOCH + Duration::from_nanos(a.change_time),
            crtime: UNIX_EPOCH + Duration::from_nanos(a.create_time),
            kind: kind(&a),
            perm: a.mode & 0o7777,
            nlink: if a.is_dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        })
    }
    fn handle(&mut self, id: u64, flags: i32) -> Result<u64> {
        self.handles
            .try_reserve(1)
            .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
        let fh = self.next;
        self.next = self.next.checked_add(1).context("Handle overflow")?;
        self.handles.insert(fh, (id, flags));
        Ok(fh)
    }
    fn busy(&self, id: u64) -> bool {
        self.handles.values().any(|(i, _)| *i == id)
    }
    fn sync_reply(&mut self, operation: &str, reply: ReplyEmpty) {
        match self.session.flush() {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno(operation, e)),
        }
    }
}
impl Filesystem for Host {
    fn forget(&mut self, _: &Request<'_>, id: u64, nlookup: u64) {
        let busy = self.busy(id);
        self.paths.forget(id, nlookup, busy);
    }
    fn init(&mut self, _: &Request<'_>, config: &mut KernelConfig) -> Result<(), i32> {
        config.set_max_write(128 * 1024).map_err(|_| libc::EINVAL)?;
        Ok(())
    }
    fn destroy(&mut self) {
        if let Err(e) = self.session.close() {
            self.shutdown_failed.store(true, Ordering::SeqCst);
            crate::error_log::event("error", "unmount", None, &format!("{e:#}"));
            eprintln!("LAPFS unmount incomplete, retain session and run mount-recover: {e:#}");
        }
    }
    fn lookup(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let r = (|| {
            let p = self.child(parent, name)?;
            let a = self.attr(&p)?;
            self.remember(a.ino, p)?;
            Ok(a)
        })();
        match r {
            Ok(a) => reply.entry(&TTL, &a, 0),
            Err(e) => reply.error(errno("lookup", e)),
        }
    }
    fn getattr(&mut self, _: &Request<'_>, id: u64, _: Option<u64>, reply: ReplyAttr) {
        let r = self.path(id).and_then(|p| self.attr(&p));
        match r {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(errno("getattr", e)),
        }
    }
    fn open(&mut self, _: &Request<'_>, id: u64, flags: i32, reply: ReplyOpen) {
        let r = (|| {
            let p = self.path(id)?;
            let a = self.attr(&p)?;
            if a.kind != FileType::RegularFile {
                return Err(std::io::Error::from_raw_os_error(libc::EISDIR).into());
            }
            if flags & libc::O_ACCMODE != libc::O_RDONLY {
                self.session.writable(&p)?;
            }
            if flags & libc::O_TRUNC != 0 {
                self.session.truncate(&p, 0)?;
            }
            self.handle(id, flags)
        })();
        match r {
            Ok(fh) => reply.opened(fh, 1),
            Err(e) => reply.error(errno("open", e)),
        }
    }
    fn create(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let r = (|| {
            let p = self.child(parent, name)?;
            self.session.create(&p)?;
            self.session
                .set_attrs(&p, Some(((mode & !umask) as u16) & 0o7777), None, None)?;
            let a = self.attr(&p)?;
            self.remember(a.ino, p)?;
            let fh = self.handle(a.ino, flags)?;
            Ok((a, fh))
        })();
        match r {
            Ok((a, fh)) => reply.created(&TTL, &a, 0, fh, 1),
            Err(e) => reply.error(errno("create", e)),
        }
    }
    fn read(
        &mut self,
        _: &Request<'_>,
        id: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _: i32,
        _: Option<u64>,
        reply: ReplyData,
    ) {
        let r = (|| {
            ensure!(offset >= 0, "Negative read offset");
            let (i, flags) = self.handles.get(&fh).context("Unknown handle")?;
            if *i != id || flags & libc::O_ACCMODE == libc::O_WRONLY {
                return Err(std::io::Error::from_raw_os_error(libc::EBADF).into());
            }
            let p = self.path(id)?;
            self.session.read(&p, offset as u64, size as usize)
        })();
        match r {
            Ok(b) => reply.data(&b),
            Err(e) => reply.error(errno("read", e)),
        }
    }
    fn write(
        &mut self,
        _: &Request<'_>,
        id: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _: u32,
        flags: i32,
        _: Option<u64>,
        reply: ReplyWrite,
    ) {
        let r = (|| {
            ensure!(offset >= 0, "Negative write offset");
            let (i, open_flags) = *self.handles.get(&fh).context("Unknown handle")?;
            if i != id || open_flags & libc::O_ACCMODE == libc::O_RDONLY {
                return Err(std::io::Error::from_raw_os_error(libc::EBADF).into());
            }
            let p = self.path(id)?;
            let off = if open_flags & libc::O_APPEND != 0 {
                self.session.attr(&p)?.size
            } else {
                offset as u64
            };
            self.session.write(&p, off, data)?;
            if (flags | open_flags) & (libc::O_SYNC | libc::O_DSYNC) != 0 {
                self.session.flush()?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => reply.written(data.len() as u32),
            Err(e) => reply.error(errno("write", e)),
        }
    }
    fn flush(&mut self, _: &Request<'_>, _: u64, _: u64, _: u64, reply: ReplyEmpty) {
        self.sync_reply("flush", reply)
    }
    fn fsync(&mut self, _: &Request<'_>, _: u64, _: u64, _: bool, reply: ReplyEmpty) {
        self.sync_reply("fsync", reply)
    }
    fn fsyncdir(&mut self, _: &Request<'_>, _: u64, _: u64, _: bool, reply: ReplyEmpty) {
        self.sync_reply("fsyncdir", reply)
    }
    fn release(
        &mut self,
        _: &Request<'_>,
        id: u64,
        fh: u64,
        _: i32,
        _: Option<u64>,
        _: bool,
        reply: ReplyEmpty,
    ) {
        self.handles.remove(&fh);
        let busy = self.busy(id);
        self.paths.release_if_unreferenced(id, busy);
        self.sync_reply("release", reply)
    }
    fn setattr(
        &mut self,
        _: &Request<'_>,
        id: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        _: Option<u64>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if uid.is_some_and(|v| v != self.uid)
            || gid.is_some_and(|v| v != self.gid)
            || crtime.is_some()
            || chgtime.is_some()
            || bkuptime.is_some()
            || flags.is_some()
        {
            crate::error_log::event(
                "error",
                "setattr",
                Some(libc::EOPNOTSUPP),
                "Unsupported metadata change",
            );
            reply.error(libc::EOPNOTSUPP);
            return;
        }
        let _ = ctime; // Kernel may include ctime for truncate; writer sets mutation time.
        let r = (|| {
            let p = self.path(id)?;
            if let Some(size) = size {
                self.session.truncate(&p, size)?;
            }
            let atime_ns = atime.map(unix_nanos).transpose()?;
            let mtime_ns = mtime.map(unix_nanos).transpose()?;
            self.session
                .set_attrs(&p, mode.map(|m| (m as u16) & 0o7777), atime_ns, mtime_ns)?;
            self.attr(&p)
        })();
        match r {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(errno("setattr", e)),
        }
    }
    fn mkdir(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let r = (|| {
            let p = self.child(parent, name)?;
            self.session.mkdir(&p)?;
            self.session
                .set_attrs(&p, Some(((mode & !umask) as u16) & 0o7777), None, None)?;
            let a = self.attr(&p)?;
            self.remember(a.ino, p)?;
            Ok(a)
        })();
        match r {
            Ok(a) => reply.entry(&TTL, &a, 0),
            Err(e) => reply.error(errno("mkdir", e)),
        }
    }
    fn unlink(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let r = (|| {
            let p = self.child(parent, name)?;
            let id = self.attr(&p)?.ino;
            if self.busy(id) {
                return Err(std::io::Error::from_raw_os_error(libc::EBUSY).into());
            }
            self.session.unlink(&p)?;
            self.paths.remove(id);
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno("unlink", e)),
        }
    }
    fn rmdir(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let r = (|| {
            let p = self.child(parent, name)?;
            let a = self.attr(&p)?;
            if a.kind != FileType::Directory {
                return Err(std::io::Error::from_raw_os_error(libc::ENOTDIR).into());
            }
            if self.busy(a.ino) {
                return Err(std::io::Error::from_raw_os_error(libc::EBUSY).into());
            }
            self.session.rmdir(&p)?;
            self.paths.remove(a.ino);
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno("rmdir", e)),
        }
    }
    fn rename(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        let r = (|| {
            if flags & !1 != 0 {
                return Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP).into());
            }
            let p = self.child(parent, name)?;
            let d = self.child(newparent, newname)?;
            let id = self.attr(&p)?.ino;
            let target = self.session.attr(&d).ok();
            if target.is_some_and(|a| self.busy(ino(&a))) {
                return Err(std::io::Error::from_raw_os_error(libc::EBUSY).into());
            }
            self.session.rename(&p, &d, flags & 1 == 0)?;
            if let Some(a) = target {
                self.paths.remove(ino(&a));
            }
            self.paths.rename(id, d)?;
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(errno("rename", e)),
        }
    }
    fn readlink(&mut self, _: &Request<'_>, id: u64, reply: ReplyData) {
        let r = self.path(id).and_then(|p| self.session.readlink(&p));
        match r {
            Ok(b) => reply.data(&b),
            Err(e) => reply.error(errno("readlink", e)),
        }
    }
    fn symlink(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let r = (|| {
            let p = self.child(parent, link_name)?;
            self.session.symlink(&p, target.as_os_str().as_bytes())?;
            let a = self.attr(&p)?;
            self.remember(a.ino, p)?;
            Ok(a)
        })();
        match r {
            Ok(a) => reply.entry(&TTL, &a, 0),
            Err(e) => reply.error(errno("symlink", e)),
        }
    }
    fn readdir(
        &mut self,
        _: &Request<'_>,
        id: u64,
        _: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let r = (|| {
            ensure!(offset >= 0, "Negative directory offset");
            let p = self.path(id)?;
            let parent = p
                .rsplit_once('/')
                .map(|x| x.0)
                .filter(|p| !p.is_empty())
                .unwrap_or("/");
            let parent_id = self.attr(parent)?.ino;
            let mut rows = vec![
                (id, FileType::Directory, ".".into()),
                (parent_id, FileType::Directory, "..".into()),
            ];
            for entry in self.session.list(&p)? {
                let id = if entry.file_id == 2 { 1 } else { entry.file_id };
                let kind = match entry.flags & 0xf {
                    4 => FileType::Directory,
                    10 => FileType::Symlink,
                    _ => FileType::RegularFile,
                };
                rows.push((id, kind, entry.name));
            }
            Ok(rows)
        })();
        match r {
            Ok(rows) => {
                for (n, (id, k, name)) in rows.into_iter().enumerate().skip(offset as usize) {
                    if reply.add(id, (n + 1) as i64, k, name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => reply.error(errno("readdir", e)),
        }
    }
    fn statfs(&mut self, _: &Request<'_>, _: u64, reply: ReplyStatfs) {
        match self.session.space() {
            Ok((total, free)) => reply.statfs(total, free, free, 0, 0, 4096, 255, 4096),
            Err(e) => reply.error(errno("statfs", e)),
        }
    }
}
pub fn mount(target: &Path, offset: u64, mountpoint: &Path, session_dir: &Path) -> Result<()> {
    crate::error_log::event(
        "info",
        "mount-rw",
        None,
        &format!(
            "target={} mountpoint={} recovery_session={}",
            target.display(),
            mountpoint.display(),
            session_dir.display()
        ),
    );
    let mp = std::fs::canonicalize(mountpoint)?;
    ensure!(
        mp.is_dir() && std::fs::read_dir(&mp)?.next().is_none(),
        "Mountpoint must be empty"
    );
    let mut session = if session_dir.exists() {
        let s = Session::resume_target(session_dir, target, offset)?;
        ensure!(
            !s.is_closed(),
            "Closed session: choose a new session directory"
        );
        s
    } else {
        Session::start(
            target,
            offset,
            session_dir,
            GROUP_BYTES,
            crate::journal::DEFAULT_RESERVE,
        )?
    };
    // Requesting a different target must never silently resume the old one.
    session.matches_target(target, offset)?;
    session.flush()?;
    let shutdown_failed = Arc::new(AtomicBool::new(false));
    let root = unsafe { libc::geteuid() } == 0;
    let uid = if root {
        std::env::var("SUDO_UID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    } else {
        unsafe { libc::getuid() }
    };
    let gid = if root {
        std::env::var("SUDO_GID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    } else {
        unsafe { libc::getgid() }
    };
    let host = Host {
        shutdown_failed: shutdown_failed.clone(),
        session,
        paths: crate::inode_paths::InodePaths::new(),
        handles: HashMap::new(),
        next: 1,
        uid,
        gid,
    };
    eprintln!(
        "LAPFS buffered RW beta: {}; durable queue {}; recover with mount-recover",
        mp.display(),
        session_dir.display()
    );
    let mut options = vec![
        MountOption::RW,
        MountOption::FSName("LAPFS-buffered".into()),
        MountOption::DefaultPermissions,
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::NoExec,
    ];
    if root && uid != 0 {
        options.push(MountOption::AllowOther);
    }
    fuser::mount2(host, &mp, &options)?;
    ensure!(
        !shutdown_failed.load(Ordering::SeqCst),
        "Unmount did not commit cleanly; retain session and run mount-recover"
    );
    Ok(())
}

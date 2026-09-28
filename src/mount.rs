//! Linux read-only FUSE host. The underlying Reader has no writable descriptor.
use crate::reader::{self, View};
use anyhow::{Context, Result, ensure};
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyOpen, ReplyStatfs, Request,
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};
const TTL: Duration = Duration::from_secs(1);
struct Host {
    view: View,
    paths: crate::inode_paths::InodePaths,
    opens: HashMap<u64, u64>,
    uid: u32,
    gid: u32,
}
fn kind(a: &apfs::Attr) -> FileType {
    match a.mode & 0xf000 {
        0x4000 => FileType::Directory,
        0xa000 => FileType::Symlink,
        0x1000 => FileType::NamedPipe,
        0x2000 => FileType::CharDevice,
        0x6000 => FileType::BlockDevice,
        0xc000 => FileType::Socket,
        _ => FileType::RegularFile,
    }
}
fn inode(a: &apfs::Attr) -> u64 {
    if a.inode == 2 { 1 } else { a.inode }
}
impl Host {
    fn attr(&mut self, p: &str) -> Result<FileAttr> {
        let a = self.view.getattr(p)?.context("Missing inode")?;
        Ok(FileAttr {
            ino: inode(&a),
            size: a.size,
            blocks: a.size.div_ceil(512),
            atime: UNIX_EPOCH + Duration::from_nanos(a.access_time),
            mtime: UNIX_EPOCH + Duration::from_nanos(a.mod_time),
            ctime: UNIX_EPOCH + Duration::from_nanos(a.change_time),
            crtime: UNIX_EPOCH + Duration::from_nanos(a.create_time),
            kind: kind(&a),
            perm: if a.is_dir { 0o500 } else { 0o400 },
            nlink: if a.is_dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        })
    }
    fn path(&self, ino: u64) -> Result<String> {
        self.paths
            .get(ino)
            .map(str::to_owned)
            .context("Unknown inode")
    }
    fn remember(&mut self, ino: u64, path: String) -> Result<()> {
        self.paths.remember(ino, path)
    }
}
impl Filesystem for Host {
    fn forget(&mut self, _: &Request<'_>, ino: u64, nlookup: u64) {
        self.paths
            .forget(ino, nlookup, self.opens.get(&ino).copied().unwrap_or(0) > 0);
    }
    fn lookup(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let result = (|| -> Result<FileAttr> {
            let name = name.to_str().context("Invalid UTF-8 filename")?;
            ensure!(
                !name.contains('/') && name != "." && name != "..",
                "Invalid name"
            );
            let p = format!("{}/{}", self.path(parent)?.trim_end_matches('/'), name);
            let a = self.attr(&p)?;
            self.remember(a.ino, p)?;
            Ok(a)
        })();
        match result {
            Ok(a) => reply.entry(&TTL, &a, 0),
            Err(e) => {
                eprintln!("LAPFS lookup: {e:#}");
                let code = e
                    .chain()
                    .find_map(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .and_then(|io| io.raw_os_error())
                    })
                    .unwrap_or(libc::ENOENT);
                reply.error(code)
            }
        }
    }
    fn getattr(&mut self, _: &Request<'_>, ino: u64, _: Option<u64>, reply: ReplyAttr) {
        match self.path(ino).and_then(|p| self.attr(&p)) {
            Ok(a) => reply.attr(&TTL, &a),
            Err(_) => reply.error(libc::EIO),
        }
    }
    fn open(&mut self, _: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        if flags & libc::O_ACCMODE != libc::O_RDONLY || flags & libc::O_TRUNC != 0 {
            reply.error(libc::EROFS);
            return;
        }
        match self.path(ino).and_then(|p| self.attr(&p)) {
            Ok(a) if a.kind == FileType::RegularFile => {
                let count = self.opens.get(&ino).copied().unwrap_or(0);
                if let Some(next) = count.checked_add(1) {
                    if self.opens.try_reserve(1).is_ok() {
                        self.opens.insert(ino, next);
                        reply.opened(0, 0);
                    } else {
                        reply.error(libc::ENOMEM);
                    }
                } else {
                    reply.error(libc::EOVERFLOW);
                }
            }
            Ok(_) => reply.error(libc::EISDIR),
            Err(_) => reply.error(libc::EIO),
        }
    }
    fn read(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        offset: i64,
        size: u32,
        _: i32,
        _: Option<u64>,
        reply: ReplyData,
    ) {
        if offset < 0 || size > 8 * 1024 * 1024 {
            reply.error(libc::EINVAL);
            return;
        }
        match self
            .path(ino)
            .and_then(|p| Ok(self.view.read_range(&p, offset as u64, size as usize)?))
        {
            Ok(b) => reply.data(&b),
            Err(e) => {
                eprintln!("LAPFS read: {e:#}");
                reply.error(libc::EIO)
            }
        }
    }
    fn release(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        _: i32,
        _: Option<u64>,
        _: bool,
        reply: fuser::ReplyEmpty,
    ) {
        if let Some(count) = self.opens.get_mut(&ino) {
            *count -= 1;
            if *count == 0 {
                self.opens.remove(&ino);
            }
        }
        self.paths
            .release_if_unreferenced(ino, self.opens.contains_key(&ino));
        reply.ok();
    }
    fn readlink(&mut self, _: &Request<'_>, ino: u64, reply: ReplyData) {
        match self
            .path(ino)
            .and_then(|p| self.view.read_symlink(&p)?.context("Not a symlink"))
        {
            Ok(b) => reply.data(&b),
            Err(_) => reply.error(libc::EIO),
        }
    }
    fn readdir(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let result = (|| -> Result<Vec<(u64, FileType, String)>> {
            let p = self.path(ino)?;
            ensure!(
                self.attr(&p)?.kind == FileType::Directory,
                "Not a directory"
            );
            let parent = p
                .rsplit_once('/')
                .map(|x| x.0)
                .filter(|x| !x.is_empty())
                .unwrap_or("/");
            let parent_ino = self.attr(parent)?.ino;
            let mut entries = vec![
                (ino, FileType::Directory, ".".into()),
                (parent_ino, FileType::Directory, "..".into()),
            ];
            for e in self.view.read_dir(&p)? {
                ensure!(
                    !e.name.contains('/') && e.name != "." && e.name != "..",
                    "Invalid catalog name"
                );
                let child = format!("{}/{}", p.trim_end_matches('/'), e.name);
                let a = self
                    .view
                    .getattr(&child)?
                    .context("Missing directory entry inode")?;
                entries.push((inode(&a), kind(&a), e.name));
            }
            Ok(entries)
        })();
        match result {
            Ok(entries) => {
                for (i, (id, k, name)) in entries.into_iter().enumerate().skip(offset as usize) {
                    if reply.add(id, (i + 1) as i64, k, name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(e) => {
                eprintln!("LAPFS readdir: {e:#}");
                reply.error(libc::EIO)
            }
        }
    }
    fn statfs(&mut self, _: &Request<'_>, _: u64, reply: ReplyStatfs) {
        // Shared-container free space cannot be inferred from per-volume allocated count.
        match self.view.volume_size() {
            Ok((total, _)) => reply.statfs(total / 4096, 0, 0, 0, 0, 4096, 255, 4096),
            Err(_) => reply.error(libc::EIO),
        }
    }
}
pub fn mount(source: &Path, offset: u64, mountpoint: &Path, volume: Option<usize>) -> Result<()> {
    let mountpoint = std::fs::canonicalize(mountpoint)?;
    ensure!(
        mountpoint.is_dir() && std::fs::read_dir(&mountpoint)?.next().is_none(),
        "Mount point must be an empty directory"
    );
    let view = reader::open(source, offset, volume)?;
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let host = Host {
        view,
        paths: crate::inode_paths::InodePaths::new(),
        opens: HashMap::new(),
        uid,
        gid,
    };
    eprintln!(
        "LAPFS read-only mount: {} (foreground; unmount with fusermount3 -u)",
        mountpoint.display()
    );
    fuser::mount2(
        host,
        &mountpoint,
        &[
            MountOption::RO,
            MountOption::FSName("LAPFS".into()),
            MountOption::DefaultPermissions,
            MountOption::NoDev,
            MountOption::NoSuid,
            MountOption::NoExec,
        ],
    )?;
    Ok(())
}

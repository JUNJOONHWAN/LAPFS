use crate::journal::{Device, Image, Overlay, BLOCK};
use anyhow::{ensure, Context, Result};
use apfs::FsView;
use apfs_core::block_device::{BlockDevice, BlockError, WritableBlockDevice};
use apfs_core::container::Container;
use apfs_write::{file, txn::Transaction};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MAX_INPUT: u64 = 8 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    WriteAt {
        source: PathBuf,
        path: String,
        offset: u64,
    },
    Truncate {
        path: String,
        size: u64,
    },
    Put {
        source: PathBuf,
        path: String,
    },
    Append {
        source: PathBuf,
        path: String,
        expected_size: u64,
        source_offset: u64,
        length: u64,
    },
    Mkdir {
        path: String,
    },
    Remove {
        path: String,
    },
    Rmdir {
        path: String,
    },
    Rename {
        path: String,
        name: String,
    },
}

pub struct Adapter<D: Device> {
    pub inner: D,
    base: u64,
    len: u64,
}
impl<D: Device> Adapter<D> {
    pub fn new(inner: D, base: u64, len: u64) -> Self {
        Self { inner, base, len }
    }
}
fn block_err(e: anyhow::Error) -> BlockError {
    BlockError::Io(std::io::Error::other(e.to_string()))
}
impl<D: Device> BlockDevice for Adapter<D> {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, off: u64, out: &mut [u8]) -> Result<(), BlockError> {
        if off
            .checked_add(out.len() as u64)
            .is_none_or(|n| n > self.len)
        {
            return Err(BlockError::OutOfRange {
                offset: off,
                len: out.len() as u64,
                size: self.len,
            });
        }
        self.inner.read(self.base + off, out).map_err(block_err)
    }
}
impl<D: Device> WritableBlockDevice for Adapter<D> {
    fn write_at(&mut self, off: u64, data: &[u8]) -> Result<(), BlockError> {
        if off
            .checked_add(data.len() as u64)
            .is_none_or(|n| n > self.len)
        {
            return Err(BlockError::OutOfRange {
                offset: off,
                len: data.len() as u64,
                size: self.len,
            });
        }
        self.inner.write(self.base + off, data).map_err(block_err)
    }
    fn flush_data(&mut self) -> Result<(), BlockError> {
        self.inner.flush().map_err(block_err)
    }
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Require an explicit container offset rather than heuristically trusting a GPT.
/// `inspect` reports a candidate GPT offset; preparation validates NXSB and bounds.
pub fn container_range<D: Device>(d: &mut D, base: u64) -> Result<u64> {
    ensure!(
        base % BLOCK as u64 == 0,
        "Container offset must be 4096-byte aligned"
    );
    let mut b = vec![0; BLOCK];
    d.read(base, &mut b)?;
    ensure!(
        &b[32..36] == b"NXSB" && u32le(&b, 36) == 4096,
        "Expected 4096-byte APFS container at --offset"
    );
    let size = u64le(&b, 40)
        .checked_mul(BLOCK as u64)
        .context("Container size overflow")?;
    ensure!(
        base.checked_add(size).is_some_and(|n| n <= d.len()),
        "Container exceeds image"
    );
    // Reject Fusion and unknown container incompatibilities, allowing version 2.
    ensure!(
        u64le(&b, 64) & !2 == 0,
        "Unsupported container incompatible features"
    );
    Ok(size)
}
pub fn inspect(path: &Path, offset: u64) -> Result<serde_json::Value> {
    crate::reader::inspect(path, offset)
}

fn path_parts(path: &str) -> Result<(String, String)> {
    ensure!(
        path.starts_with('/') && !path.ends_with('/'),
        "Use an absolute APFS file path"
    );
    let parts: Vec<_> = path[1..].split('/').collect();
    ensure!(!parts.is_empty(), "Root cannot be changed");
    for part in &parts {
        check_name(part)?;
    }
    let name = parts.last().unwrap().to_string();
    let parent = if parts.len() == 1 {
        "/".to_owned()
    } else {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    };
    Ok((parent, name))
}
fn check_name(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty() && s != "." && s != ".." && s.len() <= 255 && !s.contains(['/', '\0']),
        "Invalid APFS component"
    );
    Ok(())
}

pub fn prepare(
    image: &Path,
    dir: &Path,
    offset: u64,
    actions: &[Action],
    cap: u64,
    reserve: u64,
) -> Result<PathBuf> {
    let mut target = Image::open(image, false)?;
    target.ensure_no_pending()?;
    let dir = prepare_held(&mut target, dir, offset, actions, cap, reserve)?;
    target.bind_journal(&dir)?;
    Ok(dir)
}

/// The caller retains one exclusive FD and owns the persistent mount session.
/// Never writes the target during preparation; only the bounded overlay changes.
pub(crate) fn prepare_held(
    target: &mut Image,
    dir: &Path,
    offset: u64,
    actions: &[Action],
    cap: u64,
    reserve: u64,
) -> Result<PathBuf> {
    let identity = target.identity.clone();
    let overlay = stage_overlay(target, identity, dir, offset, actions, cap, reserve)?;
    let (_, dir) = overlay.finish()?;
    Ok(dir)
}

fn stage_overlay<D: Device>(
    mut target: D,
    identity: crate::journal::Identity,
    dir: &Path,
    offset: u64,
    actions: &[Action],
    cap: u64,
    reserve: u64,
) -> Result<Overlay<D>> {
    ensure!(
        !actions.is_empty() && actions.len() <= 64,
        "Batch requires 1..64 operations"
    );
    ensure!(
        identity.generation.is_none() || offset == 0,
        "Enrolled partition requires offset 0"
    );
    let len = container_range(&mut target, offset)?;
    let overlay = Overlay::new(
        target,
        identity,
        dir,
        cap,
        reserve,
        serde_json::to_string(actions)?,
    )?;
    let mut dev = Adapter {
        inner: overlay,
        base: offset,
        len,
    };
    let c = Container::open(&mut dev).map_err(|e| anyhow::anyhow!("APFS parse: {e:?}"))?;
    ensure!(
        c.superblock.fs_oids.len() == 1,
        "Multi-volume containers are not yet validated; preparation blocked"
    );
    for action in actions {
        let mut view = FsView::open(dev)?;
        ensure!(
            !view.volume_has_snapshots()?,
            "Snapshots require additional validation; preparation blocked"
        );
        let (vsb, omap) = view.read_vsb_omap_raw()?;
        ensure!(
            u64le(&vsb, 264) & 1 == 1,
            "Encrypted volume is not supported"
        );
        ensure!(
            u64le(&vsb, 56) & !0x9 == 0,
            "Unsupported volume features (including sealed volumes)"
        );
        ensure!(
            u64le(&vsb, 160) == 0 && u64le(&vsb, 168) == 0,
            "Pending APFS revert; preparation blocked"
        );
        let path = match action {
            Action::WriteAt { path, .. }
            | Action::Truncate { path, .. }
            | Action::Put { path, .. }
            | Action::Append { path, .. }
            | Action::Mkdir { path }
            | Action::Remove { path }
            | Action::Rmdir { path }
            | Action::Rename { path, .. } => path,
        };
        let (parent_path, name) = path_parts(path)?;
        // Do not traverse symbolic links in intermediate directories.
        let mut prefix = String::new();
        for part in parent_path.split('/').filter(|x| !x.is_empty()) {
            prefix.push('/');
            prefix.push_str(part);
            let a = view.getattr(&prefix)?.context("Parent missing")?;
            ensure!(
                a.is_dir && a.mode & 0xf000 == 0x4000,
                "Parent must be a real directory"
            );
        }
        let parent = view.getattr(&parent_path)?.context("Parent missing")?;
        ensure!(parent.is_dir, "Parent is not a directory");
        ensure!(
            parent.bsd_flags & 0x00060006 == 0,
            "Immutable/append-only parent"
        );
        let existing = view.getattr(path)?;
        if let Some(a) = existing {
            ensure!(a.bsd_flags & 0x00060006 == 0, "Immutable/append-only entry");
            if matches!(action, Action::Rmdir { .. }) {
                ensure!(a.is_dir && a.mode & 0xf000 == 0x4000, "Rmdir requires a directory");
                ensure!(view.read_dir(path)?.is_empty(), "Directory not empty");
            } else {
                ensure!(
                    view.plain_unshared_file(path)?,
                    "Only plain unshared regular files with supported metadata may be changed"
                );
            }
        }
        let mut content = None;
        match action {
            Action::Put { source, .. } | Action::WriteAt { source, .. } => {
                if let Some(a) = existing {
                    ensure!(
                        !a.is_dir && a.mode & 0xf000 == 0x8000,
                        "Put only replaces regular files"
                    );
                }
                let f = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(source)?;
                ensure!(f.metadata()?.is_file() && f.metadata()?.len() <= MAX_INPUT, "Input exceeds 8 MiB batch limit; streaming large-file writer is not implemented");
                let mut bytes = Vec::new();
                f.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() as u64 <= MAX_INPUT,
                    "Source grew beyond batch limit"
                );
                content = Some(bytes);
            }
            Action::Truncate { size, .. } => {
                let old = existing.context("Truncate target missing")?;
                ensure!(
                    *size <= MAX_INPUT,
                    "Truncate above 8 MiB is not supported by this beta"
                );
                let mut bytes = view.read_range(path, 0, old.size.min(*size) as usize)?;
                bytes.resize(*size as usize, 0);
                content = Some(bytes);
            }
            Action::Append {
                source,
                expected_size,
                source_offset,
                length,
                ..
            } => {
                if *expected_size == 0 {
                    ensure!(
                        existing.is_none(),
                        "Chunk creation destination already exists"
                    );
                } else {
                    let a = existing.context("Append target missing")?;
                    ensure!(
                        !a.is_dir && a.size == *expected_size && *expected_size % BLOCK as u64 == 0,
                        "Append offset does not match an aligned file end"
                    );
                }
                ensure!(
                    *length > 0 && *length <= MAX_INPUT,
                    "Append chunk must be 1..8 MiB"
                );
                let mut f = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(source)?;
                ensure!(
                    f.metadata()?.is_file()
                        && source_offset
                            .checked_add(*length)
                            .is_some_and(|end| end <= f.metadata().map(|m| m.len()).unwrap_or(0)),
                    "Invalid source chunk"
                );
                f.seek(SeekFrom::Start(*source_offset))?;
                let mut bytes = vec![0; *length as usize];
                f.read_exact(&mut bytes)?;
                content = Some(bytes);
            }
            Action::Mkdir { .. } => ensure!(existing.is_none(), "Entry already exists"),
            Action::Remove { .. } => {
                let a = existing.context("Entry missing")?;
                ensure!(!a.is_dir, "Directory removal not enabled");
            }
            Action::Rmdir { .. } => {
                let a = existing.context("Entry missing")?;
                ensure!(a.is_dir, "Rmdir requires a directory");
            }
            Action::Rename { name: new_name, .. } => {
                ensure!(existing.is_some(), "Entry missing");
                check_name(new_name)?;
                let dest = format!("{}/{new_name}", parent_path.trim_end_matches('/'));
                ensure!(
                    view.getattr(&dest)?.is_none(),
                    "Rename destination already exists"
                );
            }
        }
        let mut txn = Transaction::begin(view.into_dev())?;
        match action {
            Action::Put { .. } | Action::Truncate { .. } => {
                let b = content.as_ref().unwrap();
                if existing.is_some() {
                    file::overwrite_existing_file(&mut txn, &vsb, &omap, parent.inode, &name, b)?;
                } else if b.is_empty() {
                    file::create_file(&mut txn, &vsb, &omap, parent.inode, &name)?;
                } else {
                    file::write_file(&mut txn, &vsb, &omap, parent.inode, &name, b)?;
                }
            }
            Action::WriteAt { offset, .. } => {
                let old = existing.context("Write target missing")?;
                let b = content.as_ref().unwrap();
                ensure!(
                    !b.is_empty() && *offset <= old.size,
                    "Sparse or empty range write is not supported"
                );
                if old.size == 0 {
                    file::overwrite_existing_file(&mut txn, &vsb, &omap, parent.inode, &name, b)?;
                } else {
                    file::write_range_plain(
                        &mut txn,
                        &vsb,
                        &omap,
                        parent.inode,
                        &name,
                        old.size,
                        *offset,
                        b,
                    )?;
                }
            }
            Action::Append { expected_size, .. } => {
                if *expected_size == 0 {
                    file::write_file(
                        &mut txn,
                        &vsb,
                        &omap,
                        parent.inode,
                        &name,
                        content.as_ref().unwrap(),
                    )?;
                } else {
                    file::append_aligned(
                        &mut txn,
                        &vsb,
                        &omap,
                        parent.inode,
                        &name,
                        *expected_size,
                        content.as_ref().unwrap(),
                    )?;
                }
            }
            Action::Mkdir { .. } => {
                file::mkdir(&mut txn, &vsb, &omap, parent.inode, &name)?;
            }
            Action::Remove { .. } | Action::Rmdir { .. } => {
                file::unlink(&mut txn, &vsb, &omap, parent.inode, &name)?
            }
            Action::Rename { name: new_name, .. } => {
                file::rename(&mut txn, &vsb, &omap, parent.inode, &name, new_name, false)?
            }
        }
        dev = txn.commit()?;
        if let Some(b) = content {
            let mut check = FsView::open(dev)?;
            let a = check.getattr(path)?.context("Staged output missing")?;
            let start = match action {
                Action::Append { expected_size, .. } => *expected_size,
                Action::WriteAt { offset, .. } => *offset,
                _ => 0,
            };
            ensure!(
                a.size
                    == if matches!(action, Action::WriteAt { .. }) {
                        existing.unwrap().size.max(start + b.len() as u64)
                    } else {
                        start + b.len() as u64
                    }
                    && check.read_range(path, start, b.len())? == b,
                "Staged file content mismatch"
            );
            dev = check.into_dev();
        }
    }
    Ok(dev.inner)
}

/// Stage mutations over an O_RDONLY Reader, report their real journal budget,
/// and discard the scratch. Never produces a PREPARED/applicable transaction.
pub fn probe(
    source: &Path,
    offset: u64,
    actions: &[Action],
    parent: &Path,
    cap: u64,
) -> Result<serde_json::Value> {
    use std::os::unix::fs::MetadataExt;
    let reader = crate::reader::Reader::open(source)?;
    let path = std::fs::canonicalize(source)?;
    let m = std::fs::metadata(&path)?;
    let identity = crate::journal::Identity {
        path,
        size: reader.len(),
        dev: m.dev(),
        ino: m.ino(),
        mtime: m.mtime(),
        mtime_ns: m.mtime_nsec(),
        ctime: m.ctime(),
        ctime_ns: m.ctime_nsec(),
        generation: None,
    };
    let scratch = tempfile::Builder::new()
        .prefix("lapfs-readonly-probe-")
        .tempdir_in(parent)?;
    let dir = scratch.path().join("staging");
    let started = std::time::Instant::now();
    let mut report = match stage_overlay(
        reader,
        identity,
        &dir,
        offset,
        actions,
        cap,
        crate::journal::DEFAULT_RESERVE,
    ) {
        Ok(overlay) => {
            let stats = overlay.statistics();
            drop(overlay);
            serde_json::json!({"status":"passed","statistics":stats})
        }
        Err(e) => {
            serde_json::json!({"status":"refused","error":format!("{e:#}"),"partial_undo_bytes":dir.join("undo.bin").metadata().map(|m|m.len()).unwrap_or(0),"partial_redo_bytes":dir.join("redo.bin").metadata().map(|m|m.len()).unwrap_or(0)})
        }
    };
    let mut histogram = std::collections::BTreeMap::<String, u64>::new();
    if let Ok(mut f) = std::fs::File::open(dir.join("redo.bin")) {
        let mut page = [0u8; 4096];
        while f.read_exact(&mut page).is_ok() {
            let kind = u32::from_le_bytes(page[24..28].try_into().unwrap()) & 0xffff;
            let subtype = u32::from_le_bytes(page[28..32].try_into().unwrap()) & 0xffff;
            *histogram.entry(format!("{kind}/{subtype}")).or_default() += 1;
        }
    }
    report["redo_header_histogram"] = serde_json::json!(histogram);
    report["elapsed_seconds"] = serde_json::json!(started.elapsed().as_secs_f64());
    report["target_writes"] = serde_json::json!(0);
    report["access"] = serde_json::json!("O_RDONLY; temporary overlay only; no applicable journal");
    report["cap_bytes"] = serde_json::json!(cap);
    scratch.close()?;
    Ok(report)
}

pub fn read_file(image: &Path, offset: u64, path: &str) -> Result<Vec<u8>> {
    let mut view = crate::reader::open(image, offset, None)?;
    let a = view.getattr(path)?.context("No such entry")?;
    ensure!(
        a.size <= MAX_INPUT,
        "Read limited to 8 MiB; use streaming read for larger files"
    );
    let mut bytes = Vec::with_capacity(a.size as usize);
    crate::reader::copy(&mut view, path, &mut bytes)?;
    Ok(bytes)
}

/// Stream a complete file through SHA-256 without allocating its full contents.
pub fn file_digest(image: &Path, offset: u64, path: &str) -> Result<(u64, String)> {
    crate::reader::copy(
        &mut crate::reader::open(image, offset, None)?,
        path,
        &mut std::io::sink(),
    )
}

/// Hold the image lock while checking destination availability for an import.
pub(crate) fn require_absent(mut image: Image, offset: u64, path: &str) -> Result<Image> {
    let len = container_range(&mut image, offset)?;
    let mut view = FsView::open(Adapter {
        inner: image,
        base: offset,
        len,
    })?;
    ensure!(
        view.getattr(path)?.is_none(),
        "Import destination already exists; replacement is not enabled"
    );
    Ok(view.into_dev().inner)
}

/// Sequential read with bounded memory, including files larger than 4 GiB.
pub fn stream_file(
    image: &Path,
    offset: u64,
    path: &str,
    output: &mut impl std::io::Write,
) -> Result<()> {
    crate::reader::copy(&mut crate::reader::open(image, offset, None)?, path, output)?;
    Ok(())
}

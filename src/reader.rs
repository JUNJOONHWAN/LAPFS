//! Read-only intake for images and Linux block devices. No write-capable FD.
use crate::{
    apfs_batch::{container_range, Adapter},
    journal::{self, Device, Image},
};
use anyhow::{bail, ensure, Context, Result};
use apfs::{FsView, VolumeSelector};
use apfs_core::container::Container;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::{
    fs::OpenOptions,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    },
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

pub enum Reader {
    Image(Image),
    #[cfg(target_os = "linux")]
    Block {
        file: File,
        size: u64,
    },
}
impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let path =
            fs::canonicalize(path).with_context(|| format!("Cannot resolve {}", path.display()))?;
        let meta = fs::metadata(&path)?;
        if meta.is_file() {
            let image = Image::open(&path, false)?;
            image.ensure_no_pending()?;
            return Ok(Self::Image(image));
        }
        #[cfg(target_os = "linux")]
        if meta.file_type().is_block_device() {
            // O_EXCL on a Linux block device refuses a kernel-mounted/claimed device.
            // No O_RDWR: neither the parser nor FUSE can write through this handle.
            let file = OpenOptions::new().read(true)
                .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path).with_context(|| format!("Cannot open {} read-only. Permission denied requires device read permission (sudo or a targeted ACL); busy requires unmounting the device first", path.display()))?;
            ensure!(
                file.metadata()?.file_type().is_block_device()
                    && file.metadata()?.rdev() == meta.rdev(),
                "Device changed while opening"
            );
            journal::lock(&file)?;
            let mut size: u64 = 0;
            // BLKGETSIZE64, Linux asm-generic: unsigned long long pointer.
            if unsafe { libc::ioctl(file.as_raw_fd(), 0x80081272u64 as _, &mut size) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Cannot obtain block device capacity");
            }
            ensure!(
                size >= 4096 && size % 4096 == 0,
                "Invalid block device capacity"
            );
            let mut bootstrap = [0u8; 4096];
            file.read_exact_at(&mut bootstrap, 0)?;
            if &bootstrap[32..36] == b"NXSB" {
                crate::physical::pending_read_guard(&uuid(&bootstrap[72..88].try_into()?))?;
            }
            return Ok(Self::Block { file, size });
        }
        bail!(
            "Expected a regular image file or Linux block device; other special files are rejected"
        )
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Image(_) => "image",
            #[cfg(target_os = "linux")]
            Self::Block { .. } => "linux-block-device",
        }
    }
}
impl Device for Reader {
    fn len(&self) -> u64 {
        match self {
            Self::Image(i) => i.len(),
            #[cfg(target_os = "linux")]
            Self::Block { size, .. } => *size,
        }
    }
    fn read(&mut self, off: u64, bytes: &mut [u8]) -> Result<()> {
        ensure!(
            off.checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= self.len()),
            "Read outside source capacity"
        );
        match self {
            Self::Image(i) => i.read(off, bytes),
            #[cfg(target_os = "linux")]
            Self::Block { file, .. } => file
                .read_exact_at(bytes, off)
                .context("Block device read failed; unplugged or short read"),
        }
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<()> {
        bail!("Read-only source: writes are prohibited")
    }
    fn flush(&mut self) -> Result<()> {
        bail!("Read-only source has no write/flush path")
    }
}
pub fn uuid(b: &[u8; 16]) -> String {
    let s = b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &s[..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..]
    )
}
pub fn inspect(path: &Path, offset: u64) -> Result<Value> {
    let mut reader = Reader::open(path)?;
    let kind = reader.kind();
    let device_bytes = reader.len();
    let len = container_range(&mut reader, offset)?;
    let mut dev = Adapter::new(reader, offset, len);
    let c = Container::open(&mut dev).map_err(|e| anyhow::anyhow!("APFS container: {e:?}"))?;
    let volumes = c
        .list_volumes(&mut dev)
        .map_err(|e| anyhow::anyhow!("APFS volumes: {e:?}"))?;
    Ok(
        json!({"source":path,"source_kind":kind,"source_bytes":device_bytes,"offset":offset,"container_bytes":len,"container_uuid":uuid(&c.container_uuid()),"access":"read-only","volumes":volumes.iter().map(|v|json!({"index":v.index,"uuid":uuid(&v.uuid),"name":v.name,"role":format!("{:?}",v.role),"flags":v.features})).collect::<Vec<_>>() }),
    )
}
pub type View = FsView<Adapter<Reader>>;
pub fn open(path: &Path, offset: u64, volume: Option<usize>) -> Result<View> {
    let mut source = Reader::open(path)?;
    let len = container_range(&mut source, offset)?;
    let mut view = FsView::open_selected(
        Adapter::new(source, offset, len),
        volume.map(VolumeSelector::Index).unwrap_or_default(),
    )?;
    let (vsb, _) = view.read_vsb_omap_raw()?;
    ensure!(
        u64::from_le_bytes(vsb[264..272].try_into()?) & 1 == 1,
        "Encrypted APFS volumes are not supported by this beta"
    );
    Ok(view)
}
pub fn validate_path(path: &str) -> Result<()> {
    ensure!(
        path.starts_with('/')
            && !path.contains('\0')
            && path.split('/').all(|p| p != "." && p != ".."),
        "Use an absolute APFS path without . or .. components"
    );
    Ok(())
}
pub fn list(path: &Path, offset: u64, apfs_path: &str, volume: Option<usize>) -> Result<Value> {
    validate_path(apfs_path)?;
    let mut view = open(path, offset, volume)?;
    ensure!(
        view.getattr(apfs_path)?
            .context("Directory not found")?
            .is_dir,
        "Not a directory"
    );
    let mut entries = Vec::new();
    for e in view.read_dir(apfs_path)? {
        let child = format!("{}/{}", apfs_path.trim_end_matches('/'), e.name);
        let a = view
            .getattr(&child)?
            .context("Catalog entry has no inode")?;
        entries.push(json!({"name":e.name,"inode":a.inode,"bytes":a.size,"directory":a.is_dir,"mode":a.mode}));
    }
    Ok(json!({"path":apfs_path,"entries":entries}))
}
pub fn copy(view: &mut View, path: &str, output: &mut impl Write) -> Result<(u64, String)> {
    validate_path(path)?;
    let a = view.getattr(path)?.context("File not found")?;
    ensure!(
        a.mode & 0xf000 == 0x8000,
        "Expected a regular file; symlinks and special files are not followed"
    );
    let mut pos = 0;
    let mut hash = Sha256::new();
    while pos < a.size {
        let count = (a.size - pos).min(4 * 1024 * 1024) as usize;
        let b = view.read_range(path, pos, count)?;
        ensure!(
            b.len() == count,
            "Short APFS read; export was not published"
        );
        output.write_all(&b)?;
        hash.update(&b);
        pos += count as u64;
    }
    output.flush()?;
    Ok((pos, format!("{:x}", hash.finalize())))
}
/// No clobber. Stream, sync, reread/hash, then publish in the same directory.
pub fn export(
    source: &Path,
    offset: u64,
    path: &str,
    destination: &Path,
    volume: Option<usize>,
) -> Result<Value> {
    let mut view = open(source, offset, volume)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)?;
    let dest = parent.join(
        destination
            .file_name()
            .context("Destination needs a filename")?,
    );
    ensure!(!dest.try_exists()?, "Destination already exists");
    let mut temp = tempfile::Builder::new()
        .prefix(".lapfs-export-")
        .tempfile_in(&parent)?;
    let (bytes, sha) = copy(&mut view, path, &mut temp)?;
    journal::durable_sync(temp.as_file())?;
    let mut verify = File::open(temp.path())?;
    let mut hash = Sha256::new();
    let mut b = vec![0; 4 * 1024 * 1024];
    loop {
        let n = verify.read(&mut b)?;
        if n == 0 {
            break;
        }
        hash.update(&b[..n]);
    }
    ensure!(
        format!("{:x}", hash.finalize()) == sha,
        "Export readback hash mismatch"
    );
    // Hard-link publication never overwrites a concurrently created destination.
    fs::hard_link(temp.path(), &dest)
        .context("Cannot publish export without replacing an existing file")?;
    journal::sync_dir(&parent)?;
    temp.close()?;
    journal::sync_dir(&parent)?;
    Ok(json!({"state":"Exported","destination":dest,"bytes":bytes,"sha256":sha,"verified":true}))
}

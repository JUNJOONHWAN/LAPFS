//! Explicit, root-owned enrollment for OFFLINE Linux APFS partition writes.
//! Raw paths remain read-only. Writers use an enrolled target descriptor.
use crate::journal::{self, Identity};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
const STATE: &str = "/var/lib/lapfs/devices";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    format: String,
    pub device: PathBuf,
    pub container_uuid: String,
    pub size: u64,
    pub partition_start: u64,
    pub partition_uuid: String,
    checkpoint_base: u64,
    checkpoint_bytes: u64,
}
pub fn is_descriptor(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|x| x.to_string_lossy().ends_with(".lapfs-device.json"))
}
fn secure(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        m.uid() == 0 && m.mode() & 0o022 == 0 && !m.file_type().is_symlink(),
        "Device state must be root-owned and not writable by group/others"
    );
    ensure!(
        if directory {
            m.is_dir()
        } else {
            m.is_file() && m.nlink() == 1
        },
        "Unexpected device state type"
    );
    Ok(())
}
fn load(path: &Path) -> Result<Enrollment> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Enrolled device writes/recovery require sudo"
    );
    let p = fs::canonicalize(path)?;
    ensure!(
        p.parent().and_then(Path::parent) == Some(Path::new(STATE)),
        "Device descriptor must be in the LAPFS system registry"
    );
    for parent in p.ancestors().skip(1) {
        secure(parent, true)?;
    }
    secure(&p, false)?;
    ensure!(
        fs::metadata(&p)?.len() < 16384,
        "Device descriptor too large"
    );
    let e: Enrollment = journal::unseal(&p)?;
    ensure!(
        e.format == "LAPFS-DEVICE-BETA-1"
            && p.parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                == Some(&e.container_uuid),
        "Invalid device enrollment identity"
    );
    Ok(e)
}
fn size(file: &File) -> Result<u64> {
    let mut n = 0u64;
    if unsafe { libc::ioctl(file.as_raw_fd(), 0x80081272u64 as _, &mut n) } != 0 {
        return Err(std::io::Error::last_os_error()).context("BLKGETSIZE64");
    }
    ensure!(n >= 4096 && n % 4096 == 0, "Invalid device capacity");
    Ok(n)
}
fn kernel_identity(file: &File) -> Result<(u64, String)> {
    let m = file.metadata()?;
    let r = m.rdev();
    let sys = PathBuf::from(format!(
        "/sys/dev/block/{}:{}",
        libc::major(r),
        libc::minor(r)
    ));
    ensure!(
        sys.join("partition").exists(),
        "Only an APFS partition may be enrolled; whole-disk writes are prohibited"
    );
    let start = fs::read_to_string(sys.join("start"))?.trim().parse()?;
    let canonical = fs::canonicalize(format!("/dev/block/{}:{}", libc::major(r), libc::minor(r)))?;
    let mut ids = Vec::new();
    for entry in fs::read_dir("/dev/disk/by-partuuid")? {
        let p = entry?.path();
        if fs::canonicalize(&p).ok().as_ref() == Some(&canonical) {
            ids.push(p.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    ensure!(
        ids.len() == 1,
        "Cannot determine a unique GPT partition UUID"
    );
    Ok((start, ids.remove(0)))
}
fn open_node(e: &Enrollment, writable: bool) -> Result<File> {
    ensure!(
        e.device.parent() == Some(Path::new("/dev/disk/by-id")),
        "Enrollment requires a persistent /dev/disk/by-id partition path"
    );
    let node = fs::canonicalize(&e.device)?;
    let m = fs::metadata(&node)?;
    ensure!(
        m.file_type().is_block_device(),
        "Enrolled source is no longer a block device"
    );
    let f = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&node)
        .context("Cannot exclusively open device; unmount all users first")?;
    ensure!(
        f.metadata()?.rdev() == m.rdev(),
        "Device changed during open"
    );
    journal::lock(&f)?;
    ensure!(
        size(&f)? == e.size
            && kernel_identity(&f)? == (e.partition_start, e.partition_uuid.clone()),
        "Wrong/repartitioned/replaced device; refusing access"
    );
    Ok(f)
}
pub fn open(path: &Path, writable: bool) -> Result<(File, Identity)> {
    let e = load(path)?;
    let f = open_node(&e, writable)?;
    let identity = identity_for_fd(path, &f)?;
    Ok((f, identity))
}
pub(crate) fn identity_for_fd(path: &Path, f: &File) -> Result<Identity> {
    let e = load(path)?;
    let p = fs::canonicalize(path)?;
    let m = fs::metadata(&p)?;
    let mut b = vec![0; 4096];
    f.read_exact_at(&mut b, 0)?;
    let owner = p.with_file_name(format!(
        ".{}.spark-apfs-owner.json",
        p.file_name().unwrap().to_string_lossy()
    ));
    match apfs_core::nx::NxSuperblock::parse(&b) {
        Ok(nx) => ensure!(
            crate::reader::uuid(&nx.uuid) == e.container_uuid,
            "APFS UUID changed; refusing device access"
        ),
        Err(_) => ensure!(
            owner.exists(),
            "Invalid bootstrap; only recovery of an owned transaction is allowed"
        ),
    }
    // Fixed checkpoint geometry comes from the immutable enrollment, so a torn
    // bootstrap cannot redirect recovery reads to an arbitrary allocation.
    ensure!(
        e.checkpoint_bytes <= 16 * 1024 * 1024
            && e.checkpoint_base
                .checked_add(e.checkpoint_bytes)
                .is_some_and(|n| n <= e.size),
        "Invalid checkpoint enrollment range"
    );
    let mut checkpoints = vec![0; e.checkpoint_bytes as usize];
    f.read_exact_at(&mut checkpoints, e.checkpoint_base)?;
    b.extend_from_slice(&checkpoints);
    let generation = Some(journal::hash(&b));
    Ok(Identity {
        path: p,
        size: e.size,
        dev: m.dev(),
        ino: m.ino(),
        mtime: m.mtime(),
        mtime_ns: m.mtime_nsec(),
        ctime: m.ctime(),
        ctime_ns: m.ctime_nsec(),
        generation,
    })
}
pub fn enroll(device: &Path, expected_uuid: &str) -> Result<serde_json::Value> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "device-enroll requires sudo"
    );
    ensure!(
        expected_uuid.len() == 36
            && expected_uuid
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "Supply the exact container UUID from inspect"
    );
    ensure!(
        device.parent() == Some(Path::new("/dev/disk/by-id")),
        "Use the exact persistent /dev/disk/by-id/...-partN path"
    );
    let node = fs::canonicalize(device)?;
    ensure!(
        fs::metadata(&node)?.file_type().is_block_device(),
        "Expected block partition"
    );
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&node)?;
    journal::lock(&f)?;
    let size = size(&f)?;
    let (partition_start, partition_uuid) = kernel_identity(&f)?;
    let mut b = vec![0; 4096];
    f.read_exact_at(&mut b, 0)?;
    let nx = apfs_core::nx::NxSuperblock::parse(&b)
        .map_err(|e| anyhow::anyhow!("APFS bootstrap: {e:?}"))?;
    let uuid = crate::reader::uuid(&nx.uuid);
    ensure!(
        uuid == expected_uuid.to_ascii_lowercase(),
        "Container UUID does not match requested device"
    );
    ensure!(
        nx.block_size == 4096 && nx.block_count.checked_mul(4096).is_some_and(|n| n <= size),
        "Unsupported container geometry"
    );
    ensure!(
        nx.xp_desc_base >= 0 && nx.xp_desc_blocks > 0 && nx.xp_desc_blocks <= 4096,
        "Unsupported checkpoint layout"
    );
    let checkpoint_base = (nx.xp_desc_base as u64)
        .checked_mul(4096)
        .context("Checkpoint offset overflow")?;
    let checkpoint_bytes = nx.xp_desc_blocks as u64 * 4096;
    ensure!(
        checkpoint_base
            .checked_add(checkpoint_bytes)
            .is_some_and(|n| n <= size),
        "Checkpoint area exceeds device"
    );
    let e = Enrollment {
        format: "LAPFS-DEVICE-BETA-1".into(),
        device: device.into(),
        container_uuid: uuid.clone(),
        size,
        partition_start,
        partition_uuid,
        checkpoint_base,
        checkpoint_bytes,
    };
    let dir = Path::new(STATE).join(&uuid);
    ensure!(
        !dir.exists(),
        "Device is already enrolled; retain the existing descriptor and journals"
    );
    for p in [Path::new("/var/lib/lapfs"), Path::new(STATE)] {
        if !p.exists() {
            fs::create_dir(p)?;
            fs::set_permissions(p, fs::Permissions::from_mode(0o755))?;
        }
        secure(p, true)?;
    }
    ensure_durable_spool(Path::new(STATE), &e)?;
    fs::create_dir(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    journal::publish(&dir, "target.lapfs-device.json", &e)?;
    journal::sync_dir(Path::new(STATE))?;
    Ok(
        serde_json::json!({"state":"Enrolled","target":dir.join("target.lapfs-device.json"),"container_uuid":uuid,"device":device,"mode":"offline-batch-beta","physical_power_loss_certified":false}),
    )
}
fn ensure_durable_spool(dir: &Path, e: &Enrollment) -> Result<()> {
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    // Deliberately narrow: persistent Linux local filesystems with flush support.
    ensure!([0xef53,0x58465342,0x9123683e].contains(&(stat.f_type as u64)),"Physical-device journal requires persistent ext4/XFS; tmpfs, network, FUSE and multi-device filesystems are refused");
    let target = fs::metadata(fs::canonicalize(&e.device)?)?.rdev();
    fn physical_parent(dev: u64) -> Result<PathBuf> {
        let p = fs::canonicalize(format!(
            "/sys/dev/block/{}:{}",
            libc::major(dev),
            libc::minor(dev)
        ))
        .context("Cannot verify physical journal device")?;
        ensure!(!p.starts_with("/sys/devices/virtual"), "Physical journal storage must be a directly identified disk, not loop/dm/md/multi-device storage");
        if p.join("partition").exists() {
            Ok(p.parent().context("Missing disk parent")?.to_path_buf())
        } else {
            Ok(p)
        }
    }
    // The target may be a QA loop, but real journals must be on another
    // physical disk, including when someone mounts a sibling partition here.
    let spool_disk = physical_parent(fs::metadata(dir)?.dev())?;
    let target_sys = fs::canonicalize(format!(
        "/sys/dev/block/{}:{}",
        libc::major(target),
        libc::minor(target)
    ))?;
    let target_disk = if target_sys.join("partition").exists() {
        target_sys
            .parent()
            .context("Missing target disk parent")?
            .to_path_buf()
    } else {
        target_sys
    };
    ensure!(
        spool_disk != target_disk,
        "Recovery journal cannot live on the same physical disk as the APFS target"
    );
    Ok(())
}
pub fn validate_spool(target: &Path, parent: &Path) -> Result<()> {
    let e = load(target)?;
    let parent = fs::canonicalize(parent)?;
    ensure!(
        parent.starts_with(target.parent().context("Missing enrollment parent")?),
        "Physical-device journals/jobs must remain inside their root-owned enrollment directory"
    );
    for p in parent.ancestors() {
        secure(p, true)?;
    }
    ensure_durable_spool(&parent, &e)
}
pub fn pending_read_guard(uuid: &str) -> Result<()> {
    let dir = Path::new(STATE).join(uuid);
    match fs::metadata(&dir) {
        Ok(_) => {
            let owner = dir.join(".target.lapfs-device.json.spark-apfs-owner.json");
            match fs::symlink_metadata(owner) {
                Ok(_) => bail!("Device has a pending LAPFS transaction; recover before reading"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => {
                    return Err(e)
                        .context("Cannot inspect device recovery state; run read with sudo")
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

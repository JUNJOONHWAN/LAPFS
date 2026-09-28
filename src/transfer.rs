//! Resumable offline-image imports. Each 4 MiB chunk is independently undoable.
//! Publication is a final rename; an existing destination is never replaced.
use crate::{
    apfs_batch::{self, Action},
    journal::{self, Identity, Image, Journal, State},
};
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

const CHUNK: u64 = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Source {
    path: PathBuf,
    len: u64,
    dev: u64,
    ino: u64,
    mtime: i64,
    mn: i64,
    ctime: i64,
    cn: i64,
}
impl Source {
    fn inspect(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        let m = fs::metadata(&path)?;
        ensure!(m.is_file(), "Source must be a regular file");
        Ok(Self {
            path,
            len: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            mtime: m.mtime(),
            mn: m.mtime_nsec(),
            ctime: m.ctime(),
            cn: m.ctime_nsec(),
        })
    }
}
#[derive(Serialize, Deserialize)]
struct Pending {
    journal: PathBuf,
    end: u64,
    publish: bool,
    before: Identity,
}
#[derive(Serialize, Deserialize)]
struct Job {
    source: Source,
    source_sha256: String,
    image: Identity,
    offset: u64,
    destination: String,
    temporary: String,
    completed: u64,
    sequence: u64,
    pending: Option<Pending>,
    created: bool,
    done: bool,
    cap: u64,
    reserve: u64,
    last_receipt: Option<serde_json::Value>,
}
fn source_digest(source: &Source, len: u64) -> Result<String> {
    ensure!(
        Source::inspect(&source.path)? == *source && len <= source.len,
        "Source changed; preserve job and partial file"
    );
    let mut f = File::open(&source.path)?;
    let mut h = Sha256::new();
    let mut remaining = len;
    let mut b = vec![0; CHUNK as usize];
    while remaining > 0 {
        let n = remaining.min(CHUNK) as usize;
        f.read_exact(&mut b[..n])?;
        h.update(&b[..n]);
        remaining -= n as u64;
    }
    ensure!(
        Source::inspect(&source.path)? == *source,
        "Source changed during hashing"
    );
    Ok(format!("{:x}", h.finalize()))
}
fn save(dir: &Path, job: &Job) -> Result<()> {
    journal::publish(dir, "job.json", job)?;
    journal::fault_point("job-published");
    Ok(())
}

pub fn create(
    image: &Path,
    offset: u64,
    source: &Path,
    destination: &str,
    dir: &Path,
    cap: u64,
    reserve: u64,
) -> Result<()> {
    ensure!(
        destination.starts_with('/')
            && !destination.ends_with('/')
            && destination.split('/').skip(1).all(|s| !s.is_empty()
                && s != "."
                && s != ".."
                && s.len() <= 255
                && !s.contains('\0')),
        "Invalid destination"
    );
    let target = Image::open(image, false)?;
    target.ensure_no_pending()?;
    let target = apfs_batch::require_absent(target, offset, destination)?;
    let source = Source::inspect(source)?;
    ensure!(
        source.path != target.identity.path
            && !(source.dev == target.identity.dev && source.ino == target.identity.ino),
        "Source cannot be the target image"
    );
    let sha = source_digest(&source, source.len)?;
    #[cfg(target_os = "linux")]
    if target.identity.generation.is_some() {
        crate::physical::validate_spool(
            &target.identity.path,
            dir.parent().context("Job requires parent")?,
        )?;
    }
    fs::DirBuilder::new().mode(0o700).create(dir)?;
    let dir = fs::canonicalize(dir)?;
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("job.lock"))?;
    journal::lock(&guard)?;
    let unique = format!(
        "{}-{}-{}",
        dir.display(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let token = journal::hash(unique.as_bytes());
    let parent = destination.rsplit_once('/').unwrap().0;
    let job = Job {
        source,
        source_sha256: sha,
        image: target.identity.clone(),
        offset,
        destination: destination.into(),
        temporary: format!("{parent}/.spark-{}.partial", &token[..24]),
        completed: 0,
        sequence: 0,
        pending: None,
        created: false,
        done: false,
        cap,
        reserve,
        last_receipt: None,
    };
    save(&dir, &job)?;
    journal::sync_dir(dir.parent().unwrap())?;
    Ok(())
}

fn settle(dir: &Path, job: &mut Job) -> Result<()> {
    let Some(p) = &job.pending else {
        return Ok(());
    };
    if !p.journal.exists() {
        job.pending = None;
        save(dir, job)?;
        return Ok(());
    }
    let state: State = if p.journal.join("receipt.json").exists() {
        serde_json::from_value(journal::read_receipt(&p.journal)?["state"].clone())?
    } else {
        journal::unseal(&p.journal.join("state.json"))?
    };
    if state == State::Building {
        let image = Image::open(&job.image.path, false)?;
        image.check_identity(&p.before, true)?;
        journal::discard_building(&p.journal, &image)?;
        job.pending = None;
        save(dir, job)?;
        return Ok(());
    }
    let terminal;
    if p.journal.join("receipt.json").exists() {
        ensure!(
            matches!(state, State::Committed | State::RolledBack),
            "Nonterminal receipt"
        );
        journal::resume_cleanup(&p.journal)?;
        terminal = state;
    } else {
        let mut j = Journal::open(&p.journal)?;
        let mut image = Image::open(&job.image.path, true)?;
        image.check_identity(&j.manifest.identity, j.state == State::Prepared)?;
        if j.state == State::Prepared {
            // Recovery of a crash after PREPARED publication but before ownership publication.
            if image.check_journal(&j.dir).is_err() {
                image.bind_journal(&j.dir)?;
            }
            j.apply(&mut image)?;
        } else if matches!(j.state, State::Applying | State::Recovering) {
            image.check_journal(&j.dir)?;
            j.recover(&mut image)?;
        }
        image.release_journal(&j.dir)?;
        terminal = j.state.clone();
        j.cleanup()?;
    }
    job.last_receipt = Some(journal::read_receipt(&p.journal)?);
    if terminal == State::Committed {
        job.created = true;
        job.completed = p.end;
        if p.publish {
            job.done = true;
        }
    }
    job.pending = None;
    save(dir, job)?;
    prune_receipts(dir, job)
}

/// max_chunks permits a controlled pause; zero means continue to publication.
pub fn resume(dir: &Path, max_chunks: usize) -> Result<serde_json::Value> {
    let dir = fs::canonicalize(dir)?;
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("job.lock"))?;
    journal::lock(&guard)?;
    let mut job: Job = journal::unseal(&dir.join("job.json"))?;
    ensure!(
        Source::inspect(&job.source.path)? == job.source,
        "Source changed; cannot resume"
    );
    {
        let image = Image::open(&job.image.path, false)?;
        image.check_identity(&job.image, false)?;
    }
    settle(&dir, &mut job)?;
    prune_receipts(&dir, &job)?;
    if job.done {
        ensure!(
            apfs_batch::file_digest(&job.image.path, job.offset, &job.destination)?
                == (job.source.len, job.source_sha256.clone()),
            "Published file no longer matches receipt"
        );
        return Ok(
            serde_json::json!({"state":"Completed", "bytes":job.completed, "destination":job.destination, "sha256":job.source_sha256}),
        );
    }
    if job.completed > 0 {
        ensure!(
            apfs_batch::file_digest(&job.image.path, job.offset, &job.temporary)?
                == (job.completed, source_digest(&job.source, job.completed)?),
            "Partial file mismatch; refusing to append"
        );
    }
    let mut count = 0;
    loop {
        ensure!(
            Source::inspect(&job.source.path)? == job.source,
            "Source changed; import paused"
        );
        let publishing = job.completed == job.source.len && job.created;
        if !publishing && max_chunks > 0 && count >= max_chunks {
            break;
        }
        let end = if publishing {
            job.completed
        } else {
            job.completed.saturating_add(CHUNK).min(job.source.len)
        };
        let actions = if publishing {
            ensure!(
                source_digest(&job.source, job.source.len)? == job.source_sha256,
                "Source digest changed"
            );
            ensure!(
                apfs_batch::file_digest(&job.image.path, job.offset, &job.temporary)?
                    == (job.source.len, job.source_sha256.clone()),
                "Final file digest mismatch"
            );
            vec![Action::Rename {
                path: job.temporary.clone(),
                name: job.destination.rsplit('/').next().unwrap().into(),
            }]
        } else if job.source.len == 0 {
            vec![Action::Put {
                source: job.source.path.clone(),
                path: job.temporary.clone(),
            }]
        } else {
            vec![Action::Append {
                source: job.source.path.clone(),
                path: job.temporary.clone(),
                expected_size: job.completed,
                source_offset: job.completed,
                length: end - job.completed,
            }]
        };
        let before = {
            let image = Image::open(&job.image.path, false)?;
            image.ensure_no_pending()?;
            image.identity.clone()
        };
        let jp = dir.join(format!("chunk-{:08}", job.sequence));
        job.sequence += 1;
        job.pending = Some(Pending {
            journal: jp.clone(),
            end,
            publish: publishing,
            before,
        });
        save(&dir, &job)?;
        apfs_batch::prepare(
            &job.image.path,
            &jp,
            job.offset,
            &actions,
            job.cap,
            job.reserve,
        )?;
        settle(&dir, &mut job)?;
        count += 1;
        if job.done {
            break;
        }
    }
    Ok(
        serde_json::json!({"state": if job.done {"Completed"} else {"Paused"}, "bytes":job.completed, "total_bytes":job.source.len, "destination":job.destination, "temporary":job.temporary, "sha256":job.source_sha256, "chunk_bytes":CHUNK}),
    )
}

// Only terminal, source-linked chunk directories whose receipt was reflected in
// durable job progress may be reclaimed. Interrupted cleanup is repeatable.
fn prune_receipts(dir: &Path, job: &Job) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("chunk-") {
            continue;
        }
        let path = entry.path();
        if job.pending.as_ref().is_some_and(|p| p.journal == path) {
            continue;
        }
        ensure!(entry.file_type()?.is_dir(), "Unexpected chunk path");
        let Some(index) = name
            .strip_prefix("chunk-")
            .and_then(|s| s.parse::<u64>().ok())
        else {
            anyhow::bail!("Unexpected chunk name");
        };
        ensure!(index < job.sequence, "Future chunk exists");
        // An empty directory can remain after an interrupted terminal cleanup.
        let entries: Vec<_> = fs::read_dir(&path)?.collect::<std::io::Result<_>>()?;
        if !entries.is_empty() {
            // state.json and receipt.json may already have been removed, so the
            // durable job's no-pending progress is the authority for old indices.
            for e in &entries {
                ensure!(
                    ["lock", "state.json", "receipt.json"]
                        .contains(&e.file_name().to_str().unwrap_or(""))
                        && e.file_type()?.is_file(),
                    "Unexpected unretired chunk data; preserve it"
                );
            }
            for e in entries {
                fs::remove_file(e.path())?;
            }
        }
        fs::remove_dir(path)?;
    }
    journal::sync_dir(dir)
}

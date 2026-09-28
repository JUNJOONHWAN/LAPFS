//! Explicitly opted-in real APFS image test. Never opens a raw device.
use anyhow::{bail, Result};
use spark_apfs_safe::{
    apfs_batch::{self, Action},
    journal::{hash, Device, Image, Journal, Op, State},
};
use std::path::PathBuf;

struct FailIo {
    inner: Image,
    at: usize,
    n: usize,
}
impl Device for FailIo {
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn read(&mut self, o: u64, b: &mut [u8]) -> Result<()> {
        self.inner.read(o, b)
    }
    fn write(&mut self, o: u64, b: &[u8]) -> Result<()> {
        let fail = self.n == self.at;
        self.n += 1;
        if fail {
            self.inner.write(o, &b[..b.len() / 2])?;
            self.inner.flush()?;
            bail!("injected durable torn write");
        }
        self.inner.write(o, b)
    }
    fn flush(&mut self) -> Result<()> {
        let fail = self.n == self.at;
        self.n += 1;
        self.inner.flush()?;
        if fail {
            bail!("injected failure after durable flush");
        }
        Ok(())
    }
}

#[test]
#[ignore = "requires explicitly supplied disposable Mac APFS fixture"]
fn real_apfs_every_apply_boundary_restores_entire_image() {
    run_boundaries(false)
}
#[test]
#[ignore = "requires explicitly supplied disposable Mac APFS fixture"]
fn real_apfs_range_write_every_boundary_restores_entire_image() {
    run_boundaries(true)
}
fn run_boundaries(range: bool) {
    let fixture = PathBuf::from(std::env::var("SPARK_APFS_TEST_IMAGE").expect("fixture path"));
    let offset: u64 = std::env::var("SPARK_APFS_TEST_OFFSET")
        .expect("offset")
        .parse()
        .unwrap();
    assert!(fixture.is_file());
    let root = std::env::var_os("SPARK_APFS_TEST_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence"));
    let tmp = tempfile::tempdir_in(&root).unwrap();
    let image = tmp.path().join("faults.dmg");
    std::fs::copy(&fixture, &image).unwrap();
    if range {
        let mut mount = spark_apfs_safe::buffered::Session::start(
            &image,
            offset,
            &tmp.path().join("mount-init"),
            4 * 1024 * 1024,
            0,
        )
        .unwrap();
        mount.create("/range-check.txt").unwrap();
        mount
            .write("/range-check.txt", 0, &vec![0x37; 32771])
            .unwrap();
        mount.close().unwrap();
    }
    let original_hash = hash(&std::fs::read(&image).unwrap());
    let source = tmp.path().join("input.txt");
    std::fs::write(&source, vec![0xA7; 17013]).unwrap();
    let template = tmp.path().join("template");
    apfs_batch::prepare(
        &image,
        &template,
        offset,
        &[if range {
            Action::WriteAt {
                source,
                path: "/range-check.txt".into(),
                offset: 8190,
            }
        } else {
            Action::Put {
                source,
                path: "/fault-check.txt".into(),
            }
        }],
        8 * 1024 * 1024,
        0,
    )
    .unwrap();
    assert_eq!(hash(&std::fs::read(&image).unwrap()), original_hash);
    let j = Journal::open(&template).unwrap();
    let boundaries = j.manifest.ops.len() + 1;
    let writes = j
        .manifest
        .ops
        .iter()
        .filter(|o| matches!(o, Op::Write { .. }))
        .count();
    drop(j);
    for boundary in 0..boundaries {
        let path = tmp.path().join(format!("case-{boundary}"));
        std::fs::create_dir(&path).unwrap();
        for name in [
            "lock",
            "state.json",
            "manifest.json",
            "undo.bin",
            "redo.bin",
        ] {
            std::fs::copy(template.join(name), path.join(name)).unwrap();
        }
        let mut j = Journal::open(&path).unwrap();
        let mut faulty = FailIo {
            inner: Image::open(&image, true).unwrap(),
            at: boundary,
            n: 0,
        };
        assert!(j.apply(&mut faulty).is_err(), "boundary {boundary}");
        drop(faulty);
        drop(j);
        let mut j = Journal::open(&path).unwrap();
        let mut target = Image::open(&image, true).unwrap();
        target.check_identity(&j.manifest.identity, false).unwrap();
        j.recover(&mut target).unwrap();
        assert_eq!(j.state, State::RolledBack);
        drop(target);
        drop(j);
        assert_eq!(
            hash(&std::fs::read(&image).unwrap()),
            original_hash,
            "full-image restore at {boundary}"
        );
    }
    println!("APFS fault sweep (range={range}): {boundaries} boundaries ({writes} writes), full-image SHA-256 restored for every case");
}

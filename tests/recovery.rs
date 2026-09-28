use anyhow::{bail, Result};
use spark_apfs_safe::journal::{hash, Device, Identity, Image, Journal, Op, Overlay, State, BLOCK};
use std::path::PathBuf;

#[derive(Clone)]
struct Disk {
    durable: Vec<u8>,
    cache: Vec<u8>,
    event: usize,
    fail: Option<usize>,
    torn: bool,
    silent_drop: bool,
}
impl Disk {
    fn new() -> Self {
        let bytes: Vec<_> = (0..BLOCK * 12)
            .map(|i| ((i * 13 + i / 4096) % 251) as u8)
            .collect();
        Self {
            durable: bytes.clone(),
            cache: bytes,
            event: 0,
            fail: None,
            torn: false,
            silent_drop: false,
        }
    }
    fn crash(&mut self) {
        self.cache = self.durable.clone();
        self.event = 0;
        self.fail = None;
    }
    fn fails(&mut self) -> bool {
        let n = self.event;
        self.event += 1;
        self.fail == Some(n)
    }
}
impl Device for Disk {
    fn len(&self) -> u64 {
        self.cache.len() as u64
    }
    fn read(&mut self, off: u64, b: &mut [u8]) -> Result<()> {
        b.copy_from_slice(&self.cache[off as usize..off as usize + b.len()]);
        Ok(())
    }
    fn write(&mut self, off: u64, b: &[u8]) -> Result<()> {
        let off = off as usize;
        if self.silent_drop && off == 0 { return Ok(()); }
        if self.fails() {
            if self.silent_drop {
                return Ok(());
            }
            if self.torn {
                let n = b.len() / 2;
                self.cache[off..off + n].copy_from_slice(&b[..n]);
                self.durable[off..off + n].copy_from_slice(&b[..n]);
            }
            bail!("injected short write / device loss");
        }
        self.cache[off..off + b.len()].copy_from_slice(b);
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        if self.fails() {
            if self.torn {
                let n = self.cache.len() / 2;
                self.durable[..n].copy_from_slice(&self.cache[..n]);
            }
            bail!("injected flush failure");
        }
        self.durable.clone_from(&self.cache);
        Ok(())
    }
}

#[test]
fn acknowledged_but_dropped_write_is_detected_before_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("j");
    let (mut d, original) = staged(&p);
    let mut j = Journal::open(&p).unwrap();
    d.fail = None;
    d.silent_drop = true; // bootstrap block write has no later replacement
    assert!(j.apply(&mut d).is_err());
    assert_eq!(j.state, State::Applying);
    d.crash();
    d.silent_drop = false;
    j.recover(&mut d).unwrap();
    assert_eq!(d.durable, original);
}
fn identity(d: &Disk) -> Identity {
    Identity {
        path: PathBuf::from("model.img"),
        size: d.len(),
        dev: 0,
        ino: 1,
        mtime: 0,
        mtime_ns: 0,
        ctime: 0,
        ctime_ns: 0,
        generation: None,
    }
}
fn staged(dir: &std::path::Path) -> (Disk, Vec<u8>) {
    let d = Disk::new();
    let orig = d.durable.clone();
    let mut overlay =
        Overlay::new(d.clone(), identity(&d), dir, 1024 * 1024, 0, "model".into()).unwrap();
    // Non-address order, partial-page writes, repeat updates, crossing page boundary.
    overlay
        .write((BLOCK * 5 + 17) as u64, &vec![7; BLOCK + 130])
        .unwrap();
    overlay.write(0, &vec![9; BLOCK]).unwrap();
    overlay.flush().unwrap();
    overlay.write((BLOCK * 5 + 44) as u64, &[41; 93]).unwrap();
    overlay.flush().unwrap();
    let (d, _) = overlay.finish().unwrap();
    assert_eq!(d.durable, orig); // no writes reached target during staging
    (d, orig)
}

#[test]
fn every_apply_write_and_flush_failure_rolls_back_exactly() {
    for torn in [false, true] {
        for failure in 0..6 {
            // Adjacent first two pages coalesce: 3 writes, 2 recorded flushes, final flush
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("journal");
            let (mut disk, original) = staged(&path);
            let mut j = Journal::open(&path).unwrap();
            assert_eq!(j.manifest.ops.len() + 1, 7);
            disk.fail = Some(failure);
            disk.torn = torn;
            assert!(j.apply(&mut disk).is_err(), "failure={failure}");
            assert_eq!(j.state, State::Applying);
            drop(j);
            disk.crash();
            let mut j = Journal::open(&path).unwrap();
            j.recover(&mut disk).unwrap();
            assert_eq!(disk.durable, original, "failure={failure}, torn={torn}");
            assert_eq!(j.state, State::RolledBack);
            j.recover(&mut disk).unwrap();
            assert_eq!(disk.durable, original);
        }
    }
}
#[test]
fn rollback_can_itself_crash_at_every_write_and_flush() {
    for recovery_failure in 0..4 {
        // three unique changed pages + flush
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("j");
        let (mut d, original) = staged(&p);
        let mut j = Journal::open(&p).unwrap();
        d.fail = Some(5);
        d.torn = true;
        assert!(j.apply(&mut d).is_err());
        d.crash();
        d.fail = Some(recovery_failure);
        drop(j);
        let mut j = Journal::open(&p).unwrap();
        assert!(j.recover(&mut d).is_err());
        assert_eq!(j.state, State::Recovering);
        drop(j);
        d.crash();
        let mut j = Journal::open(&p).unwrap();
        j.recover(&mut d).unwrap();
        assert_eq!(d.durable, original);
    }
}
#[test]
fn success_is_durable_preserves_barriers_and_cannot_be_implicitly_undone() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("j");
    let (mut d, old) = staged(&p);
    let mut j = Journal::open(&p).unwrap();
    assert_eq!(
        j.manifest
            .ops
            .iter()
            .filter(|x| matches!(x, Op::Flush))
            .count(),
        2
    );
    j.apply(&mut d).unwrap();
    assert_eq!(j.state, State::Committed);
    assert_eq!(d.event, 6);
    let new = d.durable.clone();
    assert_ne!(new, old);
    d.crash();
    assert_eq!(new, d.cache);
    assert_eq!(&new[BLOCK..BLOCK * 5], &old[BLOCK..BLOCK * 5]);
    assert!(j.recover(&mut d).is_err());
    assert!(j.apply(&mut d).is_err());
}
#[test]
fn corrupt_undo_redo_and_metadata_each_block_all_target_writes() {
    for file in ["undo.bin", "redo.bin", "manifest.json", "state.json"] {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("j");
        let (_, _) = staged(&p);
        let path = p.join(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(path, bytes).unwrap();
        assert!(Journal::open(&p).is_err(), "{file}");
    }
}
#[test]
fn wrong_base_budget_and_space_rejections_leave_target_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("j");
    let (mut d, _) = staged(&p);
    d.cache[0] ^= 1;
    let before = d.cache.clone();
    let mut j = Journal::open(&p).unwrap();
    assert!(j.apply(&mut d).is_err());
    assert_eq!(d.cache, before);
    assert_eq!(d.event, 0);
    assert_eq!(j.state, State::Prepared);
    let d = Disk::new();
    let before = d.durable.clone();
    let mut o = Overlay::new(
        d.clone(),
        identity(&d),
        &tmp.path().join("cap"),
        (BLOCK * 2) as u64,
        0,
        "cap".into(),
    )
    .unwrap();
    assert!(o.write(0, &vec![8; BLOCK * 2]).is_err());
    assert_eq!(d.durable, before);
    assert!(Overlay::new(
        d.clone(),
        identity(&d),
        &tmp.path().join("space"),
        65536,
        u64::MAX,
        "space".into()
    )
    .is_err());
}
#[test]
fn locks_and_inode_binding_prevent_parallel_or_replaced_images() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("image");
    std::fs::write(&p, vec![0; BLOCK * 2]).unwrap();
    let image = Image::open(&p, false).unwrap();
    let old = image.identity.clone();
    assert!(Image::open(&p, true).is_err());
    drop(image);
    std::fs::rename(&p, tmp.path().join("old")).unwrap();
    std::fs::write(&p, vec![0; BLOCK * 2]).unwrap();
    assert!(Image::open(&p, true)
        .unwrap()
        .check_identity(&old, false)
        .is_err());
    let jp = tmp.path().join("j");
    staged(&jp);
    let j = Journal::open(&jp).unwrap();
    assert!(Journal::open(&jp).is_err());
    drop(j);
    assert!(Image::open(PathBuf::from("/dev/null").as_path(), true).is_err());
}
#[test]
fn undo_preflight_rejects_late_corruption_before_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("j");
    let (mut d, _) = staged(&p);
    let mut j = Journal::open(&p).unwrap();
    d.fail = Some(2);
    assert!(j.apply(&mut d).is_err());
    d.crash();
    drop(j);
    let mut j = Journal::open(&p).unwrap();
    let undo = p.join("undo.bin");
    let mut b = std::fs::read(&undo).unwrap();
    *b.last_mut().unwrap() ^= 1;
    std::fs::write(&undo, b).unwrap();
    assert!(j.recover(&mut d).is_err());
    assert_eq!(d.event, 0);
}
#[test]
fn page_read_your_writes_and_original_preservation() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("j");
    let d = Disk::new();
    let mut o = Overlay::new(
        d.clone(),
        identity(&d),
        &p,
        1024 * 1024,
        0,
        "partial".into(),
    )
    .unwrap();
    let mut expected = d.cache.clone();
    expected[4090..4190].fill(17);
    o.write(4090, &[17; 100]).unwrap();
    let mut got = vec![0; 8192];
    o.read(0, &mut got).unwrap();
    assert_eq!(got, expected[..8192]);
    let (d, _) = o.finish().unwrap();
    assert_eq!(hash(&d.durable), hash(&Disk::new().durable));
}

#[test]
fn persistent_ownership_and_terminal_cleanup() {
    use spark_apfs_safe::journal::{read_receipt, resume_cleanup};
    let tmp = tempfile::tempdir().unwrap();
    let image = tmp.path().join("image");
    std::fs::write(&image, vec![0; BLOCK * 2]).unwrap();
    let p = tmp.path().join("j");
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let target = Image::open(&image, false).unwrap();
    let id = target.identity.clone();
    let mut o = Overlay::new(target, id, &p, 65536, 0, "owner".into()).unwrap();
    o.write(0, &[19; BLOCK]).unwrap();
    let (target, _) = o.finish().unwrap();
    target.bind_journal(&p).unwrap();
    assert!(target.ensure_no_pending().is_err());
    assert!(target.check_journal(&other).is_err());
    drop(target);
    let mut j = Journal::open(&p).unwrap();
    let mut target = Image::open(&image, true).unwrap();
    target.check_journal(&p).unwrap();
    assert!(j.cleanup().is_err());
    j.apply(&mut target).unwrap();
    target.release_journal(&p).unwrap();
    j.cleanup().unwrap();
    drop(j);
    assert_eq!(read_receipt(&p).unwrap()["state"], "Committed");
    assert!(!p.join("undo.bin").exists());
    assert!(!p.join("redo.bin").exists());
    resume_cleanup(&p).unwrap();
    target.ensure_no_pending().unwrap();
}

#[test]
fn seeded_random_overlaps_flushes_and_crashes_256_workloads() {
    // Reproducible sequences exercise repeated pages, unaligned writes and
    // varying flush positions. This is a bounded randomized test, not a proof.
    for seed in 1..=256u64 {
        let mut rng = seed;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("j");
        let d = Disk::new();
        let original = d.durable.clone();
        let mut expected = original.clone();
        let mut o = Overlay::new(
            d.clone(),
            identity(&d),
            &p,
            4 * 1024 * 1024,
            0,
            format!("seed {seed}"),
        )
        .unwrap();
        for _ in 0..(8 + next() % 24) {
            let len = (next() % 6000 + 1) as usize;
            let offset = next() as usize % (expected.len() - len);
            let data = vec![(next() % 256) as u8; len];
            o.write(offset as u64, &data).unwrap();
            expected[offset..offset + len].copy_from_slice(&data);
            if next() % 3 == 0 {
                o.flush().unwrap();
            }
        }
        let (mut d, _) = o.finish().unwrap();
        let mut j = Journal::open(&p).unwrap();
        let events = j.manifest.ops.len() + 1;
        if seed % 4 != 0 {
            d.fail = Some(next() as usize % events);
            d.torn = seed % 2 == 0;
        }
        match j.apply(&mut d) {
            Ok(()) => {
                d.crash();
                assert_eq!(d.durable, expected, "seed {seed}");
            }
            Err(_) => {
                d.crash();
                j.recover(&mut d).unwrap();
                assert_eq!(d.durable, original, "seed {seed}");
            }
        }
    }
}

#[test]
fn image_aliases_cannot_bypass_persistent_ownership() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.img");
    std::fs::write(&path, vec![0; BLOCK]).unwrap();
    std::fs::hard_link(&path, tmp.path().join("alias.img")).unwrap();
    assert!(Image::open(&path, false).is_err());
    assert!(Image::open(&tmp.path().join("alias.img"), true).is_err());
}

#[test]
fn cli_adopts_prepared_orphan_only_when_target_is_pristine() {
    let tmp = tempfile::tempdir().unwrap();
    let image = tmp.path().join("orphan.img");
    std::fs::write(&image, vec![0; BLOCK * 2]).unwrap();
    let target = Image::open(&image, false).unwrap();
    let id = target.identity.clone();
    let p = tmp.path().join("j");
    let mut o = Overlay::new(target, id, &p, 1024 * 1024, 0, "orphan".into()).unwrap();
    o.write(19, b"prepared and durable").unwrap();
    let (target, _) = o.finish().unwrap();
    drop(target); // simulate death before owner publication
    let r = std::process::Command::new(
        std::env::var_os("SPARK_APFS_TEST_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_spark-apfs-safe").into()),
    )
    .args(["apply", p.to_str().unwrap()])
    .output()
    .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert_eq!(
        &std::fs::read(&image).unwrap()[19..39],
        b"prepared and durable"
    );
    assert_eq!(Journal::open(&p).unwrap().state, State::Committed);
}

#[test]
fn accelerated_hash_matches_standard_vectors() {
    assert_eq!(hash(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hash(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hash(&vec![97u8; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
}

#[test]
fn buffered_overlay_reads_disk_and_pending_tail_and_keeps_flush_order() {
    let tmp = tempfile::tempdir().unwrap();
    let mut disk=Disk::new();
    disk.durable=(0..BLOCK*600).map(|i|(i%251) as u8).collect();disk.cache=disk.durable.clone();
    let original=disk.durable.clone();let path=tmp.path().join("wide");
    let mut overlay=Overlay::new(disk.clone(),identity(&disk),&path,32*1024*1024,0,"wide".into()).unwrap();
    let input=vec![0x5a;BLOCK*550];overlay.write(0,&input).unwrap();
    let mut page=vec![0;BLOCK];overlay.read(0,&mut page).unwrap();assert_eq!(page,vec![0x5a;BLOCK]);
    overlay.read((BLOCK*549) as u64,&mut page).unwrap();assert_eq!(page,vec![0x5a;BLOCK]);
    overlay.flush().unwrap();overlay.write((BLOCK*520-9) as u64,&vec![0x3c;33]).unwrap();overlay.flush().unwrap();
    let (mut actual,_) = overlay.finish().unwrap();assert_eq!(actual.durable,original);
    let mut j=Journal::open(&path).unwrap();j.apply(&mut actual).unwrap();
    let mut expected=original;expected[..input.len()].copy_from_slice(&input);expected[BLOCK*520-9..BLOCK*520+24].fill(0x3c);
    assert_eq!(actual.durable,expected);
    assert!(actual.event < 20,"contiguous pages must use bounded bulk I/O");
}

#[test]
fn preimage_windows_preserve_originals_and_fallback_on_speculative_errors() {
    use std::{cell::Cell, rc::Rc};
    struct ReadDisk { disk: Disk, reads: Rc<Cell<usize>>, fail_large: bool, bad_page: Option<u64> }
    impl Device for ReadDisk {
        fn len(&self) -> u64 { self.disk.len() }
        fn read(&mut self, off:u64, bytes:&mut [u8]) -> Result<()> {
            self.reads.set(self.reads.get()+1);
            if self.fail_large && bytes.len()>BLOCK { bail!("speculative read refused"); }
            if self.bad_page.is_some_and(|p| off<=p && off+bytes.len() as u64>p) { bail!("requested block unreadable"); }
            self.disk.read(off,bytes)
        }
        fn write(&mut self,off:u64,bytes:&[u8])->Result<()> {self.disk.write(off,bytes)}
        fn flush(&mut self)->Result<()> {self.disk.flush()}
    }
    for fallback in [false,true] {
        let tmp=tempfile::tempdir().unwrap(); let path=tmp.path().join("journal");
        let mut disk=Disk::new();disk.durable=(0..BLOCK*600).map(|i|(i%251) as u8).collect();disk.cache=disk.durable.clone();
        let original=disk.durable.clone();let id=identity(&disk);let reads=Rc::new(Cell::new(0));
        let base=ReadDisk{disk,reads:reads.clone(),fail_large:fallback,bad_page:None};
        let mut o=Overlay::new(base,id,&path,16*1024*1024,0,"windows".into()).unwrap();
        let mut expected=original.clone();
        for page in 0..550 {
            let data=vec![(page%255) as u8;BLOCK]; o.write((page*BLOCK) as u64,&data).unwrap();
            expected[page*BLOCK..(page+1)*BLOCK].copy_from_slice(&data);
        }
        if !fallback {assert_eq!(reads.get(),4,"first page then sequential 1MiB windows");}
        o.flush().unwrap();o.write((BLOCK*256-7) as u64,b"overlap-crosses-window").unwrap();
        expected[BLOCK*256-7..BLOCK*256-7+b"overlap-crosses-window".len()].copy_from_slice(b"overlap-crosses-window");
        let mut observed=vec![0;expected.len()];o.read(0,&mut observed).unwrap();assert_eq!(observed,expected);
        let (mut d,_)=o.finish().unwrap();assert_eq!(d.disk.durable,original);
        // The grouped journal reader is independent of the staging cache.
        d.fail_large=false;
        let mut j=Journal::open(&path).unwrap();j.apply(&mut d).unwrap();assert_eq!(d.disk.durable,expected);
        // An unreadable requested page must still fail without a target write.
        drop(j);
        let tmp2=tempfile::tempdir().unwrap();let p2=tmp2.path().join("bad");
        let id=identity(&d.disk);d.bad_page=Some(BLOCK as u64);
        let mut bad=Overlay::new(d,id,&p2,1024*1024,0,"error".into()).unwrap();
        let mut page=vec![0;BLOCK];bad.read(0,&mut page).unwrap();
        assert!(bad.write(BLOCK as u64,&vec![1;BLOCK]).is_err());
    }
}

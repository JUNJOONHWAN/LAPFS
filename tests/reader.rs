use spark_apfs_safe::{
    journal::Device,
    reader::{self, Reader},
};
#[test]
fn read_only_handle_rejects_mutation_bounds_and_special_files() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("image");
    std::fs::write(&p, vec![0x42; 8192]).unwrap();
    let mut r = Reader::open(&p).unwrap();
    let mut b = [0; 3];
    r.read(4095, &mut b).unwrap();
    assert_eq!(b, [0x42; 3]);
    assert!(r.write(0, b"bad").is_err());
    assert!(r.flush().is_err());
    assert!(r.read(u64::MAX, &mut b).is_err());
    assert!(r.read(8191, &mut b).is_err());
    assert!(Reader::open(&p).is_err()); // simultaneous ownership
    assert!(Reader::open(std::path::Path::new("/dev/null")).is_err());
    drop(r);
    assert_eq!(std::fs::read(&p).unwrap(), vec![0x42; 8192]);
}
#[test]
fn pending_write_blocks_read_intake() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("image");
    std::fs::write(&p, vec![0; 8192]).unwrap();
    std::fs::write(d.path().join(".image.spark-apfs-owner.json"), b"pending").unwrap();
    assert!(Reader::open(&p).is_err());
}
#[test]
fn malformed_paths_and_container_are_rejected() {
    for p in ["relative", "/a/../b", "/./a", "/a\0b"] {
        assert!(reader::validate_path(p).is_err());
    }
    for p in ["/", "/한글/space name"] {
        reader::validate_path(p).unwrap();
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("image");
    std::fs::write(&p, vec![0; 8192]).unwrap();
    assert!(reader::inspect(&p, 0).is_err());
    assert!(reader::inspect(&p, u64::MAX).is_err());
}

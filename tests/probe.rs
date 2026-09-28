use spark_apfs_safe::{
    apfs_batch::{self, Action},
    journal::hash,
};
use std::path::PathBuf;
#[test]
#[ignore = "requires explicit disposable APFS fixture"]
fn readonly_probe_pass_and_cap_refusal_leave_entire_image_unchanged() {
    let source = PathBuf::from(std::env::var("SPARK_APFS_TEST_IMAGE").unwrap());
    let offset = std::env::var("SPARK_APFS_TEST_OFFSET")
        .unwrap()
        .parse()
        .unwrap();
    let out = PathBuf::from(std::env::var("SPARK_APFS_TEST_OUTPUT").unwrap());
    let work = tempfile::tempdir_in(out).unwrap();
    let image = work.path().join("probe.dmg");
    std::fs::copy(source, &image).unwrap();
    let payload = work.path().join("payload");
    std::fs::write(&payload, b"read-only probe").unwrap();
    let before = hash(&std::fs::read(&image).unwrap());
    for (cap, status) in [(8192, "refused"), (32 * 1024 * 1024, "passed")] {
        let report = apfs_batch::probe(
            &image,
            offset,
            &[Action::Put {
                source: payload.clone(),
                path: "/.probe-test".into(),
            }],
            work.path(),
            cap,
        )
        .unwrap();
        assert_eq!(report["status"], status);
        assert_eq!(report["target_writes"], 0);
        assert_eq!(hash(&std::fs::read(&image).unwrap()), before);
        for e in std::fs::read_dir(work.path()).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.starts_with("lapfs-readonly-probe-") && !name.contains("owner"));
        }
        spark_apfs_safe::reader::inspect(&image, offset).unwrap();
    }
}

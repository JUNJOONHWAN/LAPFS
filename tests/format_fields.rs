#[test]
fn container_uuid_uses_hex_48_not_decimal_48() {
    let mut b = vec![0u8; 4096];
    b[24..28].copy_from_slice(&0x40000001u32.to_le_bytes());
    b[32..36].copy_from_slice(b"NXSB");
    b[36..40].copy_from_slice(&4096u32.to_le_bytes());
    b[40..48].copy_from_slice(&10u64.to_le_bytes());
    b[48..64].fill(0xA5); // features are not an identity
    let uuid = [
        0x12u8, 0x23, 0x34, 0x45, 0x56, 0x67, 0x78, 0x89, 0x9a, 0xab, 0xbc, 0xcd, 0xde, 0xef, 0xf0,
        0x01,
    ];
    b[72..88].copy_from_slice(&uuid);
    let sum = apfs_core::checksum::fletcher64(&b);
    b[..8].copy_from_slice(&sum.to_le_bytes());
    let nx = apfs_core::nx::NxSuperblock::parse(&b).unwrap();
    assert_eq!(nx.uuid, uuid);
}

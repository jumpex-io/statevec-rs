use super::*;

#[test]
fn entry_digest_binds_exact_encoded_bytes_with_sha256() {
    // Standard SHA-256 vector: extraction must not change the wire commitment.
    let digest = EntryDigest::from_encoded_entry(b"abc");
    assert_eq!(
        digest.bytes(),
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23, 0xb0, 0x03,
            0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
        ]
    );
    assert_ne!(digest, EntryDigest::from_encoded_entry(b"abcd"));
}

#[test]
fn entry_digest_wire_projection_preserves_the_exact_commitment() {
    let digest = EntryDigest::from_encoded_entry(b"current-command-entry");
    assert_eq!(EntryDigest::from_untrusted_wire(digest.bytes()), digest);
}

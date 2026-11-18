use crate::delegation_key_fingerprint::{
    DelegationKeyFingerprint, DELEGATION_KEY_FINGERPRINT_HEADER,
};

#[test]
fn header_value_is_64_lowercase_hex_characters_of_the_fingerprint_bytes() {
    let mut bytes = [0u8; 32];
    bytes[0] = 0xE1;
    bytes[31] = 0x0A;
    let fingerprint = DelegationKeyFingerprint::from_bytes(bytes);

    let header_value = fingerprint.to_hex();

    assert_eq!(header_value.len(), 64);
    assert!(header_value.starts_with("e1"));
    assert!(header_value.ends_with("0a"));
    assert_eq!(header_value, header_value.to_ascii_lowercase());
    assert_eq!(hex::decode(&header_value).unwrap(), bytes.to_vec());
    assert_eq!(fingerprint.as_bytes(), &bytes);
}

#[test]
fn header_name_is_the_versioned_name_the_vote_chain_reads() {
    assert_eq!(
        DELEGATION_KEY_FINGERPRINT_HEADER,
        "x-vote-delegation-key-v1"
    );
}

#[test]
fn fingerprint_is_the_unkeyed_32_byte_blake2b_hash_of_the_debug_rendering() {
    #[derive(Debug)]
    #[allow(dead_code)]
    struct PinnedKey {
        commitments: [u8; 2],
    }
    let pinned_key = PinnedKey {
        commitments: [7, 9],
    };

    let fingerprint = DelegationKeyFingerprint::of_pinned_key(&pinned_key);

    let expected = blake2b_simd::Params::new()
        .hash_length(32)
        .hash(b"PinnedKey { commitments: [7, 9] }");
    assert_eq!(fingerprint.as_bytes().as_slice(), expected.as_bytes());
}

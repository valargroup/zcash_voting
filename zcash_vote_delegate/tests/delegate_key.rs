//! The delegate key, its `zvdk1` and `zvdsk1` strings and its fingerprint
//! (§11.2). The vectors pin the encodings and every decode failure.

use std::collections::BTreeSet;

use zcash_vote_delegate::{
    validate_ed25519_key, verify_zip215, DelegateFingerprint, DelegateSigningKey,
    FingerprintParseError,
};

/// `Debug` prints the public key and never the secret seed or its string.
#[test]
fn debug_output_hides_the_secret() {
    let signing_key = DelegateSigningKey::from_secret_seed([0x5a; 32]);
    let key_debug = format!("{signing_key:?}");
    assert!(!key_debug.contains(&hex::encode(signing_key.secret_seed())));
    assert!(!key_debug.contains(signing_key.export().as_str()));
    assert!(key_debug.contains(&signing_key.delegate_key().encode()));
}

/// A key signs digests that verify under ZIP-215, and its public key passes
/// the key rule.
#[test]
fn a_key_signs_digests_that_verify() {
    let signing_key = DelegateSigningKey::from_secret_seed([0x5a; 32]);
    let digest = [0x5a; 32];
    let signature = signing_key.sign_digest(&digest);
    let dk = signing_key.delegate_key().to_bytes();
    assert_eq!(validate_ed25519_key(&dk), Ok(()));
    assert_eq!(verify_zip215(&dk, &digest, &signature), Ok(()));
}

/// The contract's example fingerprint is the first 80 bits 00 01 … 09, and the
/// canonical display joins index and fingerprint with a middle dot.
#[test]
fn fingerprint_matches_the_contract_example() {
    let fingerprint = DelegateFingerprint::parse("qqqs-yqcy-q5rq-wzqf").unwrap();
    assert_eq!(fingerprint.to_string(), "qqqs-yqcy-q5rq-wzqf");
    assert_eq!(
        fingerprint.display_with_index(17),
        "#17 \u{b7} qqqs-yqcy-q5rq-wzqf"
    );
}

/// Fingerprints must be four lower-case groups of four bech32 characters.
#[test]
fn fingerprint_parsing_reports_each_failure() {
    for malformed in [
        "",
        "qqqsyqcyq5rqwzqf",
        "qqqs-yqcy-q5rq",
        "qqqs-yqcy-q5rq-wzqf-qqqs",
        "qqqs-yqcy-q5rq-wzq",
        "qqqs--yqcyq5rq-wzqf",
    ] {
        assert_eq!(
            DelegateFingerprint::parse(malformed),
            Err(FingerprintParseError::WrongFormat),
            "{malformed:?}"
        );
    }
    for invalid in [
        "QQQS-YQCY-Q5RQ-WZQF",
        "qqqs-yqcy-q5rq-wzqb",
        "qqqs-yqcy-q5rq-wzq1",
    ] {
        assert_eq!(
            DelegateFingerprint::parse(invalid),
            Err(FingerprintParseError::InvalidCharacter),
            "{invalid:?}"
        );
    }
    let distinct: BTreeSet<String> = (0..64u8)
        .map(|seed| {
            DelegateSigningKey::from_secret_seed([seed; 32])
                .delegate_key()
                .fingerprint()
                .to_string()
        })
        .collect();
    assert_eq!(distinct.len(), 64);
}

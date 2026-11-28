//! The delegate key, its `zvdk1` and `zvdsk1` strings and its fingerprint
//! (§11.2). The vectors pin the encodings and every decode failure.

use zcash_vote_delegate::{DelegateFingerprint, DelegateSigningKey, FingerprintParseError};

/// `Debug` prints the public key and never the secret seed or its string.
#[test]
fn debug_output_hides_the_secret() {
    let signing_key = DelegateSigningKey::from_secret_seed([0x5a; 32]);
    let key_debug = format!("{signing_key:?}");
    assert!(!key_debug.contains(&hex::encode(signing_key.secret_seed())));
    assert!(!key_debug.contains(signing_key.export().as_str()));
    assert!(key_debug.contains(&signing_key.delegate_key().encode()));
}

/// Fingerprints must be four lower-case groups of four bech32 characters.
#[test]
fn fingerprint_parsing_reports_each_failure() {
    assert_eq!(
        DelegateFingerprint::parse("qqqsyqcyq5rqwzqf"),
        Err(FingerprintParseError::WrongFormat)
    );
    assert_eq!(
        DelegateFingerprint::parse("QQQS-YQCY-Q5RQ-WZQF"),
        Err(FingerprintParseError::InvalidCharacter)
    );
}

//! Registration marker text (§11.5). The vectors pin each accepted and
//! refused text by its outer error.

use zcash_vote_delegate::{
    DelegateKeyDecodeError, DelegateMarker, DelegateSigningKey, MarkerError,
};

/// A marker whose key string does not decode reports why.
#[test]
fn a_refused_key_reports_its_cause() {
    let key = DelegateSigningKey::from_secret_seed([2; 32])
        .delegate_key()
        .encode();
    assert_eq!(
        DelegateMarker::parse(&format!("zcash-vote-delegate v1 {}", key.to_uppercase())),
        Err(MarkerError::InvalidKey(
            DelegateKeyDecodeError::NonCanonical
        ))
    );
}

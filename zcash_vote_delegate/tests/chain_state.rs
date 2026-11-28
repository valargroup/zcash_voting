//! The registered error table (§6). The vectors pin each error's name and
//! code; this pins the lookup clients classify answers with.

use zcash_vote_delegate::{ProxyChainError, VOTE_CODESPACE};

/// Clients key every rule on (codespace, code): the same code in another
/// codespace is a different error.
#[test]
fn errors_are_identified_by_codespace_and_code() {
    assert_eq!(
        ProxyChainError::from_codespace_and_code(VOTE_CODESPACE, 52),
        Some(ProxyChainError::ProxyBatchExpired)
    );
    assert_eq!(
        ProxyChainError::from_codespace_and_code("vote", 2),
        Some(ProxyChainError::DuplicateNullifier)
    );
    assert_eq!(ProxyChainError::from_codespace_and_code("sdk", 2), None);
    assert_eq!(ProxyChainError::from_codespace_and_code("vote", 56), None);
}

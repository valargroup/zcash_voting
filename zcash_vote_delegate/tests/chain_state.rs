//! Chain-state formats: the registry store keys (§3.2) and the registered
//! error table (§6). The vectors pin each key's bytes and each error's code.

use std::collections::BTreeSet;

use zcash_vote_delegate::{delegate_entry_store_key, ProxyChainError, VOTE_CODESPACE};

/// Big-endian entry keys sort in index order, so a key-paged scan returns
/// entries in ascending index.
#[test]
fn entry_keys_sort_by_index() {
    let indices = [1u32, 2, 255, 256, 65_536, 1 << 30];
    let keys: Vec<[u8; 6]> = indices
        .iter()
        .map(|d| delegate_entry_store_key(*d))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

/// Codes are unique, in ascending order, and named as vote-sdk registers them.
#[test]
fn error_table_is_unique_and_ordered() {
    let codes: Vec<u32> = ProxyChainError::ALL
        .iter()
        .map(|error| error.code())
        .collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(codes, sorted);
    let names: BTreeSet<&str> = ProxyChainError::ALL
        .iter()
        .map(|error| error.name())
        .collect();
    assert_eq!(names.len(), ProxyChainError::ALL.len());
    assert!(names.iter().all(|name| name.starts_with("Err")));
    // 50 is ErrClientCircuitKeyMismatch, and 56, 63 and 67 are unused.
    let new_codes: Vec<u32> = codes.iter().copied().filter(|code| *code >= 50).collect();
    let registered = (51..=69).filter(|code| ![56, 63, 67].contains(code));
    assert_eq!(new_codes, registered.collect::<Vec<_>>());
}

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
    assert_eq!(
        ProxyChainError::from_codespace_and_code("vote", 55),
        Some(ProxyChainError::DelegateRetired)
    );
    assert_eq!(
        ProxyChainError::from_codespace_and_code("vote", 62),
        Some(ProxyChainError::HandleTaken)
    );
    assert_eq!(ProxyChainError::from_codespace_and_code("vote", 8), None);
    assert_eq!(ProxyChainError::from_codespace_and_code("vote", 56), None);
    assert_eq!(
        ProxyChainError::from_codespace_and_code("vote", 69),
        Some(ProxyChainError::ProxyDelegationDisabled)
    );
    assert_eq!(ProxyChainError::from_codespace_and_code("vote", 50), None);
}

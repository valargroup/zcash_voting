//! Canonical handles and proof ids (§2.5), and the wallet list's reviewed
//! rule (§12.3). The vectors pin each canonical check.

use zcash_vote_delegate::{
    is_canonical_handle, normalize_handle, DelegateSigningKey, DelegateStatus, IdentityProvider,
    ProxyDelegateEntry, WalletListRow,
};

/// What a user types becomes canonical by trimming, lowercasing and dropping
/// one leading `@`, and nothing else.
#[test]
fn typed_handles_normalize_to_the_canonical_form() {
    assert_eq!(normalize_handle("  @Alice_01 \n"), "alice_01");
    assert_eq!(normalize_handle("Bob-Dev"), "bob-dev");
    assert_eq!(normalize_handle("@@alice"), "@alice");
    assert!(is_canonical_handle(
        IdentityProvider::X,
        &normalize_handle("@Alice_01")
    ));
    assert!(!is_canonical_handle(
        IdentityProvider::X,
        &normalize_handle("@@alice")
    ));
}

/// A row reviews only the active entry at its index with its key, provider
/// and handle.
#[test]
fn a_row_reviews_only_its_active_entry() {
    let entry = ProxyDelegateEntry {
        index: 17,
        dk: DelegateSigningKey::from_secret_seed([1; 32]).delegate_key(),
        provider: IdentityProvider::X,
        handle: "alice".to_owned(),
        proof_id: "1790000000000000000".to_owned(),
        status: DelegateStatus::Active,
        status_reason: 0,
        registered_height: 100,
    };
    let row = WalletListRow {
        index: 17,
        provider: 1,
        handle: "alice".to_owned(),
        dk: entry.dk.encode(),
        name: "Alice".to_owned(),
        statement: None,
        picture: None,
    };
    assert!(row.reviews(&entry));
    let other_key = DelegateSigningKey::from_secret_seed([2; 32]).delegate_key();
    let changed = [
        ProxyDelegateEntry {
            index: 18,
            ..entry.clone()
        },
        ProxyDelegateEntry {
            dk: other_key,
            ..entry.clone()
        },
        ProxyDelegateEntry {
            provider: IdentityProvider::GitHub,
            ..entry.clone()
        },
        ProxyDelegateEntry {
            handle: "alice2".to_owned(),
            ..entry.clone()
        },
        ProxyDelegateEntry {
            status: DelegateStatus::Suspended,
            ..entry.clone()
        },
        ProxyDelegateEntry {
            status: DelegateStatus::Retired,
            ..entry.clone()
        },
    ];
    for entry in &changed {
        assert!(!row.reviews(entry), "{entry:?}");
    }
}

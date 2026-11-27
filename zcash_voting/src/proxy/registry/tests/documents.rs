//! The documents a wallet fetches: the config extension's URL lists, and the
//! wallet list under its pinned keys and the newest-version rule.

use ed25519_dalek::SigningKey;
use serde_json::json;
use zcash_vote_delegate::WalletListError;

use super::fixtures::{
    entry, list_document, list_signer, pin, row, verified, wallet_list, PINNED_KEY_ID,
};
use crate::proxy::registry::{
    proxy_service_urls, resolve_wallet_list, ProxyServiceUrls, RegistryError, WalletListPin,
    WalletListUpdate,
};

/// The URL lists are read as published; an unknown member is ignored, and an
/// absent list is empty.
#[test]
fn the_config_extension_gives_its_url_lists() {
    let document = json!({
        "config_version": 1,
        "extensions": { "proxy_delegation_v1": {
            "proof_rpc_urls": ["https://rpc-a", "https://rpc-b"],
            "later": true,
        } },
    });
    assert_eq!(
        proxy_service_urls(document.to_string().as_bytes()),
        Ok(Some(ProxyServiceUrls {
            verifier_api_urls: Vec::new(),
            proof_rpc_urls: vec!["https://rpc-a".to_owned(), "https://rpc-b".to_owned()],
        }))
    );
    assert_eq!(
        proxy_service_urls(json!({ "extensions": {} }).to_string().as_bytes()),
        Ok(None)
    );
    assert!(matches!(
        proxy_service_urls(b"not json"),
        Err(RegistryError::InvalidInput { .. })
    ));
}

/// A list adopts only a verified document of a higher version; the same
/// version repeats or conflicts, a lower one is older, and a rejected one
/// keeps what was stored.
#[test]
fn the_newest_verified_wallet_list_is_kept() {
    let pin = pin();
    let first = wallet_list(1, vec![row(&entry(17, "alice"))], &["zcash"]);
    let second = wallet_list(2, vec![row(&entry(17, "alice"))], &[]);
    let document = |list| list_document(list, &list_signer(), PINNED_KEY_ID);

    let adopted = resolve_wallet_list(&pin, &document(&first), None).unwrap();
    assert_eq!(adopted.update, WalletListUpdate::Adopted);
    let stored = adopted.newest.unwrap();
    assert_eq!(stored, verified(&first));
    assert_eq!(stored.signed_by(), PINNED_KEY_ID);

    let unchanged = resolve_wallet_list(&pin, &document(&first), Some(&stored)).unwrap();
    assert_eq!(unchanged.update, WalletListUpdate::Unchanged);
    let newer = resolve_wallet_list(&pin, &document(&second), Some(&stored)).unwrap();
    assert_eq!(newer.update, WalletListUpdate::Adopted);
    assert_eq!(newer.newest, Some(verified(&second)));

    let second_stored = verified(&second);
    let older = resolve_wallet_list(&pin, &document(&first), Some(&second_stored)).unwrap();
    assert_eq!(older.update, WalletListUpdate::Older { version: 1 });
    assert_eq!(older.newest.as_ref(), Some(&second_stored));

    let swapped = wallet_list(2, vec![row(&entry(17, "mallory"))], &[]);
    let conflicting = resolve_wallet_list(&pin, &document(&swapped), Some(&second_stored)).unwrap();
    assert_eq!(conflicting.update, WalletListUpdate::ConflictingPayload);
    assert_eq!(conflicting.newest.as_ref(), Some(&second_stored));
}

/// Only a key the build pins counts, a list for another wallet or chain is
/// refused, and a stored list must be the pin's.
#[test]
fn a_wallet_list_verifies_only_under_the_pinned_key() {
    let pin = pin();
    let list = wallet_list(3, vec![row(&entry(17, "alice"))], &[]);
    let stored = verified(&wallet_list(1, Vec::new(), &[]));
    let other_key = SigningKey::from_bytes(&[10; 32]);
    for (case, document, error) in [
        (
            "another key under the pinned key id",
            list_document(&list, &other_key, PINNED_KEY_ID),
            WalletListError::NoPinnedSignature,
        ),
        (
            "the pinned key under another key id",
            list_document(&list, &list_signer(), "tachyon"),
            WalletListError::NoPinnedSignature,
        ),
        (
            "another wallet's list",
            list_document(
                &zcash_vote_delegate::WalletList {
                    wallet_id: "zodl".to_owned(),
                    ..list.clone()
                },
                &list_signer(),
                PINNED_KEY_ID,
            ),
            WalletListError::WrongWallet,
        ),
        (
            "another chain's list",
            list_document(
                &zcash_vote_delegate::WalletList {
                    chain_id: "svote-1".to_owned(),
                    ..list.clone()
                },
                &list_signer(),
                PINNED_KEY_ID,
            ),
            WalletListError::WrongChain,
        ),
    ] {
        let resolution = resolve_wallet_list(&pin, &document, Some(&stored)).unwrap();
        assert_eq!(
            resolution.update,
            WalletListUpdate::Rejected(error),
            "{case}"
        );
        assert_eq!(resolution.newest.as_ref(), Some(&stored), "{case}");
    }

    let stage_pin = WalletListPin {
        chain_id: "svote-1".to_owned(),
        ..pin
    };
    assert!(matches!(
        resolve_wallet_list(
            &stage_pin,
            &list_document(&list, &list_signer(), PINNED_KEY_ID),
            Some(&stored)
        ),
        Err(RegistryError::InvalidInput { .. })
    ));
}

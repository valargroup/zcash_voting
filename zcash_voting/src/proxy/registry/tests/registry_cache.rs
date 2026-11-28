//! The registry cache reads `GET /delegates` pages as the REST API encodes
//! them, refuses a page a server could use to skip or garble an entry, finds
//! entries by index, key, fingerprint and handle, and survives its serde
//! form.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use zcash_vote_delegate::{DelegateStatus, IdentityProvider, ProxyDelegateEntry};

use super::fixtures::{dk, entry, with_status, CHAIN_ID};
use crate::proxy::{
    delegate::ChainReadError,
    registry::{parse_registry_page, registry_page_path, RegistryCache, RegistryPage},
};

/// `entry` as `encoding/json` writes it, zero values omitted.
fn entry_json(entry: &ProxyDelegateEntry) -> Value {
    let mut value = json!({
        "index": entry.index,
        "dk": BASE64.encode(entry.dk.to_bytes()),
        "provider": entry.provider.code(),
        "handle": entry.handle,
        "proof_id": entry.proof_id,
        "registered_height": entry.registered_height,
        "added_later": "ignored",
    });
    if entry.status != DelegateStatus::Active {
        value["status"] = json!(entry.status.code());
    }
    if entry.status_reason != 0 {
        value["status_reason"] = json!(entry.status_reason);
    }
    value
}

/// A page body holding `entries`, continuing after `next`.
fn page_body(entries: &[Value], next: u32) -> Vec<u8> {
    json!({ "delegates": entries, "next_start_after": next })
        .to_string()
        .into_bytes()
}

#[test]
fn pages_read_entries_as_the_rest_api_encodes_them() {
    assert_eq!(
        registry_page_path(17),
        "/shielded-vote/v1/delegates?start_after=17&limit=1000"
    );
    let mut suspended = with_status(entry(9, "bob"), DelegateStatus::Suspended);
    suspended.status_reason = 4;
    let entries = vec![entry(5, "alice"), suspended];
    let body = page_body(&entries.iter().map(entry_json).collect::<Vec<_>>(), 9);
    let page = parse_registry_page(0, 200, &body).unwrap();
    assert_eq!(
        page,
        RegistryPage {
            entries: entries.clone(),
            next_start_after: Some(9),
        }
    );
    let last = parse_registry_page(9, 200, b"{}").unwrap();
    assert_eq!(last.entries, Vec::new());
    assert_eq!(last.next_start_after, None);
}

#[test]
fn a_page_that_could_skip_or_garble_an_entry_is_refused() {
    let entries = [entry_json(&entry(5, "alice")), entry_json(&entry(9, "bob"))];
    let edited = |field: &str, value: Value| {
        let mut edited = entry_json(&entry(7, "carol"));
        edited[field] = value;
        page_body(&[edited], 0)
    };
    for (case, start_after, body) in [
        (
            "an entry at or below start_after",
            5,
            page_body(&entries, 9),
        ),
        (
            "entries out of order",
            0,
            page_body(&[entries[1].clone(), entries[0].clone()], 5),
        ),
        ("a next page that skips entries", 0, page_body(&entries, 12)),
        ("an uppercase handle", 0, edited("handle", json!("Carol"))),
    ] {
        assert!(
            matches!(
                parse_registry_page(start_after, 200, &body),
                Err(ChainReadError::Malformed { .. })
            ),
            "{case}"
        );
    }
    assert_eq!(
        parse_registry_page(0, 503, b"{}"),
        Err(ChainReadError::Unavailable { status: 503 })
    );
}

#[test]
fn the_cache_finds_entries_by_index_key_and_fingerprint() {
    let mut cache = RegistryCache::new(CHAIN_ID);
    cache.insert_page(RegistryPage {
        entries: vec![entry(5, "alice"), entry(9, "bob")],
        next_start_after: None,
    });
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.entry(9).map(|entry| entry.index), Some(9));
    assert_eq!(
        cache.entry_with_key(&dk(5)).map(|entry| entry.index),
        Some(5)
    );
    assert_eq!(
        cache
            .entry_with_fingerprint(&dk(9).fingerprint())
            .map(|entry| entry.index),
        Some(9)
    );
    assert!(cache.entry_with_key(&dk(1)).is_none());

    // A refreshed entry replaces the cached one, key and all.
    cache.insert_page(RegistryPage {
        entries: vec![ProxyDelegateEntry {
            dk: dk(55),
            ..entry(5, "alice")
        }],
        next_start_after: None,
    });
    assert!(cache.entry_with_key(&dk(5)).is_none());
    assert_eq!(
        cache.entry_with_key(&dk(55)).map(|entry| entry.index),
        Some(5)
    );
}

/// A handle names its live entries: a retired entry no longer holds it, and
/// the same text on the other provider is another account.
#[test]
fn the_cache_finds_a_handle_s_live_entries() {
    let github = ProxyDelegateEntry {
        provider: IdentityProvider::GitHub,
        proof_id: "aa5a315d".to_owned(),
        ..entry(7, "alice")
    };
    let cache = RegistryCache::from_pages(
        CHAIN_ID,
        [RegistryPage {
            entries: vec![
                with_status(entry(5, "alice"), DelegateStatus::Retired),
                github,
                with_status(entry(9, "alice"), DelegateStatus::Suspended),
            ],
            next_start_after: None,
        }],
    );
    let found = |handle| -> Vec<u32> {
        cache
            .live_entries_with_handle(handle)
            .map(|entry| entry.index)
            .collect()
    };
    assert_eq!(found("alice"), [7, 9]);
    assert_eq!(found("bob"), [0u32; 0]);
}

#[test]
fn the_cache_round_trips_through_its_serde_form() {
    let cache = RegistryCache::from_pages(
        CHAIN_ID,
        [RegistryPage {
            entries: vec![
                entry(5, "alice"),
                with_status(entry(9, "bob"), DelegateStatus::Retired),
            ],
            next_start_after: None,
        }],
    );
    let stored = serde_json::to_string(&cache).unwrap();
    let restored: RegistryCache = serde_json::from_str(&stored).unwrap();
    assert_eq!(restored, cache);
    assert_eq!(restored.chain_id(), CHAIN_ID);
    assert!(
        serde_json::from_str::<RegistryCache>(r#"{"chain_id":"zvote-1","entries":["AAAA"]}"#)
            .is_err()
    );
}

//! Finding a delegate (contract §11.10): browse the wallet list, exact handle
//! search over the cached registry, and paste or scan.

use zcash_vote_delegate::{DelegateStatus, IdentityProvider, ProxyDelegateEntry};

use super::fixtures::{cache, dk, entry, picture, row, verified, wallet_list, with_status};
use crate::{
    proxy::registry::{PasteQuery, RegistryCache, RegistryView, VerifiedWalletList},
    wire::{
        DelegateEntryStatus, DelegateKeyInfo, DelegateLookup, ProxyDelegateListing,
        ProxyDelegateProvider,
    },
};

/// Entry 23 `bob` on GitHub.
fn github_bob() -> ProxyDelegateEntry {
    ProxyDelegateEntry {
        provider: IdentityProvider::GitHub,
        proof_id: "aa5a315d61ae9438".to_owned(),
        ..entry(23, "bob")
    }
}

/// The registry the wallet cached:
///
/// - 17 `alice` on X;
/// - 19 `bob` on X and 23 `bob` on GitHub, new this round;
/// - 31 `carol`, suspended;
/// - 40 `dave`, retired, and 41 `dave`, his new entry;
/// - 50 `alicee` and 52 `zcash`, which the wallet's list does not name.
fn registry() -> RegistryCache {
    cache(vec![
        entry(17, "alice"),
        entry(19, "bob"),
        github_bob(),
        with_status(entry(31, "carol"), DelegateStatus::Suspended),
        with_status(entry(40, "dave"), DelegateStatus::Retired),
        entry(41, "dave"),
        entry(50, "alicee"),
        entry(52, "zcash"),
    ])
}

/// The wallet's list: rows for 17, 23, 31 and 40, a row for 19 that reviewed
/// another key, and a row for 60, which the cache lacks.
fn list() -> VerifiedWalletList {
    let mut stale_key = row(&entry(19, "bob"));
    stale_key.dk = dk(119).encode();
    verified(&wallet_list(
        3,
        vec![
            row(&entry(17, "alice")),
            stale_key,
            row(&github_bob()),
            row(&entry(31, "carol")),
            row(&entry(40, "dave")),
            row(&entry(60, "erin")),
        ],
        &["zcash", "electriccoin"],
    ))
}

/// The view over `registry` and `list`, in a round whose first new entry is
/// 20.
fn view<'a>(registry: &'a RegistryCache, list: Option<&'a VerifiedWalletList>) -> RegistryView<'a> {
    RegistryView {
        cache: registry,
        wallet_list: list,
        first_new_index: Some(20),
    }
}

/// Entry 17 as the wallet's list shows it.
fn alice() -> ProxyDelegateListing {
    ProxyDelegateListing {
        index: 17,
        provider: ProxyDelegateProvider::X,
        handle: "alice".to_owned(),
        key: DelegateKeyInfo {
            dk: dk(17).encode(),
            fingerprint: dk(17).fingerprint().to_string(),
        },
        reviewed: true,
        display_name: Some("Delegate 17".to_owned()),
        statement: Some("Statement of 17.".to_owned()),
        picture: Some(picture()),
        new_this_round: false,
        status: DelegateEntryStatus::Active,
        lookalike_of: None,
    }
}

/// The listing a `Found` answer carries.
fn found(lookup: DelegateLookup) -> ProxyDelegateListing {
    match lookup {
        DelegateLookup::Found { listing } => listing,
        other => panic!("expected a found delegate, got {other:?}"),
    }
}

/// Browse shows, in list order, each row the cache holds an entry for,
/// with the row's content. Only an active entry with the row's key, provider
/// and handle is reviewed.
#[test]
fn browse_shows_the_list_rows_the_cache_holds_with_their_reviewed_status() {
    let (registry, list) = (registry(), list());
    let browse = view(&registry, Some(&list)).browse();
    assert!(!browse.list_unavailable);
    assert_eq!(browse.list_version, Some(3));
    let shown: Vec<(u32, bool, DelegateEntryStatus, bool)> = browse
        .listed
        .iter()
        .map(|listing| {
            (
                listing.index,
                listing.reviewed,
                listing.status,
                listing.new_this_round,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (17, true, DelegateEntryStatus::Active, false),
            (19, false, DelegateEntryStatus::Active, false),
            (23, true, DelegateEntryStatus::Active, true),
            (31, false, DelegateEntryStatus::Suspended, true),
            (40, false, DelegateEntryStatus::Retired, true),
        ]
    );
    assert_eq!(browse.listed[0], alice());
    assert!(browse
        .listed
        .iter()
        .all(|listing| listing.display_name == Some(format!("Delegate {}", listing.index))));
}

#[test]
fn without_a_verified_list_browse_is_empty_and_nothing_is_reviewed() {
    let registry = registry();
    let view = view(&registry, None);
    let browse = view.browse();
    assert!(browse.list_unavailable);
    assert_eq!(browse.listed, Vec::new());
    let listing = found(view.lookup("17"));
    assert!(!listing.reviewed);
    assert_eq!(listing.display_name, None);
    assert_eq!(listing.lookalike_of, None);
}

/// Search normalizes what the user typed and matches live entries' handles
/// exactly, on either provider.
#[test]
fn search_matches_a_normalized_handle_exactly_among_live_entries() {
    let (registry, list) = (registry(), list());
    let view = view(&registry, Some(&list));
    assert_eq!(view.search(" @Alice "), vec![alice()]);

    // One handle on both providers, in index order.
    let bobs: Vec<(u32, ProxyDelegateProvider)> = view
        .search("bob")
        .iter()
        .map(|listing| (listing.index, listing.provider))
        .collect();
    assert_eq!(
        bobs,
        [
            (19, ProxyDelegateProvider::X),
            (23, ProxyDelegateProvider::GitHub)
        ]
    );

    // A retired entry no longer holds its handle; a suspended one does.
    let indexes = |query: &str| -> Vec<u32> {
        view.search(query)
            .iter()
            .map(|listing| listing.index)
            .collect()
    };
    assert_eq!(indexes("dave"), [41]);
    assert_eq!(indexes("carol"), [31]);

    // Exact only, and nothing for text that is no handle.
    assert_eq!(indexes("alic"), [0u32; 0]);
    assert_eq!(indexes("ali ce"), [0u32; 0]);
    assert_eq!(indexes(""), [0u32; 0]);
}

/// A delegate whose handle looks like a wallet list handle or a reserved
/// name says so. A list row is not warned about its own handle, but another
/// entry with that handle is.
#[test]
fn a_lookalike_of_a_list_handle_or_reserved_name_is_flagged() {
    let (registry, list) = (registry(), list());
    let view = view(&registry, Some(&list));
    assert_eq!(view.search("alice")[0].lookalike_of, None);
    assert_eq!(
        view.search("alicee")[0].lookalike_of.as_deref(),
        Some("alice")
    );
    assert_eq!(
        view.search("zcash")[0].lookalike_of.as_deref(),
        Some("zcash")
    );
    // 41 `dave` is not the list's row for 40 `dave`.
    assert_eq!(view.search("dave")[0].lookalike_of.as_deref(), Some("dave"));
}

#[test]
fn a_paste_names_a_marker_key_index_or_fingerprint() {
    let key = dk(17);
    let fingerprint = key.fingerprint().to_string();
    assert_eq!(
        PasteQuery::parse(&format!("  zcash-vote-delegate v1 {key}\n")),
        Some(PasteQuery::Key(key))
    );
    assert_eq!(PasteQuery::parse(&key.encode()), Some(PasteQuery::Key(key)));
    assert_eq!(PasteQuery::parse("17"), Some(PasteQuery::Index(17)));
    assert_eq!(PasteQuery::parse("#17"), Some(PasteQuery::Index(17)));
    for form in [
        fingerprint.clone(),
        fingerprint.replace('-', ""),
        fingerprint.to_uppercase(),
    ] {
        assert_eq!(
            PasteQuery::parse(&form),
            Some(PasteQuery::Fingerprint(key.fingerprint())),
            "{form}"
        );
    }
    for text in ["", "0", "017", "#0", "alice", "4294967296", "zvdk1qqqq"] {
        assert_eq!(PasteQuery::parse(text), None, "{text:?}");
    }
}

/// A paste is answered from the cache alone: a key or fingerprint finds the
/// entry with that key, an index its entry, and what the cache lacks is not
/// found.
#[test]
fn a_paste_finds_the_cached_entry_it_names() {
    let (registry, list) = (registry(), list());
    let view = view(&registry, Some(&list));
    for text in [
        dk(17).encode(),
        format!("zcash-vote-delegate v1 {}", dk(17)),
        dk(17).fingerprint().to_string(),
        "#17".to_owned(),
    ] {
        assert_eq!(found(view.lookup(&text)), alice(), "{text}");
    }
    assert_eq!(
        found(view.lookup("40")).status,
        DelegateEntryStatus::Retired
    );

    assert_eq!(view.lookup(&dk(99).encode()), DelegateLookup::NotFound);
    assert_eq!(view.lookup("99"), DelegateLookup::NotFound);
    assert_eq!(
        view.lookup(&dk(99).fingerprint().to_string()),
        DelegateLookup::NotFound
    );
    assert_eq!(view.lookup("alice"), DelegateLookup::Unrecognized);
}

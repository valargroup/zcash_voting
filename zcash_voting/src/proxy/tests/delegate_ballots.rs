//! The round's delegate ballot list is read page by page from the bytes the
//! host fetched: pages chain by key, carry only the round's ballots in
//! ascending order, and a page that breaks the chain leaves the list as it
//! was.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::json;

use super::fixtures::ROUND_ID;
use crate::{
    proxy::DelegateBallotList,
    wire::{ProxyDelegationError, ProxyDelegationErrorKind},
};

/// The fixture round's id as the REST API encodes it.
fn round_b64(round_id: &str) -> String {
    BASE64_STANDARD.encode(hex::decode(round_id).unwrap())
}

/// One `DelegateBallotRecord` of `round_id`.
fn ballot(round_id: &str, delegate_index: u32, options: &[u32]) -> serde_json::Value {
    json!({
        "vote_round_id": round_b64(round_id),
        "delegate_index": delegate_index,
        "options": options,
        "height": 40,
    })
}

/// A page holding `ballots` of the fixture round, continuing after `next`.
fn page(ballots: &[(u32, &[u32])], next: u32) -> Vec<u8> {
    let ballots: Vec<_> = ballots
        .iter()
        .map(|(delegate_index, options)| ballot(ROUND_ID, *delegate_index, options))
        .collect();
    let mut page = json!({ "ballots": ballots });
    if next != 0 {
        page["next_start_after"] = json!(next);
    }
    page.to_string().into_bytes()
}

/// Whether `result` failed as an ordinary malformed input.
fn refused(result: Result<(), ProxyDelegationError>) -> bool {
    matches!(
        result,
        Err(ProxyDelegationError {
            kind: ProxyDelegationErrorKind::Other,
            ..
        })
    )
}

#[test]
fn pages_chain_by_key_until_one_names_no_successor() {
    let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
    assert_eq!(list.next_start_after(), Some(0));

    list.add_page(&page(&[(2, &[1, 0]), (5, &[0, 2])], 5))
        .unwrap();
    assert_eq!(list.next_start_after(), Some(5));
    assert!(!list.is_complete());
    list.add_page(&page(&[(9, &[3, 3])], 0)).unwrap();

    assert!(list.is_complete());
    assert_eq!(list.next_start_after(), None);
    assert_eq!(list.options(2), Some(&[1, 0][..]));
    assert_eq!(list.options(5), Some(&[0, 2][..]));
    assert_eq!(list.options(9), Some(&[3, 3][..]));
    assert_eq!(list.options(3), None);
    assert!(refused(list.add_page(&page(&[], 0))), "a complete list");
}

#[test]
fn an_empty_page_with_every_field_omitted_completes_an_empty_list() {
    let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
    list.add_page(b"{}").unwrap();
    assert!(list.is_complete());
    assert_eq!(list.options(1), None);
}

#[test]
fn fields_the_contract_does_not_define_are_ignored() {
    let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
    let mut record = ballot(ROUND_ID, 4, &[1]);
    record["signature"] = json!("ignored");
    list.add_page(
        json!({ "ballots": [record], "total": 1 })
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(list.options(4), Some(&[1][..]));
}

#[test]
fn a_page_that_breaks_the_chain_leaves_the_list_unchanged() {
    let other_round = "02".repeat(32);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not JSON", b"ballots".to_vec()),
        (
            "another round's ballot",
            json!({ "ballots": [ballot(&other_round, 6, &[1])] })
                .to_string()
                .into_bytes(),
        ),
        ("not above start_after", page(&[(3, &[1])], 0)),
        ("descending", page(&[(8, &[1]), (7, &[1])], 0)),
        ("a repeated delegate", page(&[(8, &[1]), (8, &[1])], 0)),
        ("no options", page(&[(8, &[])], 0)),
        (
            "a cursor that is not the last delegate",
            page(&[(8, &[1])], 9),
        ),
        ("a cursor on an empty page", page(&[], 4)),
    ];
    for (case, bytes) in cases {
        let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
        list.add_page(&page(&[(3, &[0])], 3)).unwrap();
        let before = list.clone();

        assert!(refused(list.add_page(&bytes)), "{case}");
        assert_eq!(list, before, "{case}");
    }
}

#[test]
fn a_page_holds_at_most_a_thousand_ballots() {
    let ballots: Vec<(u32, &[u32])> = (1..=1_001).map(|index| (index, &[1][..])).collect();
    let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
    assert!(refused(list.add_page(&page(&ballots, 0))));
    list.add_page(&page(&ballots[..1_000], 1_000)).unwrap();
    assert_eq!(list.next_start_after(), Some(1_000));
}

#[test]
fn a_list_needs_a_canonical_round_id() {
    assert!(DelegateBallotList::new("01").is_err());
    assert!(DelegateBallotList::new(&"0A".repeat(32)).is_err());
}

/// The delegator's list and the delegate's page reader trust a page on the
/// same rules, since both read it with one parser: a cursor that skips past
/// the page's last delegate, too many options and too many ballots are
/// refused by both, and a valid page reads the same in both.
#[test]
fn the_delegator_and_the_delegate_read_a_page_alike() {
    let round: [u8; 32] = hex::decode(ROUND_ID).unwrap().try_into().unwrap();
    let many: Vec<(u32, &[u32])> = (1..=1_001).map(|index| (index, &[1][..])).collect();
    let fifty_one = [0u32; 51];
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("valid", page(&[(2, &[1, 0]), (5, &[0, 2])], 5), true),
        ("a skipping cursor", page(&[(2, &[1])], 9), false),
        ("51 options", page(&[(2, &fifty_one)], 0), false),
        ("1,001 ballots", page(&many, 0), false),
    ];
    for (case, bytes, valid) in cases {
        let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
        let listed = list.add_page(&bytes);
        let read = crate::proxy::delegate::parse_delegate_ballots_page(&round, 0, 200, &bytes);
        assert_eq!(listed.is_ok(), valid, "{case}");
        assert_eq!(read.is_ok(), valid, "{case}");
        if let Ok(read) = read {
            assert_eq!(list.next_start_after(), read.next_start_after, "{case}");
            for ballot in &read.ballots {
                assert_eq!(
                    list.options(ballot.delegate_index),
                    Some(ballot.options.as_slice()),
                    "{case}"
                );
            }
        }
    }
}

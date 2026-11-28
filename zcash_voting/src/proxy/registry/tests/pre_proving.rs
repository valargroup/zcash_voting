//! The pre-proving check (contract §11.11): one evidence answer proves the
//! chosen entries, and each choice gets the first verdict that applies, with
//! a confirm listing from the proven entry.

use zcash_vote_delegate::{DelegateStatus, IdentityProvider, ProxyDelegateEntry};

use super::fixtures::{
    dk, entry, entry_proof, proven, row, verified, wallet_list, with_status, ROUND_ID,
};
use crate::{
    proxy::registry::{
        check_delegates, confirm_proof_requests, ConfirmContext, ProvenRecord, ProvenValue,
        RegistryProofRequest,
    },
    wire::{
        DelegateCheck, DelegateEntryStatus, ProxyAllocationRequest, ProxyDelegateChoice,
        ProxyDelegateProvider,
    },
};

/// The choice a delegator makes from a listing of `picked`.
fn choice(picked: &ProxyDelegateEntry) -> ProxyDelegateChoice {
    ProxyDelegateChoice {
        delegate_index: picked.index,
        percent_bps: 1_000,
        provider: match picked.provider {
            IdentityProvider::X => ProxyDelegateProvider::X,
            IdentityProvider::GitHub => ProxyDelegateProvider::GitHub,
        },
        handle: picked.handle.clone(),
        dk: picked.dk.encode(),
    }
}

/// A request choosing each of `picked`.
fn request(picked: &[ProxyDelegateEntry]) -> ProxyAllocationRequest {
    ProxyAllocationRequest {
        round_id: ROUND_ID.to_owned(),
        choices: picked.iter().map(choice).collect(),
    }
}

/// The one verdict for a choice of `picked`, given `proofs`.
fn verdict(picked: &ProxyDelegateEntry, proofs: &[ProvenRecord]) -> DelegateCheck {
    let context = ConfirmContext {
        wallet_list: None,
        first_new_index: None,
    };
    check_delegates(&context, &request(std::slice::from_ref(picked)), proofs)[0].check
}

/// The verdict for a choice of `picked` when the chain holds `on_chain` at
/// its index.
fn verdict_against(picked: &ProxyDelegateEntry, on_chain: &ProxyDelegateEntry) -> DelegateCheck {
    verdict(picked, &[proven(on_chain)])
}

#[test]
fn one_evidence_answer_proves_each_distinct_chosen_entry() {
    let chosen = request(&[entry(17, "alice"), entry(23, "bob"), entry(17, "alice")]);
    assert_eq!(
        confirm_proof_requests(&chosen),
        vec![
            RegistryProofRequest { delegate_index: 17 },
            RegistryProofRequest { delegate_index: 23 },
        ]
    );
}

/// Only a proven, decodable entry with the chosen index gets a verdict about
/// the delegate. Anything else cannot be verified.
#[test]
fn an_entry_that_is_not_proven_cannot_be_verified() {
    let alice = entry(17, "alice");
    for (case, proofs) in [
        ("no proof", Vec::new()),
        (
            "an unavailable proof",
            vec![entry_proof(17, ProvenValue::Unavailable)],
        ),
        (
            "bytes that do not decode",
            vec![entry_proof(17, ProvenValue::Present(vec![0xff; 4]))],
        ),
        (
            "another entry's bytes",
            vec![entry_proof(
                17,
                ProvenValue::Present(entry(18, "alice").encode()),
            )],
        ),
    ] {
        assert_eq!(
            verdict(&alice, &proofs),
            DelegateCheck::CannotVerify,
            "{case}"
        );
    }
}

/// A commit needs an active entry: an absent, suspended or retired one is
/// refused as such.
#[test]
fn only_a_proven_active_entry_is_ok() {
    let alice = entry(17, "alice");
    assert_eq!(verdict_against(&alice, &alice), DelegateCheck::Ok);
    assert_eq!(
        verdict_against(
            &alice,
            &with_status(alice.clone(), DelegateStatus::Suspended)
        ),
        DelegateCheck::Suspended
    );
    assert_eq!(
        verdict_against(&alice, &with_status(alice.clone(), DelegateStatus::Retired)),
        DelegateCheck::Retired
    );
    assert_eq!(
        verdict(&alice, &[entry_proof(17, ProvenValue::Absent)]),
        DelegateCheck::NotFound
    );
}

/// The proven entry must hold the key, provider and handle the delegator
/// picked, so a registry cache cannot pass another delegate's entry off under
/// a chosen index.
#[test]
fn a_proven_entry_with_another_key_provider_or_handle_is_a_mismatch() {
    let alice = entry(17, "alice");
    for other in [
        ProxyDelegateEntry {
            dk: dk(99),
            ..alice.clone()
        },
        ProxyDelegateEntry {
            provider: IdentityProvider::GitHub,
            proof_id: "aa5a315d".to_owned(),
            ..alice.clone()
        },
        ProxyDelegateEntry {
            handle: "alicee".to_owned(),
            ..alice.clone()
        },
    ] {
        assert_eq!(
            verdict_against(&alice, &other),
            DelegateCheck::Mismatch,
            "{other:?}"
        );
    }
}

/// The confirm listing shows the proven entry's handle, key and status, with
/// its reviewed status recomputed from them, and none without a proven
/// entry.
#[test]
fn the_confirm_listing_comes_from_the_proven_entry() {
    let list = verified(&wallet_list(
        1,
        vec![row(&entry(17, "alice")), row(&entry(18, "bob"))],
        &[],
    ));
    let context = ConfirmContext {
        wallet_list: Some(&list),
        first_new_index: Some(18),
    };
    // The chain's entry 18 is not the delegate the list reviewed.
    let other = with_status(
        ProxyDelegateEntry {
            dk: dk(99),
            ..entry(18, "mallory")
        },
        DelegateStatus::Suspended,
    );
    let picked = [entry(17, "alice"), entry(18, "bob"), entry(19, "carol")];
    let confirmed = check_delegates(
        &context,
        &request(&picked),
        &[proven(&entry(17, "alice")), proven(&other)],
    );

    let alice = confirmed[0].listing.as_ref().unwrap();
    assert_eq!(confirmed[0].check, DelegateCheck::Ok);
    assert_eq!(alice.handle, "alice");
    assert_eq!(alice.key.dk, dk(17).encode());
    assert!(alice.reviewed && !alice.new_this_round);

    let mallory = confirmed[1].listing.as_ref().unwrap();
    assert_eq!(confirmed[1].check, DelegateCheck::Mismatch);
    assert_eq!(mallory.handle, "mallory");
    assert_eq!(mallory.key.fingerprint, dk(99).fingerprint().to_string());
    assert_eq!(mallory.status, DelegateEntryStatus::Suspended);
    assert!(!mallory.reviewed && mallory.new_this_round);

    assert_eq!(confirmed[2].check, DelegateCheck::CannotVerify);
    assert_eq!(confirmed[2].listing, None);
}

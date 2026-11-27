//! The delegate's one ballot per round: complete, signed under the entry's
//! key, stored before it is posted, resent unchanged, and never replaced by
//! a different one.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use zcash_vote_delegate::{verify_zip215, DelegateStatus};

use super::fixtures::{
    bob_key, chain, db, hex_bytes, proven_entry, roster, round_timing, signing_key, vector_bytes,
    vectors, ALICE_SECRET, BOB_SECRET, DELEGATE_INDEX, VECTOR_ROUND_ID,
};
use crate::{
    proxy::delegate::{
        ballot_preflight, classify_ballot_answer, existing_ballot_result, sign_ballot,
        stored_ballot, BallotAnswer, BallotError, ChainDelegateBallot, CompleteBallot,
        DelegateChain, DelegateModeError, DelegateOpAnswer, DelegateOpKind, DelegateRoundTiming,
        ProvenDelegateEntry, StoredBallot,
    },
    storage::VotingDb,
    wire::{BallotPreflight, BallotSubmitResult, DelegateBallotBlock},
    ProposalRosterEntry,
};

/// The round's ballot deadline.
const VOTE_END_TIME: u64 = 1_791_000_000;
/// A device time while the round runs.
const NOW: u64 = VOTE_END_TIME - 3_600;

/// The ballot vectors' round id.
fn round_id() -> [u8; 32] {
    hex_bytes(VECTOR_ROUND_ID)
}

/// The vectors' round's timing.
fn round() -> DelegateRoundTiming {
    round_timing(round_id(), VOTE_END_TIME)
}

/// Bob's active entry, whose key signs the ballot vectors.
fn bob_entry() -> ProvenDelegateEntry {
    proven_entry(bob_key(), DelegateStatus::Active)
}

/// A complete ballot of `options` for the vectors' round with 8 options per
/// proposal.
fn ballot(options: &[u32]) -> CompleteBallot {
    let count = u32::try_from(options.len()).unwrap();
    CompleteBallot::new(round_id(), &roster(count), options.to_vec()).unwrap()
}

/// Signs `options` for bob's entry with bob's key at [`NOW`].
fn sign_as_bob(db: &VotingDb, options: &[u32]) -> Result<StoredBallot, BallotError> {
    sign_ballot(
        db,
        &chain(),
        &round(),
        &bob_entry(),
        &ballot(options),
        &signing_key(BOB_SECRET),
        NOW,
    )
}

/// A listed ballot of the vectors' delegate.
fn listed(options: &[u32]) -> ChainDelegateBallot {
    ChainDelegateBallot {
        delegate_index: DELEGATE_INDEX,
        options: options.to_vec(),
        height: 77,
    }
}

/// The roster must list ids 1..n with 2 to 8 options, and the ballot must
/// answer every proposal with one of its options.
#[test]
fn complete_ballots_answer_every_proposal() {
    let round = round_id();
    let new = |proposals: &[ProposalRosterEntry], options: Vec<u32>| {
        CompleteBallot::new(round, proposals, options)
    };
    assert!(matches!(new(&[], vec![]), Err(BallotError::EmptyRoster)));
    assert!(matches!(
        new(&roster(51), vec![0; 51]),
        Err(BallotError::TooManyProposals { count: 51 })
    ));
    let gap = [
        roster(1)[0],
        ProposalRosterEntry {
            proposal_id: 3,
            num_options: 2,
        },
    ];
    assert!(matches!(
        new(&gap, vec![0, 0]),
        Err(BallotError::RosterOutOfOrder {
            position: 2,
            proposal_id: 3
        })
    ));
    for num_options in [1, 9] {
        let odd = [ProposalRosterEntry {
            proposal_id: 1,
            num_options,
        }];
        assert!(matches!(
            new(&odd, vec![0]),
            Err(BallotError::InvalidOptionCount { proposal_id: 1, .. })
        ));
    }
    for options in [vec![0, 1], vec![0, 1, 2, 3]] {
        assert!(matches!(
            new(&roster(3), options),
            Err(BallotError::WrongAnswerCount { proposals: 3, .. })
        ));
    }
    let largest_option_value = u32::MAX;
    assert!(matches!(
        new(&roster(1), vec![largest_option_value]),
        Err(BallotError::OptionOutOfRange {
            proposal_id: 1,
            option: u32::MAX,
            num_options: 8
        })
    ));
    let two_options = [ProposalRosterEntry {
        proposal_id: 1,
        num_options: 2,
    }];
    assert!(matches!(
        new(&two_options, vec![2]),
        Err(BallotError::OptionOutOfRange { option: 2, .. })
    ));
    let complete = new(&roster(3), vec![7, 0, 3]).unwrap();
    assert_eq!(complete.options(), &[7, 0, 3]);
    assert_eq!(complete.vote_round_id(), &round);
}

/// The `ballot` vectors a roster can hold are signed under the entry's key
/// over the vectors' BALLOT digests, in `MsgDelegateOp` JSON.
#[test]
fn ballots_sign_the_vector_digests() {
    let vectors = vectors();
    let mut signed = 0;
    for case in vectors["ballot"].as_array().unwrap() {
        let options: Vec<u32> = case["inputs"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| u32::try_from(option.as_u64().unwrap()).unwrap())
            .collect();
        if options.iter().any(|option| *option >= 8) {
            continue;
        }
        assert_eq!(case["inputs"]["dk_sk"], BOB_SECRET);
        assert_eq!(case["inputs"]["vote_round_id"], VECTOR_ROUND_ID);
        assert_eq!(case["inputs"]["delegate_index"], DELEGATE_INDEX);
        let stored = sign_as_bob(&db(), &options).unwrap();
        let op = stored.op();
        assert_eq!(op.kind(), DelegateOpKind::Ballot);
        let text = String::from_utf8(op.body().to_vec()).unwrap();
        assert!(
            text.starts_with(r#"{"ballot":{"vote_round_id":""#),
            "{text}"
        );
        let body: Value = serde_json::from_slice(op.body()).unwrap();
        let ballot_json = &body["ballot"];
        assert_eq!(
            BASE64
                .decode(ballot_json["vote_round_id"].as_str().unwrap())
                .unwrap(),
            round_id()
        );
        assert_eq!(ballot_json["delegate_index"], DELEGATE_INDEX);
        assert_eq!(ballot_json["options"], case["inputs"]["options"]);
        let expected = &case["expected"];
        let dk_sig: [u8; 64] = vector_bytes(&expected["dk_sig"]);
        assert_eq!(
            BASE64
                .decode(ballot_json["dk_sig"].as_str().unwrap())
                .unwrap(),
            dk_sig,
            "{}",
            case["name"]
        );
        let dk: [u8; 32] = vector_bytes(&expected["dk"]);
        assert_eq!(stored.dk().to_bytes(), dk);
        assert_eq!(
            verify_zip215(&dk, &vector_bytes::<32>(&expected["digest"]), &dk_sig),
            Ok(())
        );
        signed += 1;
    }
    assert_eq!(signed, 2);
}

/// A retry posts the stored bytes: signing the same ballot again returns
/// the stored op without a new signature or any check, even after voting
/// ended or the entry was suspended, and the stored op needs no key at all.
#[test]
fn retries_resend_the_stored_bytes() {
    let db = db();
    let first = sign_as_bob(&db, &[1, 0, 2]).unwrap();
    let body = first.op().body().to_vec();

    let reloaded = stored_ballot(&db, &chain(), &round_id(), DELEGATE_INDEX)
        .unwrap()
        .unwrap();
    assert_eq!(reloaded, first);
    assert_eq!(reloaded.op().body(), body.as_slice());

    let again = sign_ballot(
        &db,
        &chain(),
        &round(),
        &proven_entry(bob_key(), DelegateStatus::Suspended),
        &ballot(&[1, 0, 2]),
        &signing_key(ALICE_SECRET),
        VOTE_END_TIME + 1,
    )
    .unwrap();
    assert_eq!(again, first);
    assert_eq!(again.op().body(), body.as_slice());
    assert_eq!(again.signed_at(), NOW);
}

/// A different ballot for a round that has one is refused, and the stored
/// one stays.
#[test]
fn a_different_second_ballot_is_refused() {
    let db = db();
    let first = sign_as_bob(&db, &[1, 0, 2]).unwrap();
    assert!(matches!(
        sign_as_bob(&db, &[1, 0, 3]),
        Err(BallotError::DifferentBallotStored { ref stored }) if stored == &vec![1, 0, 2]
    ));
    assert_eq!(
        stored_ballot(&db, &chain(), &round_id(), DELEGATE_INDEX).unwrap(),
        Some(first)
    );
}

/// Preflight reports the listed ballot first, then this device's, and
/// whether it equals the ballot about to be submitted.
#[test]
fn preflight_reports_the_stored_ballot() {
    let db = db();
    let chain = chain();
    let preflight = |listed_ballot: Option<&ChainDelegateBallot>, options: &[u32]| {
        ballot_preflight(
            &db,
            &chain,
            &round_id(),
            DELEGATE_INDEX,
            listed_ballot,
            options,
        )
    };
    assert_eq!(
        preflight(None, &[1, 0, 2]).unwrap(),
        BallotPreflight::NoBallot
    );
    sign_as_bob(&db, &[1, 0, 2]).unwrap();
    assert_eq!(
        preflight(None, &[1, 0, 2]).unwrap(),
        BallotPreflight::AlreadySubmitted {
            options: vec![1, 0, 2],
            identical: true,
        }
    );
    assert_eq!(
        preflight(None, &[0, 0, 0]).unwrap(),
        BallotPreflight::AlreadySubmitted {
            options: vec![1, 0, 2],
            identical: false,
        }
    );
    assert_eq!(
        preflight(Some(&listed(&[3, 3, 3])), &[1, 0, 2]).unwrap(),
        BallotPreflight::AlreadySubmitted {
            options: vec![3, 3, 3],
            identical: false,
        }
    );
    let mut other_delegate = listed(&[1, 0, 2]);
    other_delegate.delegate_index = 18;
    assert!(matches!(
        preflight(Some(&other_delegate), &[1, 0, 2]),
        Err(DelegateModeError::InvalidInput { .. })
    ));
}

/// A new ballot is signed only while voting is open, for an active entry,
/// by the entry's key, and for the round being signed. Nothing is stored
/// otherwise.
#[test]
fn a_new_ballot_needs_an_open_round_an_active_entry_and_its_key() {
    let db = db();
    let sign = |entry: &ProvenDelegateEntry, secret: &str, now: u64| {
        sign_ballot(
            &db,
            &chain(),
            &round(),
            entry,
            &ballot(&[1]),
            &signing_key(secret),
            now,
        )
    };
    assert!(matches!(
        sign(&bob_entry(), BOB_SECRET, VOTE_END_TIME),
        Err(BallotError::VotingEnded {
            vote_end_time: VOTE_END_TIME
        })
    ));
    for (status, block) in [
        (DelegateStatus::Suspended, DelegateBallotBlock::Suspended),
        (DelegateStatus::Retired, DelegateBallotBlock::Retired),
    ] {
        assert!(matches!(
            sign(&proven_entry(bob_key(), status), BOB_SECRET, NOW),
            Err(BallotError::Blocked { block: blocked }) if blocked == block
        ));
    }
    assert!(matches!(
        sign(&bob_entry(), ALICE_SECRET, NOW),
        Err(BallotError::NotEntryKey { fingerprint }) if fingerprint == bob_key().fingerprint().to_string()
    ));
    let other_round = round_timing([9; 32], VOTE_END_TIME);
    assert!(matches!(
        sign_ballot(
            &db,
            &chain(),
            &other_round,
            &bob_entry(),
            &ballot(&[1]),
            &signing_key(BOB_SECRET),
            NOW,
        ),
        Err(BallotError::RoundMismatch { .. })
    ));
    assert_eq!(
        stored_ballot(&db, &chain(), &round_id(), DELEGATE_INDEX).unwrap(),
        None
    );
    assert!(sign(&bob_entry(), BOB_SECRET, VOTE_END_TIME - 1).is_ok());
}

/// A ballot is stored for its vote chain: the same round and entry on
/// another chain of the network has none.
#[test]
fn a_ballot_is_stored_for_its_vote_chain() {
    let db = db();
    sign_as_bob(&db, &[1]).unwrap();
    let other = DelegateChain::new(chain().network(), "zvote-2").unwrap();
    assert_eq!(
        stored_ballot(&db, &other, &round_id(), DELEGATE_INDEX).unwrap(),
        None
    );
}

/// Code 0 is accepted; code 57 is success only when the listed ballot is the
/// stored one.
#[test]
fn ballot_answers_settle_the_submission() {
    let stored = sign_as_bob(&db(), &[1, 0, 2]).unwrap();
    assert_eq!(
        classify_ballot_answer(&DelegateOpAnswer::Accepted {
            tx_hash: "AB".repeat(32)
        }),
        BallotAnswer::Settled(BallotSubmitResult::Accepted)
    );
    assert_eq!(
        classify_ballot_answer(&DelegateOpAnswer::Rejected {
            codespace: "vote".to_owned(),
            code: 57,
            log: "exists".to_owned(),
        }),
        BallotAnswer::BallotExists
    );
    assert_eq!(
        existing_ballot_result(&stored, Some(&listed(&[1, 0, 2]))),
        Some(BallotSubmitResult::AlreadyAccepted)
    );
    assert!(matches!(
        existing_ballot_result(&stored, Some(&listed(&[1, 0, 3]))),
        Some(BallotSubmitResult::Rejected { ref codespace, code: 57, .. }) if codespace == "vote"
    ));
    assert_eq!(existing_ballot_result(&stored, None), None);
    let mut other_delegate = listed(&[1, 0, 2]);
    other_delegate.delegate_index = 18;
    assert_eq!(existing_ballot_result(&stored, Some(&other_delegate)), None);
}

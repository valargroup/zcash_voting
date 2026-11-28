use std::time::Duration;

use proptest::prelude::*;
use rand::{rngs::StdRng, SeedableRng};

pub(super) use super::submission_fixtures::FIXTURE_ROSTER;
use crate::{
    backend::pasta_curves::{
        group::{ff::PrimeField, Group, GroupEncoding},
        pallas,
    },
    governance::BALLOT_DIVISOR,
    proxy::{
        commit_allocation, preview_allocation, BundleBallots, CommittedAllocation, DcShareLayout,
        MAX_PROXY_DELEGATES, WHOLE_ALLOCATION_BPS,
    },
    storage::{queries, VotingDb},
    types::{Network, VotingRoundParams},
    wire::{
        ProxyAllocationPreview, ProxyDelegateChoice, ProxyDelegationError, ProxyDelegationErrorKind,
    },
    ChainAdvancePolicy, RoundHostContext,
};

/// A canonical 32-byte round id, so the round can own lifecycle identities.
pub(super) const ROUND_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
/// The wallet every single-wallet fixture belongs to.
pub(super) const WALLET_ID: &str = "proxy-wallet";

pub(super) use super::submission_fixtures::delegate_choice as choice;

/// The refusal of `delegates` delegates over `total_ballots` ballots, too few
/// to give each one.
pub(super) fn not_enough_voting_power(
    total_ballots: u64,
    delegates: usize,
) -> ProxyDelegationError {
    ProxyDelegationError::new(
        ProxyDelegationErrorKind::NotEnoughVotingPower,
        format!(
            "{delegates} delegates need at least {delegates} ballots, the bundles hold \
             {total_ballots}"
        ),
    )
}

/// The refusal of an allocation once a vote of `bundle_index` may have spent
/// its VAN or saved a share plan.
pub(super) fn already_voted(bundle_index: u32) -> ProxyDelegationError {
    ProxyDelegationError::new(
        ProxyDelegationErrorKind::AlreadyVoted,
        format!(
            "a vote of bundle {bundle_index} may already have spent its VAN or has a saved share \
             plan, so the wallet cannot delegate"
        ),
    )
}

/// A request choosing each `(delegate_index, percent_bps)` in order.
pub(super) fn request(choices: &[(u32, u16)]) -> Vec<ProxyDelegateChoice> {
    choices
        .iter()
        .map(|&(delegate_index, percent_bps)| choice(delegate_index, percent_bps))
        .collect()
}

/// Bundles `0, 1, …` holding `weights` ballots.
pub(super) fn bundles(weights: &[u64]) -> Vec<BundleBallots> {
    weights
        .iter()
        .enumerate()
        .map(|(bundle_index, &ballots)| BundleBallots {
            bundle_index: bundle_index as u32,
            ballots,
        })
        .collect()
}

/// One bundle's weight, from a single ballot up to the circuit's 2^30 bound,
/// with small weights common so the one-ballot floor and splits are hit.
fn bundle_weight() -> impl Strategy<Value = u64> {
    prop_oneof![
        1u64..=8,
        9u64..=1_000,
        1_001u64..=1_000_000,
        1_000_001u64..=(1 << 30)
    ]
}

/// Basis points for `delegates` choices, each at least 1: a random split of
/// the whole round when `whole`, otherwise of a random total.
fn shares(delegates: usize, whole: bool) -> impl Strategy<Value = Vec<u16>> {
    let total = if whole {
        Just(WHOLE_ALLOCATION_BPS).boxed()
    } else {
        (delegates as u16..=WHOLE_ALLOCATION_BPS).boxed()
    };
    total.prop_flat_map(move |total| {
        proptest::sample::subsequence((1..total).collect::<Vec<u16>>(), delegates - 1).prop_map(
            move |cuts| {
                let mut bounds = vec![0];
                bounds.extend(cuts);
                bounds.push(total);
                bounds.windows(2).map(|pair| pair[1] - pair[0]).collect()
            },
        )
    })
}

/// A random request of 1 to 8 choices over 1 to 12 random bundles, with a
/// random layout. Choice indices run against request order, so nothing can
/// lean on index order to break ties.
pub(super) fn planning_case(
) -> impl Strategy<Value = (Vec<ProxyDelegateChoice>, Vec<u64>, DcShareLayout)> {
    (1..=MAX_PROXY_DELEGATES, any::<bool>())
        .prop_flat_map(|(delegates, whole)| {
            (
                shares(delegates, whole),
                proptest::collection::vec(bundle_weight(), 1..=12),
                prop_oneof![
                    Just(DcShareLayout::Standard),
                    Just(DcShareLayout::SingleShare)
                ],
            )
        })
        .prop_map(|(shares, weights, layout)| {
            let request = shares
                .iter()
                .enumerate()
                .map(|(position, &bps)| choice(50 - position as u32, bps))
                .collect();
            (request, weights, layout)
        })
}

/// A wallet database whose round [`ROUND_ID`] has bundles `0, 1, …` holding
/// `weights` whole ballots, each stored with a sub-ballot remainder of
/// zatoshi that planning must drop.
pub(super) fn db_with_bundle_weights(weights: &[u64]) -> VotingDb {
    let db = VotingDb::open_in_memory().unwrap();
    db.set_wallet_id(WALLET_ID);
    add_round(&db, WALLET_ID, weights);
    db
}

/// Adds round [`ROUND_ID`] for `wallet_id`, with bundles holding `weights`
/// whole ballots and a sub-ballot remainder each.
pub(super) fn add_round(db: &VotingDb, wallet_id: &str, weights: &[u64]) {
    let conn = db.conn();
    queries::insert_round(
        &conn,
        wallet_id,
        Network::Testnet,
        &VotingRoundParams {
            vote_round_id: ROUND_ID.to_string(),
            snapshot_height: 1_000,
            ea_pk: vec![0xEA; 32],
            nc_root: vec![0xAA; 32],
            nullifier_imt_root: vec![0xBB; 32],
        },
        None,
    )
    .unwrap();
    for (bundle_index, &ballots) in weights.iter().enumerate() {
        queries::insert_bundle(&conn, ROUND_ID, wallet_id, bundle_index as u32, &[]).unwrap();
        set_bundle_note_total(
            &conn,
            wallet_id,
            bundle_index as u32,
            Some(ballots * BALLOT_DIVISOR + BALLOT_DIVISOR - 1),
        );
    }
}

/// Records option 0 for every [`FIXTURE_ROSTER`] proposal of `wallet_id`,
/// so an allocation that keeps ballots may commit. Option 0 matches the
/// votes the tests store.
pub(super) fn answer_roster(db: &VotingDb, wallet_id: &str) {
    for proposal_id in FIXTURE_ROSTER {
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO ballot_intent
                     (round_id, wallet_id, proposal_id, skipped, choice, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, 0, 0, 0)",
                rusqlite::params![ROUND_ID, wallet_id, proposal_id],
            )
            .unwrap();
    }
}

/// [`db_with_bundle_weights`] with the kept share answered ([`answer_roster`]).
pub(super) fn answered_db_with_bundle_weights(weights: &[u64]) -> VotingDb {
    let db = db_with_bundle_weights(weights);
    answer_roster(&db, WALLET_ID);
    db
}

/// Overwrites one bundle's stored note total, in zatoshi.
pub(super) fn set_bundle_note_total(
    conn: &rusqlite::Connection,
    wallet_id: &str,
    bundle_index: u32,
    total_note_value: Option<u64>,
) {
    conn.execute(
        "UPDATE bundles SET total_note_value = ?1
          WHERE round_id = ?2 AND wallet_id = ?3 AND bundle_index = ?4",
        rusqlite::params![
            total_note_value.map(|value| value as i64),
            ROUND_ID,
            wallet_id,
            bundle_index
        ],
    )
    .unwrap();
}

/// Every row of `table`, rendered as text, in rowid order.
pub(super) fn dump_table(db: &VotingDb, table: &str) -> Vec<Vec<String>> {
    let conn = db.conn();
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| {
                    let value: rusqlite::types::Value = row.get(column)?;
                    Ok(format!("{value:?}"))
                })
                .collect()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// A deterministic CSPRNG for slot positions.
pub(super) fn seeded_rng(seed: u64) -> StdRng {
    StdRng::seed_from_u64(seed)
}

pub(super) use super::submission_fixtures::HOTKEY_SECRET;

/// The network the fixtures store their rounds under.
pub(super) const NETWORK: Network = Network::Testnet;

/// The `van_comm_rand` a hotkey-bound fixture stores for `bundle_index`.
pub(super) fn van_comm_rand(bundle_index: u32) -> [u8; 32] {
    pallas::Base::from(1_000 + u64::from(bundle_index)).to_repr()
}

/// The VAN the round hotkey, `ballots`, the fixture round and `van_comm_rand`
/// commit to, with full proposal authority.
pub(super) fn hotkey_van(ballots: u64, van_comm_rand: &[u8; 32]) -> Vec<u8> {
    super::submission_fixtures::hotkey_van(NETWORK, ballots * BALLOT_DIVISOR, van_comm_rand)
}

/// A wallet database whose round has bundles holding `weights` whole ballots,
/// each with the delegation setup the round hotkey reproduces: a canonical
/// `van_comm_rand`, address index 0 and the registration VAN they commit to.
/// The round's election key is the Pallas generator, so DCs build against it.
pub(super) fn db_with_hotkey_setup(weights: &[u64]) -> VotingDb {
    let db = db_with_bundle_weights(weights);
    {
        let conn = db.conn();
        conn.execute(
            "UPDATE rounds SET ea_pk = ?1 WHERE round_id = ?2",
            rusqlite::params![pallas::Point::generator().to_bytes().to_vec(), ROUND_ID],
        )
        .unwrap();
        for (bundle_index, &ballots) in weights.iter().enumerate() {
            let rand = van_comm_rand(bundle_index as u32);
            conn.execute(
                "UPDATE bundles SET van_comm_rand = ?1, gov_comm = ?2, address_index = 0
                  WHERE round_id = ?3 AND wallet_id = ?4 AND bundle_index = ?5",
                rusqlite::params![
                    rand.to_vec(),
                    hotkey_van(ballots, &rand),
                    ROUND_ID,
                    WALLET_ID,
                    bundle_index as i64
                ],
            )
            .unwrap();
        }
    }
    db
}

/// Previews and commits `request` with `layout`, slots drawn from a seeded
/// CSPRNG.
pub(super) fn commit(
    db: &VotingDb,
    request: &Vec<ProxyDelegateChoice>,
    layout: DcShareLayout,
) -> CommittedAllocation {
    answer_roster(db, &db.wallet_id());
    let plan = preview_allocation(db, ROUND_ID, request, layout).unwrap();
    commit_allocation(
        db,
        ROUND_ID,
        request,
        layout,
        &ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut seeded_rng(7),
    )
    .unwrap()
}

/// A host context at `now_seconds` for a round from `ceremony_start` to
/// `vote_end`, either of which may be unauthenticated.
pub(super) fn host_at(
    now_seconds: u64,
    ceremony_start_seconds: Option<u64>,
    vote_end_time_seconds: Option<u64>,
) -> RoundHostContext {
    RoundHostContext {
        configured_helper_urls: Vec::new(),
        now_seconds,
        ceremony_start_seconds,
        vote_end_time_seconds,
        vote_tree_node_urls: Vec::new(),
        delegation: None,
        chain_policy: ChainAdvancePolicy {
            pending_repoll: Duration::from_millis(1),
            ..ChainAdvancePolicy::default()
        },
        max_proof_concurrency: 1,
    }
}

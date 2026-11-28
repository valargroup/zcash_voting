//! A bundle whose proxy batch is proved and ready to sign and submit, shared
//! by the attempt, lifecycle, planning and executor tests.
//!
//! The batch's DCs are built from their planned slots exactly as proving
//! builds them, so every public field and `r_vpk` is real and D1 signatures
//! verify; only the ZKP4 proofs are placeholders, which nothing in these tests
//! verifies. The bundle's delegation setup is complete and bound to the
//! fixture hotkey, and a fixed SpendAuth key signs its registration.

use std::sync::Arc;

use rand::SeedableRng;

use crate::wire::ProxyDelegateChoice;
use crate::{
    backend::{
        orchard::primitives::redpallas::{SpendAuth, VerificationKey},
        pasta_curves::{
            group::{ff::PrimeField, Group, GroupEncoding},
            pallas,
        },
        zcash_keys::keys::UnifiedSpendingKey,
    },
    governance::{BALLOT_DIVISOR, BUNDLE_NOTE_SLOTS},
    proxy::{
        batch::{
            build_dc_chain, insert_proved_batch, load_chain_inputs, plan_batch_slots, BatchAnchor,
            ProvedProxyBatch, ProvedProxyDc,
        },
        commit_allocation, preview_allocation, DcShareLayout,
    },
    storage::{queries, VotingDb},
    types::{Network, VotingRoundParams},
    ChainSubmissionClientConfig, ChainTransport, HelperClient, ProposalRosterEntry, RoundBinding,
    RoundExecutor, VotingHotkey,
};

/// The proposal roster the fixtures commit against. Every fixture round
/// records proposal 1 as skipped before it commits, so a kept share counts
/// as answered.
pub(crate) const FIXTURE_ROSTER: [u32; 1] = [1];

/// Records every [`FIXTURE_ROSTER`] proposal of `db`'s wallet as skipped, so
/// an allocation that keeps ballots may commit.
pub(crate) fn skip_fixture_roster(db: &VotingDb, round_id: &str) {
    for proposal_id in FIXTURE_ROSTER {
        db.set_ballot_intent(round_id, proposal_id, crate::session::Decision::Skipped, 2)
            .unwrap();
    }
}

/// The fixture round, a canonical 32-byte id.
pub(crate) const ROUND_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
/// The fixture wallet.
pub(crate) const WALLET_ID: &str = "wallet";
/// The fixture network; Regtest activates the governance output early enough
/// for the round snapshot.
pub(crate) const NETWORK: Network = Network::Regtest;
/// The round hotkey's stored secret.
pub(crate) const HOTKEY_SECRET: [u8; 64] = [0x21; 64];
/// The chain id D1 binds in the fixtures.
pub(crate) const CHAIN_ID: &str = "svote-test";
/// The registration's PCZT sighash.
pub(crate) const SIGHASH: [u8; 32] = [0x49; 32];
/// The anchor height a chain-anchored fixture batch binds.
pub(crate) const CHAIN_ANCHOR_HEIGHT: u32 = 41;
/// The tree position of a chain-anchored fixture bundle's registration VAN.
pub(crate) const REGISTRATION_POSITION: u64 = 7;

/// Where the fixture batch's first DC finds its VAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FixtureAnchor {
    /// The batch carries the registration; the bundle is unregistered.
    Registration,
    /// The registration landed at [`REGISTRATION_POSITION`]; the batch binds
    /// [`CHAIN_ANCHOR_HEIGHT`].
    Chain,
}

/// A fixture round with a proved batch on bundle 0.
pub(crate) struct ProvedBatchFixture {
    /// The sidecar, scoped to [`WALLET_ID`].
    pub(crate) db: Arc<VotingDb>,
    /// The batch as stored.
    pub(crate) batch: ProvedProxyBatch,
    /// A valid SpendAuth signature over [`SIGHASH`] for the registration.
    pub(crate) registration_signature: [u8; 64],
}

/// The bundle's note total: 100 ballots and a sub-ballot remainder.
pub(crate) fn note_total() -> u64 {
    100 * BALLOT_DIVISOR + BALLOT_DIVISOR / 3
}

/// The bundle's VAN blinding.
pub(crate) fn van_comm_rand() -> [u8; 32] {
    pallas::Base::from(9u64).to_repr()
}

/// The VAN the fixture hotkey on `network`, `total_zats`, the fixture round
/// and `van_comm_rand` commit to, with full proposal authority.
pub(crate) fn hotkey_van(network: Network, total_zats: u64, van_comm_rand: &[u8; 32]) -> Vec<u8> {
    let target = VotingHotkey::from_stored_secret(&HOTKEY_SECRET, network)
        .unwrap()
        .delegation_target();
    let (g_d_x, pk_d_x) =
        crate::action::derive_hotkey_x_coords_from_raw_address(target.raw_orchard_address())
            .unwrap();
    crate::governance::construct_van(
        &g_d_x,
        &pk_d_x,
        total_zats,
        &hex::decode(ROUND_ID).unwrap(),
        van_comm_rand,
    )
    .unwrap()
}

/// The governance output a hotkey-bound fixture setup stores at
/// `snapshot_height`: its `nf_signed`, `rseed_output` and `cmx_new`.
pub(crate) fn hotkey_output(snapshot_height: u64) -> ([u8; 32], [u8; 32], [u8; 32]) {
    let target = VotingHotkey::from_stored_secret(&HOTKEY_SECRET, NETWORK)
        .unwrap()
        .delegation_target();
    let nf_signed = pallas::Base::from(11u64).to_repr();
    let rseed_output = [0x47u8; 32];
    let cmx_new = crate::action::derive_governance_output_cmx(
        target.raw_orchard_address(),
        &nf_signed,
        &rseed_output,
        NETWORK,
        snapshot_height,
    )
    .unwrap();
    (nf_signed, rseed_output, cmx_new)
}

/// The chain config of an executor over a scripted chain: one endpoint, one
/// POST attempt and [`CHAIN_ID`].
pub(crate) fn scripted_chain_config() -> ChainSubmissionClientConfig {
    ChainSubmissionClientConfig::for_network(NETWORK, vec!["https://vote.example".into()])
        .with_vote_chain_id(CHAIN_ID)
        .with_post_attempts(1, Vec::new())
}

/// An executor over `db` bound to the fixture round with the fixture hotkey
/// and one proposal per `(proposal_id, num_options)`.
pub(crate) fn bound_executor<T: ChainTransport>(
    db: &Arc<VotingDb>,
    chain: T,
    chain_config: ChainSubmissionClientConfig,
    helpers: HelperClient,
    proposals: &[(u32, u32)],
) -> RoundExecutor<T> {
    RoundExecutor::with_transport(Arc::clone(db), chain, chain_config, helpers)
        .unwrap()
        .with_binding(RoundBinding {
            round_id: ROUND_ID.to_string(),
            network: NETWORK,
            proposals: proposals
                .iter()
                .map(|&(proposal_id, num_options)| ProposalRosterEntry {
                    proposal_id,
                    num_options,
                })
                .collect(),
            hotkey_secret: Some(zeroize::Zeroizing::new(HOTKEY_SECRET.to_vec())),
        })
        .unwrap()
}

/// A choice of `delegate_index` for `percent_bps`: `delegate<index>` on X,
/// with the key of a secret seed made of the index's low byte.
pub(crate) fn delegate_choice(delegate_index: u32, percent_bps: u16) -> ProxyDelegateChoice {
    let seed = [delegate_index as u8; 32];
    ProxyDelegateChoice {
        delegate_index,
        percent_bps,
        provider: crate::wire::ProxyDelegateProvider::X,
        handle: format!("delegate{delegate_index}"),
        dk: zcash_vote_delegate::DelegateSigningKey::from_secret_seed(seed)
            .delegate_key()
            .encode(),
    }
}

/// The two delegates of the fixture allocation, keeping the rest.
pub(crate) fn two_delegates() -> Vec<ProxyDelegateChoice> {
    vec![delegate_choice(17, 2_300), delegate_choice(5, 1_700)]
}

/// A round whose bundle 0 holds a complete, hotkey-bound delegation setup and
/// `request` committed under `layout`, its batch planned but not proved.
pub(crate) fn committed_round_for(
    layout: DcShareLayout,
    request: Vec<ProxyDelegateChoice>,
) -> (Arc<VotingDb>, [u8; 64]) {
    let params = VotingRoundParams {
        vote_round_id: ROUND_ID.to_string(),
        snapshot_height: u64::from(crate::types::REGTEST_NU6_3_ACTIVATION_HEIGHT),
        ea_pk: pallas::Point::generator().to_bytes().to_vec(),
        nc_root: vec![0xAA; 32],
        nullifier_imt_root: vec![0xBB; 32],
    };
    let db = Arc::new(VotingDb::open_in_memory().unwrap());
    db.set_wallet_id(WALLET_ID);
    db.create_round(NETWORK, &params, None).unwrap();

    let (nf_signed, rseed_output, cmx_new) = hotkey_output(params.snapshot_height);
    let registration_van = hotkey_van(NETWORK, note_total(), &van_comm_rand());

    // The SpendAuth key that signs the registration, randomized by `alpha`.
    let alpha = pallas::Scalar::from(7u64);
    let spending = UnifiedSpendingKey::from_seed(
        &NETWORK,
        &[0x42; 64],
        voting_crypto_deps::zip32::AccountId::ZERO,
    )
    .unwrap();
    let randomized = crate::backend::orchard::keys::SpendAuthorizingKey::from(spending.orchard())
        .randomize(&voting_crypto_deps::spend_auth_randomizer(&alpha));
    let rk: [u8; 32] = (&VerificationKey::<SpendAuth>::from(&randomized)).into();
    let registration_signature: [u8; 64] =
        (&randomized.sign(voting_crypto_deps::rand::rngs::OsRng, &SIGHASH)).into();
    let gov_nullifiers = vec![vec![0x12u8; 32]; BUNDLE_NOTE_SLOTS];
    {
        let conn = db.conn();
        queries::insert_bundle(&conn, ROUND_ID, WALLET_ID, 0, &[1, 2, 3, 4, 5]).unwrap();
        conn.execute(
            "UPDATE bundles SET note_identity_hashes_blob = ?1
              WHERE round_id = ?2 AND wallet_id = ?3 AND bundle_index = 0",
            rusqlite::params![vec![0x01u8; 32 * BUNDLE_NOTE_SLOTS], ROUND_ID, WALLET_ID],
        )
        .unwrap();
        queries::store_delegation_data_with_pczt_fields(
            &conn,
            ROUND_ID,
            WALLET_ID,
            0,
            &van_comm_rand(),
            &[],
            &pallas::Base::from(5u64).to_repr(),
            &[],
            &nf_signed,
            &cmx_new,
            &alpha.to_repr(),
            &[0x46; 32],
            &rseed_output,
            &registration_van,
            note_total(),
            0,
            &[],
            &SIGHASH,
            &crate::tx1::placeholder_tx1_effects(),
            &[],
            &rk,
            &gov_nullifiers,
        )
        .unwrap();
        queries::store_proof_result_fields_with_van_comm(
            &conn,
            ROUND_ID,
            WALLET_ID,
            0,
            &rk,
            &gov_nullifiers,
            &nf_signed,
            &cmx_new,
            &registration_van,
        )
        .unwrap();
        queries::store_proof(&conn, ROUND_ID, WALLET_ID, 0, &[0x61; 96], None).unwrap();
    }

    skip_fixture_roster(&db, ROUND_ID);
    let plan = preview_allocation(&db, ROUND_ID, &request, layout).unwrap();
    commit_allocation(
        &db,
        ROUND_ID,
        &request,
        layout,
        &crate::wire::ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut rand::rngs::StdRng::seed_from_u64(3),
    )
    .unwrap();
    (db, registration_signature)
}

/// [`committed_round_for`] with bundle 0's batch planned, built and stored as
/// proved against `anchor`.
pub(crate) fn proved_batch(anchor: FixtureAnchor) -> ProvedBatchFixture {
    proved_batch_with(anchor, DcShareLayout::Standard)
}

/// [`proved_batch`] with the allocation committed under `layout`.
pub(crate) fn proved_batch_with(
    anchor: FixtureAnchor,
    layout: DcShareLayout,
) -> ProvedBatchFixture {
    proved_batch_for(anchor, layout, two_delegates())
}

/// [`proved_batch_with`], committing `request` in place of
/// [`two_delegates`].
pub(crate) fn proved_batch_for(
    anchor: FixtureAnchor,
    layout: DcShareLayout,
    request: Vec<ProxyDelegateChoice>,
) -> ProvedBatchFixture {
    let (db, registration_signature) = committed_round_for(layout, request);
    plan_batch_slots(&db, ROUND_ID, 0, &HOTKEY_SECRET, NETWORK).unwrap();
    let inputs = db
        .read_transaction("fixture batch inputs", |tx| {
            load_chain_inputs(tx, ROUND_ID, WALLET_ID, 0, NETWORK)
        })
        .unwrap();
    // DCs do not depend on the anchor, so the registration-anchored build
    // stands in for a chain-anchored one too.
    let built = build_dc_chain(&inputs, &BatchAnchor::Registration, &HOTKEY_SECRET).unwrap();
    let anchor_height = match anchor {
        FixtureAnchor::Registration => 0,
        FixtureAnchor::Chain => {
            db.store_delegation_tx_hash(ROUND_ID, 0, &"dd".repeat(32))
                .unwrap();
            db.store_van_position(ROUND_ID, 0, REGISTRATION_POSITION as u32)
                .unwrap();
            CHAIN_ANCHOR_HEIGHT
        }
    };
    let batch = ProvedProxyBatch {
        bundle_index: 0,
        anchor_height,
        layout,
        dcs: built
            .iter()
            .map(|dc| {
                let instance = &dc.bundle.instance;
                ProvedProxyDc {
                    dc_slot: dc.slot.dc_slot,
                    delegate_index: dc.slot.delegate_index,
                    weight_ballots: dc.slot.weight_ballots,
                    van_nullifier: instance.van_nullifier.to_repr(),
                    r_vpk: dc.bundle.r_vpk_bytes,
                    vote_authority_note_new: instance.vote_authority_note_new.to_repr(),
                    delegation_commitment: instance.delegation_commitment.to_repr(),
                    dc_slot_nullifier: instance.dc_slot_nullifier.to_repr(),
                    alpha_v: dc.slot.alpha_v,
                    proof: vec![0x77; 64],
                }
            })
            .collect(),
    };
    db.write_transaction("fixture proved batch", |tx| {
        insert_proved_batch(tx, ROUND_ID, WALLET_ID, &batch, 1)
    })
    .unwrap();
    ProvedBatchFixture {
        db,
        batch,
        registration_signature,
    }
}

/// The batch digest every attempt of the fixture batch shares: D1 with
/// expiry 0 under [`CHAIN_ID`].
pub(crate) fn batch_digest(fixture: &ProvedBatchFixture) -> [u8; 32] {
    let registration_van = crate::proxy::batch::registration_van(
        &fixture.db.conn(),
        ROUND_ID,
        WALLET_ID,
        0,
        &fixture.batch,
    )
    .unwrap();
    let round: [u8; 32] = hex::decode(ROUND_ID).unwrap().try_into().unwrap();
    zcash_vote_delegate::proxy_batch_digest(
        CHAIN_ID,
        &crate::proxy::batch::batch_fields(round, &fixture.batch, registration_van, 0),
    )
    .unwrap()
}

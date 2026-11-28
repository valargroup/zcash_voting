//! A Regtest round whose one bundle holds a hotkey-bound, unregistered
//! delegation setup and a committed proxy allocation, and an executor bound
//! to the same hotkey.

use super::super::fixtures::{host_with_driver_target, note, round_params, Interrupt, ROUND_ID};
pub(super) use crate::proxy::tests::submission_fixtures::{
    note_total, van_comm_rand, HOTKEY_SECRET, NETWORK,
};
use crate::{
    backend::pasta_curves::{
        group::{ff::PrimeField, Group, GroupEncoding},
        pallas,
    },
    proxy::{
        commit_allocation, preview_allocation,
        tests::submission_fixtures::{
            hotkey_output, hotkey_van, skip_fixture_roster, two_delegates, FIXTURE_ROSTER,
        },
        DcShareLayout,
    },
    round::VotingDb,
    wire::VotingRoundParams,
    ChainSubmissionClientConfig, ChainSubmissionControl, HelperClient, HelperHealth,
    HyperTransport, ProposalRosterEntry, RoundBinding, RoundExecutor, RoundHostContext,
    VotingHotkey,
};
use rand::SeedableRng;
use std::sync::Arc;

/// A sidecar for wallet `wallet` whose round's bundle 0 holds the setup the
/// fixture hotkey reproduces for `total_note_value` zatoshi, unregistered, and
/// a committed two-delegate allocation in the standard layout.
pub(super) fn proxy_database(total_note_value: u64) -> Arc<VotingDb> {
    let params = VotingRoundParams {
        snapshot_height: u64::from(crate::types::REGTEST_NU6_3_ACTIVATION_HEIGHT),
        ea_pk: pallas::Point::generator().to_bytes().to_vec(),
        ..round_params()
    };
    let database = Arc::new(VotingDb::open_in_memory().unwrap());
    database.set_wallet_id("wallet");
    database.create_round(NETWORK, &params, None).unwrap();
    database.ensure_bundles(ROUND_ID, &[note()]).unwrap();

    let (nf_signed, rseed_output, cmx_new) = hotkey_output(params.snapshot_height);
    let registration_van = hotkey_van(NETWORK, total_note_value, &van_comm_rand());
    crate::storage::queries::store_delegation_data(
        &database.conn(),
        ROUND_ID,
        "wallet",
        0,
        &van_comm_rand(),
        &[],
        &pallas::Base::from(5u64).to_repr(),
        &[],
        &nf_signed,
        &cmx_new,
        &[0x45; 32],
        &[0x46; 32],
        &rseed_output,
        &registration_van,
        total_note_value,
        0,
        &[],
        &[0x49; 32],
        &crate::tx1::placeholder_tx1_effects(),
    )
    .unwrap();

    let request = two_delegates();
    skip_fixture_roster(&database, ROUND_ID);
    let plan = preview_allocation(&database, ROUND_ID, &request, DcShareLayout::Standard).unwrap();
    commit_allocation(
        &database,
        ROUND_ID,
        &request,
        DcShareLayout::Standard,
        &crate::wire::ProxyAllocationPreview::from(&plan),
        &FIXTURE_ROSTER,
        &mut rand::rngs::StdRng::seed_from_u64(3),
    )
    .unwrap();
    database
}

/// An executor over `database` bound to the fixture round, with the fixture
/// hotkey unless `with_hotkey` is false.
pub(super) fn proxy_executor(
    database: Arc<VotingDb>,
    with_hotkey: bool,
) -> RoundExecutor<HyperTransport> {
    RoundExecutor::new(
        database,
        ChainSubmissionClientConfig::for_network(NETWORK, vec!["http://chain.invalid".to_string()]),
        HelperClient::new(Arc::new(HyperTransport::new()), HelperHealth::default()),
    )
    .unwrap()
    .with_binding(RoundBinding {
        round_id: ROUND_ID.to_string(),
        network: NETWORK,
        proposals: vec![ProposalRosterEntry {
            proposal_id: 1,
            num_options: 2,
        }],
        hotkey_secret: with_hotkey.then(|| zeroize::Zeroizing::new(HOTKEY_SECRET.to_vec())),
    })
    .unwrap()
}

/// A host whose delegation driver stores a placeholder ZKP1 proof and then
/// interrupts `control`.
pub(super) fn interrupting_host(
    control: &ChainSubmissionControl,
    database: &Arc<VotingDb>,
) -> RoundHostContext {
    host_with_driver_target(
        control,
        Interrupt::Cancel,
        NETWORK,
        Some(
            VotingHotkey::from_stored_secret(&HOTKEY_SECRET, NETWORK)
                .unwrap()
                .delegation_target(),
        ),
        "wallet",
        database,
    )
}

use super::*;
use crate::{storage::queries, types::NoopProgressReporter};

/// Both public setup entry points accept unknown and mismatched informational IDs.
#[test]
fn setup_keeps_the_fixed_signing_profile_for_any_branch_hint() {
    let unknown_branch = 0xFFFF_FFFF;
    assert!(BranchId::try_from(unknown_branch).is_err());
    for branch in [unknown_branch, u32::from(BranchId::Nu7), 0] {
        for prepared_api in [false, true] {
            let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
            prepared.branch_id_provider = LightwalletdBranchIdProvider::resolved(branch);
            let (pczt_bytes, sighash, effects) = if prepared_api {
                let setup = prepared.setup(&db, &NoopProgressReporter).unwrap();
                (
                    setup.pczt_bytes,
                    setup.pczt_sighash.to_vec(),
                    setup.tx1_effects,
                )
            } else {
                let setup = db
                    .build_governance_pczt(
                        &prepared.round_id,
                        prepared.bundle_index,
                        &prepared.bundle_note_infos,
                        &prepared.delegation_keys,
                        branch,
                    )
                    .unwrap();
                (setup.pczt_bytes, setup.pczt_sighash, setup.tx1_effects)
            };
            let pczt = crate::action::pczt::Pczt::parse(&pczt_bytes).unwrap();
            assert_eq!(*pczt.global().tx_version(), 6);
            assert_eq!(
                *pczt.global().consensus_branch_id(),
                u32::from(BranchId::Nu6_3)
            );
            assert_eq!(pczt.ironwood().actions().len(), 1);
            crate::tx1::validate_tx1_effects(&effects).unwrap();
            assert_eq!(pczt_sighash(&pczt_bytes).unwrap().as_slice(), sighash);
            assert_eq!(
                queries::load_pczt_sighash(&db.conn(), &prepared.round_id, &db.wallet_id(), 0)
                    .unwrap(),
                sighash
            );

            // Changing the hint cannot replace an authorization that was already saved.
            prepared.branch_id_provider = LightwalletdBranchIdProvider::resolved(branch ^ 1);
            assert!(matches!(
                prepared.setup(&db, &NoopProgressReporter),
                Err(VotingError::SetupAlreadyPersisted { .. })
            ));
            assert_eq!(
                queries::load_pczt_sighash(&db.conn(), &prepared.round_id, &db.wallet_id(), 0)
                    .unwrap(),
                sighash
            );
        }
    }
}

/// An arbitrary branch hint cannot enable a pre-Ironwood or out-of-range snapshot.
#[test]
fn invalid_snapshot_is_rejected_before_setup_writes() {
    for (height, error_text) in [
        (
            u64::from(crate::types::REGTEST_NU6_3_ACTIVATION_HEIGHT) - 1,
            "before NU6.3/Ironwood activation",
        ),
        (u64::from(u32::MAX) + 1, "does not fit in u32"),
    ] {
        let (_, mut params, _, mut prepared) = prepared_wallet_delegation_fixture();
        params.snapshot_height = height;
        prepared.round_params = params.clone();
        prepared.branch_id_provider = LightwalletdBranchIdProvider::resolved(0xFFFF_FFFF);
        let db = VotingDb::open_in_memory().unwrap();
        db.set_wallet_id("branch-independent-snapshot");
        db.ensure_round(Network::Regtest, &params, None).unwrap();
        db.ensure_bundles(&prepared.round_id, &prepared.bundle_note_infos)
            .unwrap();
        let changes_before = db.conn().total_changes();
        let error = prepared.setup(&db, &NoopProgressReporter).unwrap_err();
        assert!(matches!(error, VotingError::InvalidInput { .. }), "{error}");
        assert!(error.to_string().contains(error_text), "{error}");
        let keystone_error = prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap_err();
        assert!(
            keystone_error.to_string().contains(error_text),
            "{keystone_error}"
        );
        assert_eq!(db.conn().total_changes(), changes_before);
        assert!(queries::load_padded_note_secrets_optional(
            &db.conn(),
            &prepared.round_id,
            &db.wallet_id(),
            0,
        )
        .unwrap()
        .is_none());
        assert!(
            queries::load_pczt_sighash(&db.conn(), &prepared.round_id, &db.wallet_id(), 0,)
                .is_err()
        );
    }
}

/// Legacy provider failures still stop setup before any writes.
#[test]
fn branch_provider_errors_still_propagate() {
    struct UnavailableProvider;
    impl BranchIdProvider for UnavailableProvider {
        /// Models a caller-owned provider that cannot obtain its metadata.
        fn consensus_branch_id(&self) -> Result<u32, VotingError> {
            Err(VotingError::InvalidInput {
                message: "provider unavailable".into(),
            })
        }
    }
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let changes_before = db.conn().total_changes();
    let error = setup(
        &db,
        &prepared.round_id,
        prepared.bundle_index,
        &prepared.bundle_note_infos,
        &prepared.delegation_keys,
        &UnavailableProvider,
        &NoopProgressReporter,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("provider unavailable"),
        "{error}"
    );
    assert_eq!(db.conn().total_changes(), changes_before);
}

/// External-signing requests build and reload the same PCZT without a branch hint check.
#[test]
fn keystone_requests_ignore_branch_hints_on_creation_and_reload() {
    for branch in [0xFFFF_FFFF, u32::from(BranchId::Nu7), 0] {
        let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
        prepared.branch_id_provider = LightwalletdBranchIdProvider::resolved(branch);
        let original = prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap();
        let pczt = crate::action::pczt::Pczt::parse(&original.pczt_bytes).unwrap();
        assert_eq!(*pczt.global().tx_version(), 6);
        assert_eq!(
            *pczt.global().consensus_branch_id(),
            u32::from(BranchId::Nu6_3)
        );
        assert_eq!(
            queries::load_pczt_sighash(&db.conn(), &prepared.round_id, &db.wallet_id(), 0).unwrap(),
            original.pczt_sighash
        );
        prepared.branch_id_provider = LightwalletdBranchIdProvider::resolved(branch ^ 1);
        let changes_before = db.conn().total_changes();
        let reloaded = prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap();
        assert_eq!(reloaded, original);
        assert_eq!(db.conn().total_changes(), changes_before);
    }
}

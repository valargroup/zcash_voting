use super::{tests::prepared_wallet_delegation_fixture, *};
use crate::{storage::queries, types::NoopProgressReporter};
use std::sync::{Arc, Barrier};

#[test]
fn keystone_request_reuses_warmed_setup_and_survives_restart() {
    assert_request_survives_restart(false);
}

#[test]
fn ledger_request_reuses_warmed_setup_and_survives_restart() {
    assert_request_survives_restart(true);
}

fn assert_request_survives_restart(ledger_output_review: bool) {
    let (_, params, _, mut prepared) = prepared_wallet_delegation_fixture();
    if ledger_output_review {
        prepared.delegation_keys = prepared.delegation_keys.with_ledger_output_review();
    }
    let path =
        std::env::temp_dir().join(format!("keystone-warmup-{}.sqlite", uuid::Uuid::new_v4()));
    let path = path.to_str().unwrap();
    let db = VotingDb::open(path).unwrap();
    db.set_wallet_id("keystone");
    db.ensure_round(Network::Regtest, &params, None).unwrap();
    db.ensure_bundles(&prepared.round_id, &prepared.bundle_note_infos)
        .unwrap();
    let setup = prepared.setup(&db, &NoopProgressReporter).unwrap();
    // The actual proof coordination and generation are covered by the prover
    // suite. Here a durable proof row exercises post-proof request retrieval.
    queries::store_proof(
        &db.conn(),
        &prepared.round_id,
        "keystone",
        0,
        &[0xAB; 96],
        None,
    )
    .unwrap();
    let before = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert_eq!(before.pczt_bytes, setup.pczt_bytes);
    assert_eq!(before.pczt_sighash, setup.pczt_sighash);
    let memo = if ledger_output_review {
        ledger_display_memo
    } else {
        display_memo
    };
    assert_eq!(
        before.display_memo,
        memo(
            &prepared.round_name,
            crate::round::raw_bundle_weight(&prepared.bundle_note_infos).unwrap()
        )
    );
    drop(db);
    let reopened = VotingDb::open(path).unwrap();
    reopened.set_wallet_id("keystone");
    // A refreshed host title must not change the metadata of the saved request.
    prepared.round_name = "Renamed after restart".to_string();
    prepared.delegation_keys.round_name = prepared.round_name.clone();
    let after = prepared
        .keystone_request(&reopened, &NoopProgressReporter)
        .unwrap();
    assert_eq!(before, after);
    assert!(reopened
        .delegation_phase(&prepared.round_id, 0)
        .unwrap()
        .has_persisted_proof());
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn keystone_request_recovers_truncated_unicode_memo() {
    let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
    prepared.round_name = "投票🗳".repeat(200);
    prepared.delegation_keys.round_name = prepared.round_name.clone();
    let expected = display_memo(
        &prepared.round_name,
        crate::round::raw_bundle_weight(&prepared.bundle_note_infos).unwrap(),
    );
    let before = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert_eq!(before.display_memo, expected);
    prepared.round_name = "Updated title".to_string();
    prepared.delegation_keys.round_name = prepared.round_name.clone();
    let after = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn keystone_request_rejects_unrecoverable_persisted_memo() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    let pczt = crate::action::pczt::Pczt::parse(&original.pczt_bytes).unwrap();
    let pczt = crate::action::pczt::roles::redactor::Redactor::new(pczt)
        .redact_ironwood_with(|mut bundle| {
            bundle.redact_action(original.action_index as usize, |mut action| {
                action.clear_output_recipient();
            });
        })
        .finish()
        .serialize()
        .unwrap();
    // Removing recovery metadata leaves the signed transaction hash unchanged.
    assert_eq!(
        pczt_sighash(&pczt).unwrap().as_slice(),
        original.pczt_sighash
    );
    db.conn()
        .execute("UPDATE bundles SET delegation_pczt = ?1", [&pczt])
        .unwrap();
    assert!(matches!(
        prepared.keystone_request(&db, &NoopProgressReporter),
        Err(VotingError::Internal { message })
            if message == "persisted delegation PCZT memo cannot be recovered"
    ));
}

#[test]
fn concurrent_keystone_requests_reuse_setup_after_busy_retry() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let db = Arc::new(db);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            let prepared = prepared.clone();
            std::thread::spawn(move || {
                barrier.wait();
                prepared.keystone_request(&db, &NoopProgressReporter)
            })
        })
        .collect();
    let attempts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(attempts.iter().any(Result::is_ok));
    let requests: Vec<_> = attempts
        .into_iter()
        .map(|attempt| match attempt {
            Ok(request) => request,
            Err(VotingError::Busy { .. }) => prepared
                .keystone_request(&db, &NoopProgressReporter)
                .unwrap(),
            Err(error) => panic!("unexpected request failure: {error}"),
        })
        .collect();
    assert_eq!(requests[0], requests[1]);
}

#[test]
fn missing_keystone_transaction_fails_without_rebuilding() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let original = prepared.setup(&db, &NoopProgressReporter).unwrap();
    db.conn()
        .execute("UPDATE bundles SET delegation_pczt = NULL", [])
        .unwrap();
    let error = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap_err();
    assert!(matches!(
        error,
        VotingError::DelegationPcztUnavailable { .. }
    ));
    assert_eq!(
        queries::load_pczt_sighash(&db.conn(), &prepared.round_id, &db.wallet_id(), 0).unwrap(),
        original.pczt_sighash
    );
    assert!(!error.retryable());
}

#[test]
fn setup_replacement_persists_the_replacement_keystone_transaction() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    let mut replacement = prepared.clone();
    replacement.delegation_keys.hotkey_raw_address =
        *crate::VotingHotkey::from_stored_secret(&[0x47; 64], Network::Regtest)
            .unwrap()
            .raw_orchard_address();
    let setup = replacement.setup(&db, &NoopProgressReporter).unwrap();
    let request = replacement
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert_ne!(request.pczt_bytes, original.pczt_bytes);
    assert_eq!(request.pczt_bytes, setup.pczt_bytes);
    assert_eq!(request.pczt_sighash, setup.pczt_sighash);
    assert!(prepared
        .keystone_request(&db, &NoopProgressReporter)
        .is_err());
}

#[test]
fn persisted_keystone_request_rejects_changed_notes_target_and_bytes() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    let mut other_notes = prepared.clone();
    other_notes.bundle_note_infos[0].value += 1;
    assert!(other_notes
        .keystone_request(&db, &NoopProgressReporter)
        .is_err());
    let mut other_target = prepared.clone();
    other_target.delegation_keys.hotkey_raw_address =
        *crate::VotingHotkey::from_stored_secret(&[0x47; 64], Network::Regtest)
            .unwrap()
            .raw_orchard_address();
    assert!(other_target
        .keystone_request(&db, &NoopProgressReporter)
        .is_err());
    assert_eq!(
        prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap(),
        original
    );
    db.conn()
        .execute("UPDATE bundles SET delegation_pczt = X'00'", [])
        .unwrap();
    assert!(prepared
        .keystone_request(&db, &NoopProgressReporter)
        .is_err());
}

#[test]
fn observed_keystone_requests_preserve_durable_reuse_and_missing_pczt_errors() {
    let (db, _, _, prepared) = prepared_wallet_delegation_fixture();
    let mut original = None;
    for warmed in [false, true] {
        if warmed {
            queries::store_proof(
                &db.conn(),
                &prepared.round_id,
                &db.wallet_id(),
                0,
                &[0xAB; 96],
                None,
            )
            .unwrap();
        }
        let invocation =
            crate::ObservationScope::new(Some(crate::ObservabilityOptions::default())).invocation();
        let request = prepared
            .observe_keystone_request(&db, &NoopProgressReporter, invocation.scope())
            .unwrap();
        assert_eq!(
            request,
            prepared
                .keystone_request(&db, &NoopProgressReporter)
                .unwrap()
        );
        if let Some(original) = &original {
            assert_eq!(&request, original);
        } else {
            original = Some(request.clone());
        }
        let diagnostics = invocation
            .complete(
                "keystone_request",
                crate::ObservationOutcome::Succeeded,
                request,
            )
            .observability
            .unwrap();
        assert_eq!(
            diagnostics.round_id.as_deref(),
            Some(prepared.round_id.as_str())
        );
        let stage = diagnostics
            .records
            .iter()
            .find(|record| record.stage.as_ref() == "delegation::keystone_request")
            .unwrap();
        assert_eq!(stage.outcome, crate::ObservationOutcome::Succeeded);
        assert_eq!(stage.attribution.bundle_index, Some(0));
        assert_eq!(
            diagnostics
                .records
                .iter()
                .any(|record| record.stage.as_ref() == "delegation::setup"),
            !warmed
        );
    }
    db.conn()
        .execute("UPDATE bundles SET delegation_pczt = NULL", [])
        .unwrap();
    let invocation =
        crate::ObservationScope::new(Some(crate::ObservabilityOptions::default())).invocation();
    let failure = prepared
        .observe_keystone_request(&db, &NoopProgressReporter, invocation.scope())
        .unwrap_err();
    assert!(matches!(
        failure,
        VotingError::DelegationPcztUnavailable { .. }
    ));
    let diagnostics = invocation
        .complete(
            "keystone_request",
            crate::ObservationOutcome::Failed,
            failure,
        )
        .observability
        .unwrap();
    let stage = diagnostics
        .records
        .iter()
        .find(|record| record.stage.as_ref() == "delegation::keystone_request")
        .unwrap();
    assert_eq!(stage.outcome, crate::ObservationOutcome::Failed);
    assert_eq!(
        stage.error_kind.as_deref(),
        Some("DelegationPcztUnavailable")
    );
}

#[test]
fn ledger_recovers_an_unsigned_default_request_once() {
    let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    queries::store_proof(
        &db.conn(),
        &prepared.round_id,
        &db.wallet_id(),
        0,
        &[0xAB; 96],
        None,
    )
    .unwrap();
    prepared.delegation_keys.round_name = "Updated after restart".into();
    prepared.delegation_keys = prepared.delegation_keys.with_ledger_output_review();
    let repaired = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert_ne!(repaired.pczt_sighash, original.pczt_sighash);
    assert_eq!(
        repaired.display_memo,
        original.display_memo.replace('\n', " ")
    );
    assert_eq!(
        db.delegation_phase(&prepared.round_id, 0).unwrap(),
        DelegationPhase::PcztBuilt
    );
    assert!(crate::delegate::keystone::ledger_can_review(
        &repaired.pczt_bytes,
        repaired.action_index as usize,
        &prepared.delegation_keys,
        &repaired.display_memo
    )
    .unwrap());
    assert_eq!(
        prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap(),
        repaired
    );
}

#[test]
fn ledger_recovery_preserves_a_signed_or_submitted_default_request() {
    for signed in [true, false] {
        let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
        let original = prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap();
        if signed {
            queries::store_keystone_signature(
                &db.conn(),
                &prepared.round_id,
                &db.wallet_id(),
                0,
                &[0xAB; 64],
                &original.pczt_sighash,
                &original.rk,
            )
            .unwrap();
        } else {
            queries::store_delegation_tx_hash(
                &db.conn(),
                &prepared.round_id,
                &db.wallet_id(),
                0,
                &"ab".repeat(32),
            )
            .unwrap();
        }
        let default_keys = prepared.delegation_keys.clone();
        prepared.delegation_keys = default_keys.clone().with_ledger_output_review();
        let error = prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap_err();
        if signed {
            assert!(matches!(error, VotingError::InvalidInput { .. }));
            assert_eq!(
                queries::get_keystone_signatures(&db.conn(), &prepared.round_id, &db.wallet_id())
                    .unwrap()
                    .len(),
                1
            );
        } else {
            assert!(matches!(
                error,
                VotingError::DelegationAlreadyBroadcast { .. }
            ));
        }
        prepared.delegation_keys = default_keys;
        assert_eq!(
            prepared
                .keystone_request(&db, &NoopProgressReporter)
                .unwrap(),
            original
        );
    }
}

#[test]
fn ledger_recovery_refuses_a_different_account_derivation() {
    let (db, _, _, mut prepared) = prepared_wallet_delegation_fixture();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    let saved_keys = prepared.delegation_keys.clone();
    prepared.delegation_keys = saved_keys.clone().with_ledger_output_review();
    prepared.delegation_keys.account_index += 1;
    assert!(matches!(
        prepared.keystone_request(&db, &NoopProgressReporter),
        Err(VotingError::InvalidInput { .. })
    ));
    prepared.delegation_keys = saved_keys;
    assert_eq!(
        prepared
            .keystone_request(&db, &NoopProgressReporter)
            .unwrap(),
        original
    );
}

#[test]
fn device_rejection_can_replace_only_the_current_request_once_across_restart() {
    let (_, params, _, mut prepared) = prepared_wallet_delegation_fixture();
    prepared.delegation_keys = prepared.delegation_keys.with_ledger_output_review();
    let path = std::env::temp_dir().join(format!("ledger-retry-{}.sqlite", uuid::Uuid::new_v4()));
    let open = || {
        let db = VotingDb::open(path.to_str().unwrap()).unwrap();
        db.set_wallet_id("ledger");
        db
    };
    let db = open();
    db.ensure_round(Network::Regtest, &params, None).unwrap();
    db.ensure_bundles(&prepared.round_id, &prepared.bundle_note_infos)
        .unwrap();
    let original = prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert!(keystone::after_ledger_rejection(&prepared, &db, &[0; 32]).is_err());
    let replaced = keystone::after_ledger_rejection(
        &prepared,
        &db,
        original.pczt_sighash.as_slice().try_into().unwrap(),
    )
    .unwrap();
    assert_ne!(original.pczt_sighash, replaced.pczt_sighash);
    assert_eq!(replaced.display_memo, original.display_memo);
    drop(db);
    let reopened = open();
    assert!(keystone::after_ledger_rejection(
        &prepared,
        &reopened,
        replaced.pczt_sighash.as_slice().try_into().unwrap()
    )
    .is_err());
    assert_eq!(
        prepared
            .keystone_request(&reopened, &NoopProgressReporter)
            .unwrap(),
        replaced
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn saved_authorization_title_is_bounded_by_the_verified_amount_suffix() {
    let weight = 987_654_321;
    for title in [
        "round-id",
        "First.\nAmount: 1 ZEC. 第二",
        &"投票".repeat(512),
    ] {
        let memo = display_memo(title, weight);
        let (visible, legacy) = keystone::saved_title(&memo, weight).unwrap();
        assert!(legacy);
        assert!(title.starts_with(visible));
        for separator in [". ", ".\r\n"] {
            let (before, after) = memo.rsplit_once(".\nAmount:").unwrap();
            let variant = format!("{before}{separator}Amount:{after}");
            assert_eq!(
                keystone::saved_title(&variant, weight),
                Some((visible, false))
            );
        }
        assert!(keystone::saved_title(&memo, weight + 1).is_none());
        assert!(keystone::saved_title(&memo.replace("authorizing", "revoking"), weight).is_none());
    }
}

#[test]
#[ignore = "generates a real delegation proof; run with make proofs"]
fn repair_preserves_note_witnesses_for_direct_proving() {
    use crate::backend::{
        orchard::tree::MerkleHashOrchard,
        pasta_curves::{group::ff::PrimeField, pallas},
    };
    use voting_circuits::delegation::{ImtProvider, SpacedLeafImtProvider};
    use voting_crypto_deps::incrementalmerkletree::{Hashable, Level};
    struct Snapshot(SpacedLeafImtProvider);
    impl crate::pir::PirProofSource for Snapshot {
        fn circuit_root(&self) -> pallas::Base {
            self.0.root()
        }
        fn fetch_proofs(
            &self,
            nfs: &[pallas::Base],
        ) -> anyhow::Result<Vec<crate::pir::ImtProofData>> {
            nfs.iter()
                .map(|nf| {
                    let proof = self.0.non_membership_proof(*nf).unwrap();
                    Ok(crate::pir::ImtProofData {
                        root: proof.root,
                        nf_bounds: proof.nf_bounds,
                        leaf_pos: proof.leaf_pos,
                        path: proof.path,
                    })
                })
                .collect()
        }
    }
    let (_, mut params, _, mut prepared) = prepared_wallet_delegation_fixture();
    prepared.bundle_note_infos.truncate(1);
    let note = &prepared.bundle_note_infos[0];
    let mut root =
        MerkleHashOrchard::from_bytes(&note.commitment.as_slice().try_into().unwrap()).unwrap();
    let mut auth_path = Vec::new();
    for level in 0..32 {
        let sibling = MerkleHashOrchard::empty_root(Level::from(level));
        auth_path.push(sibling.to_bytes().to_vec());
        root = if (note.position >> level) & 1 == 0 {
            MerkleHashOrchard::combine(Level::from(level), &root, &sibling)
        } else {
            MerkleHashOrchard::combine(Level::from(level), &sibling, &root)
        };
    }
    let pir = Snapshot(SpacedLeafImtProvider::new());
    params.nc_root = root.to_bytes().to_vec();
    params.nullifier_imt_root = pir.0.root().to_repr().to_vec();
    let db = VotingDb::open_in_memory().unwrap();
    db.set_wallet_id("ledger");
    db.ensure_round(Network::Regtest, &params, None).unwrap();
    db.ensure_bundles(&prepared.round_id, &prepared.bundle_note_infos)
        .unwrap();
    db.store_witnesses(
        &prepared.round_id,
        0,
        &[crate::types::WitnessData {
            position: note.position,
            note_commitment: note.commitment.clone(),
            root: params.nc_root,
            auth_path,
        }],
    )
    .unwrap();
    prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    prepared.delegation_keys = prepared.delegation_keys.with_ledger_output_review();
    prepared
        .keystone_request(&db, &NoopProgressReporter)
        .unwrap();
    assert!(!prepared
        .prove(&db, &pir, &NoopProgressReporter)
        .unwrap()
        .bytes
        .is_empty());
}

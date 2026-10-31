use super::*;

/// NU7 and far-future snapshots keep the original voting authorization format.
#[test]
fn later_snapshots_keep_the_fixed_tx1_profile() {
    for snapshot_height in [
        u64::from(crate::types::REGTEST_NU7_ACTIVATION_HEIGHT),
        u64::from(u32::MAX),
    ] {
        let notes = [mock_note()];
        let mut params = mock_params();
        params.snapshot_height = snapshot_height;

        let result = build_governance_pczt(
            &notes,
            &params,
            VotingNetwork::Regtest,
            &mock_fvk_bytes(),
            &mock_hotkey_address(),
            VotingNetwork::Regtest.network_type().coin_type(),
            &MOCK_SEED_FP,
            MOCK_ACCOUNT,
            "Test Round",
            &sample_padded_note_secrets(notes.len()).unwrap(),
            None,
            false,
        )
        .unwrap();

        // Vote-chain verifiers rebuild the sighash with the NU6.3 profile, so
        // an NU7 branch here would produce a signature the chain rejects.
        let pczt = pczt::Pczt::parse(&result.pczt_bytes).unwrap();
        assert_eq!(*pczt.global().tx_version(), 6);
        assert_eq!(
            *pczt.global().consensus_branch_id(),
            u32::from(BranchId::Nu6_3)
        );
        assert_eq!(pczt.ironwood().actions().len(), 1);
        crate::tx1::validate_tx1_effects(&result.tx1_effects).unwrap();
    }
}

/// The internal builder also rejects pre-Ironwood snapshots.
#[test]
fn test_build_governance_pczt_rejects_snapshot_before_activation() {
    let mut params = mock_params();
    params.snapshot_height = u64::from(crate::types::REGTEST_NU6_3_ACTIVATION_HEIGHT) - 1;

    let err = build_governance_pczt(
        &[mock_note()],
        &params,
        VotingNetwork::Regtest,
        &mock_fvk_bytes(),
        &mock_hotkey_address(),
        VotingNetwork::Regtest.network_type().coin_type(),
        &MOCK_SEED_FP,
        MOCK_ACCOUNT,
        "Test Round",
        &sample_padded_note_secrets(1).unwrap(),
        None,
        false,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("before NU6.3/Ironwood activation"),
        "{err}"
    );
}

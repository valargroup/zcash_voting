use super::*;

#[test]
fn test_build_governance_pczt_signs_nu7_snapshot_under_tx1_profile() {
    let notes = [mock_note()];
    let mut params = mock_params();
    params.snapshot_height = u64::from(crate::types::REGTEST_NU7_ACTIVATION_HEIGHT);

    let result = build_governance_pczt(
        &notes,
        &params,
        VotingNetwork::Regtest,
        &mock_fvk_bytes(),
        &mock_hotkey_address(),
        u32::from(BranchId::Nu7),
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

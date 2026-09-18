use super::*;

#[test]
fn default_output_is_not_recoverable_from_published_effects() {
    assert_output_recovery(false);
}

#[test]
fn ledger_output_is_recoverable_from_published_effects() {
    assert_output_recovery(true);
}

fn assert_output_recovery(ledger_output_review: bool) {
    let result = build_governance_pczt(
        &[mock_note()],
        &mock_nu6_3_params(),
        VotingNetwork::Regtest,
        &mock_fvk_bytes(),
        &mock_hotkey_address(),
        u32::from(BranchId::Nu6_3),
        VotingNetwork::Regtest.network_type().coin_type(),
        &MOCK_SEED_FP,
        MOCK_ACCOUNT,
        "Test Round",
        &sample_padded_note_secrets(1).unwrap(),
        None,
        ledger_output_review,
    )
    .unwrap();
    assert_eq!(
        extract_pczt_sighash(&result.pczt_bytes).unwrap().to_vec(),
        result.pczt_sighash
    );
    let fvk = FullViewingKey::from_bytes(&mock_fvk_bytes().try_into().unwrap()).unwrap();
    let ovk = fvk.to_ovk(Scope::External);

    for index in 0..crate::tx1::TX1_ACTION_COUNT {
        let start = 1 + index * crate::tx1::TX1_ACTION_EFFECTS_LEN;
        let action = Action::from_parts(
            Nullifier::from_bytes(
                result.tx1_effects[start + 32..start + 64]
                    .try_into()
                    .unwrap(),
            )
            .unwrap(),
            VerificationKey::<SpendAuth>::try_from(
                <[u8; 32]>::try_from(&result.tx1_effects[start + 64..start + 96]).unwrap(),
            )
            .unwrap(),
            ExtractedNoteCommitment::from_bytes(
                result.tx1_effects[start + 96..start + 128]
                    .try_into()
                    .unwrap(),
            )
            .unwrap(),
            TransmittedNoteCiphertext {
                epk_bytes: result.tx1_effects[start + 128..start + 160]
                    .try_into()
                    .unwrap(),
                enc_ciphertext: result.tx1_effects[start + 160..start + 740]
                    .try_into()
                    .unwrap(),
                out_ciphertext: result.tx1_effects[start + 740..start + 820]
                    .try_into()
                    .unwrap(),
            },
            ValueCommitment::from_bytes(result.tx1_effects[start..start + 32].try_into().unwrap())
                .unwrap(),
            (),
        )
        .unwrap();

        assert_eq!(
            try_output_recovery_with_ovk(
                &IronwoodDomain::for_action(&action),
                &ovk,
                &action,
                action.cv_net(),
                &action.encrypted_note().out_ciphertext,
            )
            .is_some(),
            ledger_output_review,
            "unexpected account-OVK recovery for action {index}"
        );
    }
}

#[test]
fn ledger_memo_retains_round_and_amount_without_entering_device_hash_path() {
    let memo = crate::delegate::ledger_display_memo("Test Round", 13_000_000);
    assert_eq!(memo, "I am authorizing this hotkey managed by my wallet to vote on Test Round. Amount: 0.13000000 ZEC.");
    let escaped = crate::delegate::ledger_display_memo(&"投票\n".repeat(200), 123_456_789);
    assert!(escaped.len() <= 512);
    assert!(escaped.bytes().all(|byte| matches!(byte, 0x20..=0x7e)));
    assert!(escaped.contains("\\u{6295}"));
    assert!(escaped.ends_with(" Amount: 1.23456789 ZEC."));
}

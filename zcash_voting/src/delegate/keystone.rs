//! Reuse the exact durable transaction at the external signing boundary.
use super::*;
use crate::action::{
    orchard::note::NoteVersion,
    pczt::{orchard::EncCiphertext, roles::redactor::Redactor, Pczt},
};

pub(super) fn request(
    prepared: &PreparedDelegationBundle,
    voting_db: &VotingDb,
    stages: &dyn DelegationProgressReporter,
    observations: &crate::ObservationScope,
) -> Result<KeystoneSigningRequest, VotingError> {
    let scoped_db = voting_db.scoped(&voting_db.wallet_id())?;
    let voting_db = &scoped_db;
    // The setup lease can return Busy during background setup. A later retry
    // reloads the winner's exact PCZT without waiting for proof generation.
    if voting_db.delegation_phase(&prepared.round_id, prepared.bundle_index)?
        == DelegationPhase::Prepared
    {
        match prepared.observe_setup(voting_db, stages, observations) {
            Ok(_) | Err(VotingError::SetupAlreadyPersisted { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    let request = load_request(prepared, voting_db)?;
    if prepared.delegation_keys.ledger_output_review
        && !ledger_can_review(
            &request.pczt_bytes,
            request.action_index as usize,
            &prepared.delegation_keys,
            &request.display_memo,
        )?
    {
        // Only regenerate the known default signing profile. A malformed or
        // unfamiliar saved request needs inspection before replacing anything.
        let weight = crate::round::raw_bundle_weight(&prepared.bundle_note_infos)?;
        if prepared.delegation_keys.van_blinding_key.is_none()
            || request.display_memo != display_memo(&prepared.delegation_keys.round_name, weight)
        {
            return Err(VotingError::InvalidInput {
                message: "cannot identify the saved delegation signing profile".into(),
            });
        }
        verify_saved_account(&request, &prepared.delegation_keys)?;
        let repaired = voting_db.build_or_repair_governance_pczt(
            &prepared.round_id,
            prepared.bundle_index,
            &prepared.bundle_note_infos,
            &prepared.delegation_keys,
            Some(&request.pczt_sighash),
        )?;
        let current = load_request(prepared, voting_db)?;
        if current.pczt_sighash != repaired.pczt_sighash {
            return Err(VotingError::Busy {
                message: "delegation changed during Ledger recovery".into(),
            });
        }
        return Ok(current);
    }
    Ok(request)
}

fn load_request(
    prepared: &PreparedDelegationBundle,
    voting_db: &VotingDb,
) -> Result<KeystoneSigningRequest, VotingError> {
    // Validation and loading must observe the same setup if another connection
    // replaces an unbroadcast bundle for a different hotkey.
    let identity = DelegationProofIdentity::new(
        voting_db.sidecar_id(),
        voting_db.wallet_id(),
        &prepared.round_id,
        prepared.bundle_index,
    );
    let (pczt_bytes, stored_sighash, stored_rk) = voting_db.validated_delegation_pczt(
        &identity,
        &prepared.bundle_note_infos,
        &prepared.delegation_keys,
    )?;
    let persisted_sighash = array32("pczt_sighash", stored_sighash)?;
    let recomputed_sighash = pczt_sighash(&pczt_bytes)?;
    if recomputed_sighash != persisted_sighash {
        return Err(VotingError::Internal {
            message: "persisted delegation PCZT sighash does not match stored setup".to_string(),
        });
    }
    let rk = array32("rk", stored_rk)?;
    let action_index = crate::action::delegation_pczt_action_index(&pczt_bytes, &rk)?;
    let redacted_pczt_bytes = redact_delegation_pczt_for_signer(&pczt_bytes)?;
    let display_memo = persisted_display_memo(&pczt_bytes, action_index)?;
    let action_index =
        crate::wire::BoundedU32::try_from(action_index).map_err(|_| VotingError::InvalidInput {
            message: format!("action_index {action_index} does not fit u32"),
        })?;

    Ok(KeystoneSigningRequest {
        pczt_bytes,
        redacted_pczt_bytes,
        pczt_sighash: persisted_sighash.to_vec(),
        rk: rk.to_vec(),
        action_index: action_index.0,
        display_memo,
        eligible_weight_zatoshi: prepared.eligible_weight_zatoshi(),
        delegated_weight_zatoshi: prepared.delegated_weight_zatoshi()?,
        bundle_count: prepared.layout.bundle_count,
        bundle_index: prepared.bundle_index,
    })
}

fn verify_saved_account(
    request: &KeystoneSigningRequest,
    keys: &DelegationKeys,
) -> Result<(), VotingError> {
    use crate::action::pczt::roles::verifier::{OrchardError, Verifier};
    use voting_crypto_deps::zip32::{fingerprint::SeedFingerprint, ChildIndex};
    let pczt = Pczt::parse(&request.pczt_bytes).map_err(|e| VotingError::Internal {
        message: format!("invalid saved delegation: {e:?}"),
    })?;
    Verifier::new(pczt)
        .with_ironwood(|bundle| {
            let spend = bundle
                .actions()
                .get(request.action_index as usize)
                .ok_or(OrchardError::Custom(()))?
                .spend();
            let account = spend.zip32_derivation().as_ref().and_then(|path| {
                path.extract_account_index(
                    &SeedFingerprint::from_bytes(keys.seed_fingerprint),
                    ChildIndex::hardened(keys.coin_type),
                )
            });
            let same_viewing_key = spend
                .fvk()
                .as_ref()
                .is_some_and(|fvk| fvk.to_bytes().as_slice() == keys.fvk_bytes.as_slice());
            if account.map(u32::from) != Some(keys.account_index)
                || !same_viewing_key
                || spend.spend_auth_sig().is_some()
            {
                return Err(OrchardError::Custom(()));
            }
            Ok(())
        })
        .map_err(|_| VotingError::InvalidInput {
            message: "saved delegation must be unsigned and belong to the selected Ledger account"
                .into(),
        })?;
    Ok(())
}

/// Check the ciphertext the device receives with the account's external outgoing viewing key.
pub(crate) fn ledger_can_review(
    bytes: &[u8],
    index: usize,
    keys: &DelegationKeys,
    display: &str,
) -> Result<bool, VotingError> {
    use crate::action::orchard::keys::FullViewingKey;
    use voting_crypto_deps::zip32::Scope;
    let key = keys
        .fvk_bytes
        .as_slice()
        .try_into()
        .ok()
        .and_then(FullViewingKey::from_bytes)
        .ok_or_else(|| VotingError::InvalidInput {
            message: "invalid delegation viewing key".into(),
        })?;
    let tx = Pczt::parse(bytes)
        .map_err(|e| VotingError::Internal {
            message: format!("invalid delegation PCZT: {e:?}"),
        })?
        .into_effects()
        .map_err(|e| VotingError::Internal {
            message: format!("invalid delegation effects: {e:?}"),
        })?;
    let bundle = tx.ironwood_bundle().ok_or_else(|| VotingError::Internal {
        message: "missing delegation output".into(),
    })?;
    let Some((note, address, memo)) =
        bundle.recover_output_with_ovk(index, &key.to_ovk(Scope::External))
    else {
        return Ok(false);
    };
    if address.to_raw_address_bytes() != keys.hotkey_raw_address
        || note.value().inner() != 0
        || !memo.starts_with(display.as_bytes())
        || memo[display.len()..].iter().any(|b| *b != 0)
    {
        return Err(VotingError::InvalidInput {
            message: "recovered delegation output differs from the signing request".into(),
        });
    }
    Ok(display.bytes().all(|b| (0x20..=0x7e).contains(&b)))
}

/// Recover the signed memo without changing the persisted or signer-facing PCZT.
fn persisted_display_memo(pczt_bytes: &[u8], action_index: usize) -> Result<String, VotingError> {
    let pczt = Pczt::parse(pczt_bytes).map_err(|error| VotingError::Internal {
        message: format!("failed to parse persisted delegation PCZT: {error:?}"),
    })?;
    let pczt = Redactor::new(pczt)
        .redact_ironwood_with(|mut bundle| {
            bundle.redact_action(action_index, |mut action| {
                action.replace_enc_ciphertext_with_decrypted_memo_plaintext(NoteVersion::V3);
            });
        })
        .finish();
    let action =
        pczt.ironwood()
            .actions()
            .get(action_index)
            .ok_or_else(|| VotingError::Internal {
                message: "persisted delegation PCZT has no governance action".to_string(),
            })?;
    let EncCiphertext::MemoPlaintext(memo) = action.output().enc_ciphertext() else {
        return Err(VotingError::Internal {
            message: "persisted delegation PCZT memo cannot be recovered".to_string(),
        });
    };
    std::str::from_utf8(memo.as_stripped_bytes())
        .map(str::to_owned)
        .map_err(|_| VotingError::Internal {
            message: "persisted delegation PCZT memo is not UTF-8 text".to_string(),
        })
}

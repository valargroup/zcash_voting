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
    if !prepared.delegation_keys.ledger_output_review {
        return Ok(request);
    }
    ledger_request(prepared, voting_db, request, None)
}

/// The specific saved generation checked before building a candidate.
#[derive(Clone, Copy)]
pub(crate) struct LedgerRepair<'a> {
    pub(crate) previous_hash: &'a [u8],
    pub(crate) notes: &'a [NoteInfo],
    pub(crate) after_device_rejection: bool,
}

pub(crate) fn after_ledger_rejection(
    prepared: &PreparedDelegationBundle,
    voting_db: &VotingDb,
    rejected_hash: &[u8; 32],
) -> Result<KeystoneSigningRequest, VotingError> {
    if !prepared.delegation_keys.ledger_output_review {
        return Err(VotingError::InvalidInput {
            message: "Ledger output review is required".into(),
        });
    }
    let db = voting_db.scoped(&voting_db.wallet_id())?;
    let request = load_request(prepared, &db)?;
    if request.pczt_sighash.as_slice() != rejected_hash {
        return Err(VotingError::Busy {
            message: "rejected delegation is no longer current".into(),
        });
    }
    ledger_request(prepared, &db, request, Some(rejected_hash))
}

// Match the fixed authorization and the entire amount suffix. A title may
// itself contain a newline, punctuation, or the word "Amount:".
pub(super) fn saved_title(memo: &str, weight: u64) -> Option<(&str, bool)> {
    let empty_title = display_memo("", weight);
    let amount = empty_title
        .strip_prefix(DISPLAY_MEMO_PREFIX)?
        .strip_prefix(".\n")?;
    let title_and_separator = memo
        .strip_prefix(DISPLAY_MEMO_PREFIX)?
        .strip_suffix(amount)?;
    for (separator, legacy) in [(".\r\n", false), (".\n", true), (". ", false)] {
        if let Some(title) = title_and_separator.strip_suffix(separator) {
            return Some((title, legacy));
        }
    }
    None
}

fn ledger_request(
    prepared: &PreparedDelegationBundle,
    voting_db: &VotingDb,
    request: KeystoneSigningRequest,
    rejected_hash: Option<&[u8; 32]>,
) -> Result<KeystoneSigningRequest, VotingError> {
    let keys = &prepared.delegation_keys;
    let reviewable = ledger_can_review(
        &request.pczt_bytes,
        request.action_index as usize,
        keys,
        &request.display_memo,
    )?;
    let weight = crate::round::raw_bundle_weight(&prepared.bundle_note_infos)?;
    let (title, default_profile) =
        saved_title(&request.display_memo, weight).ok_or_else(|| VotingError::InvalidInput {
            message: "cannot verify saved delegation authorization and amount".into(),
        })?;
    if reviewable && rejected_hash.is_none() {
        return Ok(request);
    }
    verify_saved_account(&request, keys)?;
    if rejected_hash.is_none() && !default_profile {
        // The saved authorization is understood, but only a device rejection
        // warrants replacing this alternate formatting.
        return Ok(request);
    }
    if keys.van_blinding_key.is_none() {
        return Err(VotingError::InvalidInput {
            message: "Ledger recovery requires the local voting hotkey".into(),
        });
    }
    let mut keys = keys.clone();
    keys.round_name = title.to_owned();
    let repaired = voting_db.build_or_repair_governance_pczt(
        &prepared.round_id,
        prepared.bundle_index,
        &prepared.bundle_note_infos,
        &keys,
        Some(LedgerRepair {
            previous_hash: &request.pczt_sighash,
            notes: &prepared.bundle_note_infos,
            after_device_rejection: rejected_hash.is_some(),
        }),
    )?;
    let current = load_request(prepared, voting_db)?;
    if current.pczt_sighash != repaired.pczt_sighash {
        return Err(VotingError::Busy {
            message: "delegation changed during Ledger recovery".into(),
        });
    }
    Ok(current)
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
    let action = pczt
        .ironwood()
        .sole_action()
        .ok_or_else(|| VotingError::InvalidInput {
            message: "expected one saved delegation action".into(),
        })?;
    if action.output().recipient().as_ref() != Some(&keys.hotkey_raw_address)
        || *action.output().value() != Some(0)
    {
        return Err(VotingError::InvalidInput {
            message: "saved delegation output differs from the voting target".into(),
        });
    }
    Verifier::new(pczt)
        .with_ironwood(|bundle| {
            let action = bundle
                .actions()
                .get(request.action_index as usize)
                .ok_or(OrchardError::Custom(()))?;
            let spend = action.spend();
            action
                .output()
                .verify_note_commitment(spend)
                .map_err(|_| OrchardError::Custom(()))?;
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

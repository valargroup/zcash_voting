//! A proved batch's signed attempt. Every DC signs D1 (the batch plus an
//! expiry) under its stored randomizer, and the attempt is persisted before
//! the POST that carries it (contract §4.1, §11.3).
//!
//! The first attempt fixes the batch's identity, which is its `batch_digest`
//! (D1 with expiry 0), chain id and any registration SpendAuth signature. A
//! later attempt only re-signs D1 with a later expiry, with no new proof or
//! device signature. A new attempt is signed only while nothing of the batch
//! is on the wire and the stored one has expired, so one attempt is live at
//! a time.

use rusqlite::{named_params, Connection, OptionalExtension};
use zcash_vote_delegate::{
    proxy_batch_digest, proxy_batch_sighash, ProxyDelegationActionFields,
    ProxyDelegationBatchFields,
};

use super::{
    batch_store,
    slot_plan::{decode_round_id, load_planned_slots},
    ProvedProxyBatch,
};
use crate::{
    backend::orchard::primitives::redpallas::{Signature, SpendAuth, VerificationKey},
    proxy::PROXY_BATCH_EXPIRY_BLOCKS,
    storage::VotingDb,
    types::{Network, VotingError},
};

/// The newest signed attempt of a bundle's batch, as stored.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SignedBatchAttempt {
    /// D1 with expiry 0, the batch's identity across attempts.
    pub(crate) batch_digest: [u8; 32],
    /// The chain id D1 binds.
    pub(crate) chain_id: String,
    /// Present exactly when the batch carries the registration.
    pub(crate) registration_signature: Option<[u8; 64]>,
    /// The first height at which the chain rejects this attempt.
    pub(crate) expiry_height: u64,
    /// One D1 signature per DC, in slot order.
    pub(crate) dc_signatures: Vec<[u8; 64]>,
}

impl std::fmt::Debug for SignedBatchAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedBatchAttempt")
            .field("batch_digest", &hex::encode(self.batch_digest))
            .field("chain_id", &self.chain_id)
            .field("expiry_height", &self.expiry_height)
            .field("dc_count", &self.dc_signatures.len())
            .finish_non_exhaustive()
    }
}

/// What signing an attempt of one bundle's batch needs.
pub(crate) struct AttemptSigningRequest<'a> {
    /// Canonical lowercase hex.
    pub(crate) round_id: &'a str,
    pub(crate) bundle_index: u32,
    /// The round hotkey's stored secret, whose randomized keys sign D1.
    pub(crate) hotkey_seed: &'a [u8],
    pub(crate) network: Network,
    /// The chain id D1 binds, from the chain client's configuration.
    pub(crate) chain_id: &'a str,
    /// Required for the first attempt of a batch carrying the registration,
    /// and ignored after it.
    pub(crate) registration_signature: Option<[u8; 64]>,
    /// The vote chain's latest committed height.
    pub(crate) chain_height: u64,
    /// The chain answered that the stored attempt expired (code 52).
    pub(crate) stored_attempt_expired: bool,
}

/// The attempt a signing call left live.
#[derive(Clone, Debug)]
pub(crate) struct AttemptSigning {
    /// The attempt the next POST carries.
    pub(crate) attempt: SignedBatchAttempt,
    /// This call signed it rather than keeping the stored one.
    pub(crate) signed_now: bool,
}

/// Leaves the bundle's proved batch with a live signed attempt, signing one
/// when none is stored or the stored one expired. Runs in one immediate
/// transaction.
///
/// Refuses with [`VotingError::InvalidInput`] a bundle without a proved
/// batch and one with a lifecycle row (its attempt may be on the wire). A
/// stored attempt is kept while `chain_height` is below its expiry and the
/// chain has not reported it expired. Otherwise the new expiry is
/// `max(chain_height, stored expiry) + PROXY_BATCH_EXPIRY_BLOCKS`, or
/// `chain_height + PROXY_BATCH_EXPIRY_BLOCKS` for the first attempt. Every
/// signature is verified under its DC's `r_vpk` before writing. A re-sign
/// must keep the stored batch digest and chain id.
pub(crate) fn sign_batch_attempt(
    db: &VotingDb,
    request: AttemptSigningRequest<'_>,
) -> Result<AttemptSigning, VotingError> {
    let wallet_id = db.wallet_id();
    let round_bytes = decode_round_id(request.round_id)?;
    let bundle_index = request.bundle_index;
    db.write_transaction("sign proxy batch attempt", |tx| {
        let batch = batch_store::load_proved_batch(tx, request.round_id, &wallet_id, bundle_index)?
            .ok_or_else(|| VotingError::InvalidInput {
                message: format!("bundle {bundle_index} has no proved proxy batch to sign"),
            })?;
        if batch_store::lifecycle_state(tx, request.round_id, &wallet_id, bundle_index)?.is_some() {
            return Err(VotingError::InvalidInput {
                message: format!(
                    "bundle {bundle_index}'s proxy batch has a submission that may be on the \
                     wire; it is not signed again"
                ),
            });
        }
        let stored = load_signed_attempt(tx, request.round_id, &wallet_id, bundle_index)?;
        if let Some(stored) = &stored {
            if stored.chain_id != request.chain_id {
                return Err(VotingError::InvalidInput {
                    message: format!(
                        "bundle {bundle_index}'s proxy batch was signed for chain {}, not {}",
                        stored.chain_id, request.chain_id
                    ),
                });
            }
            if !request.stored_attempt_expired && request.chain_height < stored.expiry_height {
                return Ok(AttemptSigning {
                    attempt: stored.clone(),
                    signed_now: false,
                });
            }
        }

        let expiry_height = stored
            .as_ref()
            .map_or(request.chain_height, |stored| {
                stored.expiry_height.max(request.chain_height)
            })
            .checked_add(PROXY_BATCH_EXPIRY_BLOCKS)
            .ok_or_else(|| VotingError::InvalidInput {
                message: "the proxy batch expiry height overflows".to_string(),
            })?;
        let registration_van =
            registration_van(tx, request.round_id, &wallet_id, bundle_index, &batch)?;
        let registration_signature = match (
            registration_van,
            stored
                .as_ref()
                .and_then(|stored| stored.registration_signature),
        ) {
            (None, _) => None,
            (Some(_), Some(stored)) => Some(stored),
            (Some(_), None) => {
                let signature =
                    request
                        .registration_signature
                        .ok_or_else(|| VotingError::InvalidInput {
                            message: format!(
                                "bundle {bundle_index}'s proxy batch carries the registration, so \
                             its first attempt needs the registration's SpendAuth signature"
                            ),
                        })?;
                // Verifies the signature under the stored randomized key and
                // PCZT sighash.
                crate::delegate::submission_with_conn(
                    tx,
                    &wallet_id,
                    request.round_id,
                    bundle_index,
                    signature,
                )?;
                Some(signature)
            }
        };
        let fields = batch_fields(round_bytes, &batch, registration_van, expiry_height);
        let batch_digest = proxy_batch_digest(request.chain_id, &fields).map_err(digest_error)?;
        if let Some(stored) = &stored {
            if stored.batch_digest != batch_digest {
                return Err(VotingError::Internal {
                    message: format!(
                        "bundle {bundle_index}'s stored proxy batch digest does not match its \
                         proved batch"
                    ),
                });
            }
        }
        let d1 = proxy_batch_sighash(request.chain_id, &fields).map_err(digest_error)?;
        let slots = load_planned_slots(tx, request.round_id, &wallet_id, bundle_index)?;
        let mut dc_signatures = Vec::with_capacity(batch.dcs.len());
        for dc in &batch.dcs {
            let slot = slots
                .iter()
                .find(|slot| slot.dc_slot == dc.dc_slot)
                .ok_or_else(|| VotingError::Internal {
                    message: format!("proved DC in slot {} has no planned slot", dc.dc_slot),
                })?;
            let signature = crate::vote_commitment::sign_cast_vote_digest(
                request.hotkey_seed,
                request.network,
                &d1,
                &slot.alpha_v,
            )?
            .vote_auth_sig;
            let signature: [u8; 64] =
                signature
                    .as_slice()
                    .try_into()
                    .map_err(|_| VotingError::Internal {
                        message: "a D1 signature is not 64 bytes".to_string(),
                    })?;
            verify_dc_signature(&dc.r_vpk, &d1, &signature).map_err(|_| {
                VotingError::InvalidInput {
                    message: format!(
                        "the round hotkey does not sign for the DC in slot {}",
                        dc.dc_slot
                    ),
                }
            })?;
            dc_signatures.push(signature);
        }
        let attempt = SignedBatchAttempt {
            batch_digest,
            chain_id: request.chain_id.to_string(),
            registration_signature,
            expiry_height,
            dc_signatures,
        };
        write_attempt(
            tx,
            request.round_id,
            &wallet_id,
            bundle_index,
            &attempt,
            stored.is_some(),
        )?;
        Ok(AttemptSigning {
            attempt,
            signed_now: true,
        })
    })
}

/// What submitting one bundle's proved batch reads first, from one view.
#[derive(Clone, Debug)]
pub(crate) struct SubmissionState {
    pub(crate) carries_registration: bool,
    /// The newest signed attempt, if stored.
    pub(crate) attempt: Option<SignedBatchAttempt>,
    /// The `state` of the batch's lifecycle row, if any.
    pub(crate) lifecycle_state: Option<String>,
}

/// Reads `bundle_index`'s submission state in one read transaction, refusing
/// a bundle without a proved batch.
pub(crate) fn load_submission_state(
    db: &VotingDb,
    round_id: &str,
    bundle_index: u32,
) -> Result<SubmissionState, VotingError> {
    let wallet_id = db.wallet_id();
    db.read_transaction("load proxy batch submission state", |tx| {
        let batch = batch_store::load_proved_batch(tx, round_id, &wallet_id, bundle_index)?
            .ok_or_else(|| VotingError::InvalidInput {
                message: format!("bundle {bundle_index} has no proved proxy batch"),
            })?;
        Ok(SubmissionState {
            carries_registration: batch.anchor_height == 0,
            attempt: load_signed_attempt(tx, round_id, &wallet_id, bundle_index)?,
            lifecycle_state: batch_store::lifecycle_state(tx, round_id, &wallet_id, bundle_index)?,
        })
    })
}

/// The bundle's stored attempt, if one is signed.
pub(crate) fn load_signed_attempt(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<Option<SignedBatchAttempt>, VotingError> {
    type Row = (Vec<u8>, String, Option<Vec<u8>>, i64, Vec<u8>);
    let row: Option<Row> = conn
        .query_row(
            "SELECT batch_digest, chain_id, registration_spend_auth_sig, expiry_height,
                    dc_signatures
               FROM proxy_batch_attempts
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
            },
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| VotingError::from_sqlite("load proxy batch attempt", &error))?;
    let Some((digest, chain_id, registration, expiry, signatures)) = row else {
        return Ok(None);
    };
    let corrupt = |what: &str| VotingError::Internal {
        message: format!("bundle {bundle_index}'s stored proxy batch attempt has {what}"),
    };
    let (dc_signatures, rest) = signatures.as_chunks::<64>();
    if !rest.is_empty() {
        return Err(corrupt("signatures that are not 64 bytes each"));
    }
    Ok(Some(SignedBatchAttempt {
        batch_digest: digest
            .try_into()
            .map_err(|_| corrupt("a batch digest that is not 32 bytes"))?,
        chain_id,
        registration_signature: registration
            .map(|signature| signature.try_into())
            .transpose()
            .map_err(|_| corrupt("a registration signature that is not 64 bytes"))?,
        expiry_height: u64::try_from(expiry).map_err(|_| corrupt("a negative expiry height"))?,
        dc_signatures: dc_signatures.to_vec(),
    }))
}

/// The D1 fields of `batch` with `expiry_height` and any `registration_van`.
pub(crate) fn batch_fields(
    round_id: [u8; 32],
    batch: &ProvedProxyBatch,
    registration_van: Option<[u8; 32]>,
    expiry_height: u64,
) -> ProxyDelegationBatchFields {
    ProxyDelegationBatchFields {
        vote_round_id: round_id,
        vote_comm_tree_anchor_height: u64::from(batch.anchor_height),
        expiry_height,
        registration_van_cmx: registration_van,
        dcs: batch
            .dcs
            .iter()
            .map(|dc| ProxyDelegationActionFields {
                r_vpk: dc.r_vpk,
                van_nullifier: dc.van_nullifier,
                vote_authority_note_new: dc.vote_authority_note_new,
                proxy_delegation_commitment: dc.delegation_commitment,
                dc_slot_nullifier: dc.dc_slot_nullifier,
            })
            .collect(),
    }
}

/// The registration VAN a batch carrying the registration binds in D1, or
/// `None` for a chain-anchored batch.
pub(crate) fn registration_van(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    batch: &ProvedProxyBatch,
) -> Result<Option<[u8; 32]>, VotingError> {
    if batch.anchor_height != 0 {
        return Ok(None);
    }
    let stored: Option<Vec<u8>> = conn
        .query_row(
            "SELECT gov_comm FROM bundles
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
            },
            |row| row.get(0),
        )
        .map_err(|error| VotingError::from_sqlite("load the registration VAN", &error))?;
    stored
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!("bundle {bundle_index} has no registration VAN"),
        })?
        .try_into()
        .map(Some)
        .map_err(|_| VotingError::Internal {
            message: format!(
                "bundle {bundle_index} stores a registration VAN that is not 32 bytes"
            ),
        })
}

/// Verifies one DC's D1 signature under its `r_vpk`.
pub(crate) fn verify_dc_signature(
    r_vpk: &[u8; 32],
    d1: &[u8; 32],
    signature: &[u8; 64],
) -> Result<(), VotingError> {
    let key =
        VerificationKey::<SpendAuth>::try_from(*r_vpk).map_err(|_| VotingError::Internal {
            message: "a DC's r_vpk is not a valid SpendAuth verification key".to_string(),
        })?;
    key.verify(d1, &Signature::<SpendAuth>::from(*signature))
        .map_err(|_| VotingError::InvalidInput {
            message: "a D1 signature does not verify under its DC's r_vpk".to_string(),
        })
}

/// Writes `attempt`, inserting the batch's first or replacing the stored one.
fn write_attempt(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    attempt: &SignedBatchAttempt,
    replaces_stored: bool,
) -> Result<(), VotingError> {
    let signatures: Vec<u8> = attempt.dc_signatures.iter().flatten().copied().collect();
    let params = named_params! {
        ":round_id": round_id,
        ":wallet_id": wallet_id,
        ":bundle_index": i64::from(bundle_index),
        ":batch_digest": attempt.batch_digest.as_slice(),
        ":chain_id": attempt.chain_id,
        ":registration": attempt.registration_signature.as_ref().map(|signature| signature.as_slice()),
        ":expiry_height": i64::try_from(attempt.expiry_height).map_err(|_| VotingError::InvalidInput {
            message: "the proxy batch expiry height does not fit storage".to_string(),
        })?,
        ":dc_signatures": signatures,
        ":signed_at": i64::try_from(crate::proxy::unix_now()).unwrap_or(i64::MAX),
    };
    let written = if replaces_stored {
        conn.execute(
            "UPDATE proxy_batch_attempts
                SET expiry_height = :expiry_height, dc_signatures = :dc_signatures,
                    signed_at = :signed_at
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index AND batch_digest = :batch_digest
                AND chain_id = :chain_id
                AND registration_spend_auth_sig IS :registration",
            params,
        )
    } else {
        conn.execute(
            "INSERT INTO proxy_batch_attempts
                 (round_id, wallet_id, bundle_index, batch_digest, chain_id,
                  registration_spend_auth_sig, expiry_height, dc_signatures, signed_at)
             VALUES (:round_id, :wallet_id, :bundle_index, :batch_digest, :chain_id,
                     :registration, :expiry_height, :dc_signatures, :signed_at)",
            params,
        )
    }
    .map_err(|error| VotingError::from_sqlite("persist proxy batch attempt", &error))?;
    if written != 1 {
        return Err(VotingError::Internal {
            message: format!("bundle {bundle_index}'s proxy batch attempt changed while signing"),
        });
    }
    Ok(())
}

/// Maps a D1 digest failure to an input error.
fn digest_error(error: zcash_vote_delegate::DigestError) -> VotingError {
    VotingError::InvalidInput {
        message: format!("cannot compute the proxy batch's D1: {error}"),
    }
}

//! Building a batch's DCs from their persisted slots, in slot order, without
//! proving. The chaining is described on [`BatchAnchor`] (contract §5.1).

use vote_commitment_tree::{MerkleHashVote, MerklePath};
use voting_circuits::proxy_delegation::{
    build_proxy_delegation_proof, ProxyDelegationBundle, ShareLayout,
};
use zcash_vote_delegate::{DcSeedInputs, DcShareLayout};

use super::{slot_plan::canonical_base, BatchAnchor, PlannedSlot};
use crate::{
    backend::pasta_curves::{
        group::{ff::PrimeField, GroupEncoding},
        pallas,
    },
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    proxy::ProxyRoundKey,
    types::{Network, VotingError},
    vote::{single_leaf_auth_path, VAN_AUTH_PATH_LEN},
};

/// Everything a bundle's DC chain is built from, captured in one read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DcChainInputs {
    pub(crate) round_id: [u8; 32],
    /// The network the hotkey derives under.
    pub(crate) network: Network,
    /// The hotkey address index the bundle's VANs commit to.
    pub(crate) address_index: u32,
    /// The bundle's VAN blinding, which every successor keeps.
    pub(crate) van_comm_rand: [u8; 32],
    pub(crate) ea_pk: [u8; 32],
    /// The registration VAN the first DC spends.
    pub(crate) registration_van: [u8; 32],
    /// The spent VAN's weight.
    pub(crate) van_ballots: u64,
    /// In slot order.
    pub(crate) slots: Vec<PlannedSlot>,
}

/// One DC built from its slot. Not `Debug` because the bundle carries the
/// DC's share secrets and randomizer.
pub(crate) struct BuiltDc {
    pub(crate) slot: PlannedSlot,
    /// The circuit, its instance and the derived share material.
    pub(crate) bundle: ProxyDelegationBundle,
}

/// Builds every DC of the batch, in slot order, against `anchor`.
///
/// `dc_seed` binds the spent VAN's nullifier, so the builder runs twice per
/// DC, once to learn that nullifier (no seed affects it) and once with the
/// seed. It checks that each DC spends the VAN its chain expects and
/// publishes its persisted slot nullifier.
pub(crate) fn build_dc_chain(
    inputs: &DcChainInputs,
    anchor: &BatchAnchor,
    hotkey_seed: &[u8],
) -> Result<Vec<BuiltDc>, VotingError> {
    let spending_key =
        spending_key_from_hotkey_seed(hotkey_seed, inputs.network, VOTING_HOTKEY_ACCOUNT_INDEX)?;
    let round_key = ProxyRoundKey::from_hotkey_secret(
        spending_key.to_bytes(),
        inputs.network,
        &inputs.round_id,
    );
    let round = canonical_base("vote round id", &inputs.round_id)?;
    let van_comm_rand = canonical_base("van_comm_rand", &inputs.van_comm_rand)?;
    let ea_pk = election_key(&inputs.ea_pk)?;
    let anchor_height = anchor.anchor_height();

    let mut spent_van = inputs.registration_van;
    let mut van_ballots = inputs.van_ballots;
    let mut built = Vec::with_capacity(inputs.slots.len());
    for (index, slot) in inputs.slots.iter().enumerate() {
        let (path, position) = match (index, anchor) {
            (0, BatchAnchor::Chain(witness)) => (witness.auth_path_fixed()?, witness.position),
            _ => (single_leaf_auth_path(spent_van)?, 0),
        };
        let path_fields = path_fields(&path)?;
        let alpha_v = Option::from(pallas::Scalar::from_repr(slot.alpha_v)).ok_or_else(|| {
            VotingError::Internal {
                message: format!("slot {} stores a non-canonical randomizer", slot.dc_slot),
            }
        })?;
        let build = |dc_seed: [u8; 32]| {
            build_proxy_delegation_proof(
                &spending_key,
                inputs.address_index,
                van_ballots,
                van_comm_rand,
                round,
                path_fields,
                position,
                anchor_height,
                ea_pk,
                alpha_v,
                slot.delegate_index,
                slot.weight_ballots,
                slot.dc_slot,
                dc_seed,
                share_layout(slot.layout),
            )
            .map_err(|error| VotingError::InvalidInput {
                message: format!("cannot build the DC in slot {}: {error}", slot.dc_slot),
            })
        };
        let van_nullifier = build([0; 32])?.instance.van_nullifier.to_repr();
        let dc_seed = round_key.dc_seed(&DcSeedInputs {
            van_nullifier,
            delegate_index: slot.delegate_index,
            weight_ballots: slot.weight_ballots,
            layout: slot.layout,
        });
        let bundle = build(*dc_seed.expose_secret())?;

        let spent = canonical_base("spent VAN", &spent_van)?;
        if !merkle_path(position, &path)?.verify(spent, bundle.instance.vote_comm_tree_root) {
            return Err(VotingError::Internal {
                message: format!(
                    "the DC in slot {} does not spend the VAN its batch chains to",
                    slot.dc_slot
                ),
            });
        }
        if bundle.instance.dc_slot_nullifier.to_repr() != slot.dc_slot_nullifier {
            return Err(VotingError::Internal {
                message: format!(
                    "the DC in slot {} does not publish its planned slot nullifier",
                    slot.dc_slot
                ),
            });
        }
        spent_van = bundle.instance.vote_authority_note_new.to_repr();
        van_ballots = van_ballots
            .checked_sub(slot.weight_ballots)
            .expect("the builder refuses a DC heavier than its VAN");
        built.push(BuiltDc {
            slot: slot.clone(),
            bundle,
        });
    }
    Ok(built)
}

/// The circuit's share layout for a committed batch layout.
pub(crate) fn share_layout(layout: DcShareLayout) -> ShareLayout {
    match layout {
        DcShareLayout::Standard => ShareLayout::Standard,
        DcShareLayout::SingleShare => ShareLayout::SingleShare,
    }
}

/// The election authority key, refusing a non-point or the identity.
pub(crate) fn election_key(bytes: &[u8; 32]) -> Result<pallas::Affine, VotingError> {
    use crate::backend::pasta_curves::group::{Curve, Group};
    let point: pallas::Point = Option::from(pallas::Point::from_bytes(bytes)).ok_or_else(|| {
        VotingError::InvalidInput {
            message: "ea_pk is not a valid compressed Pallas point".to_string(),
        }
    })?;
    if bool::from(point.is_identity()) {
        return Err(VotingError::InvalidInput {
            message: "ea_pk must not be the identity point".to_string(),
        });
    }
    Ok(point.to_affine())
}

/// An authentication path as the field elements the circuit takes.
fn path_fields(
    path: &[[u8; 32]; VAN_AUTH_PATH_LEN],
) -> Result<[pallas::Base; VAN_AUTH_PATH_LEN], VotingError> {
    let mut fields = [pallas::Base::zero(); VAN_AUTH_PATH_LEN];
    for (field, sibling) in fields.iter_mut().zip(path) {
        *field = canonical_base("VAN authentication path sibling", sibling)?;
    }
    Ok(fields)
}

/// The vote-tree path that `position` and `path` describe.
fn merkle_path(
    position: u32,
    path: &[[u8; 32]; VAN_AUTH_PATH_LEN],
) -> Result<MerklePath, VotingError> {
    let mut siblings = [MerkleHashVote::from_fp(pallas::Base::zero()); VAN_AUTH_PATH_LEN];
    for (sibling, bytes) in siblings.iter_mut().zip(path) {
        *sibling = MerkleHashVote::from_fp(canonical_base("VAN path sibling", bytes)?);
    }
    Ok(MerklePath::from_parts(position, siblings))
}

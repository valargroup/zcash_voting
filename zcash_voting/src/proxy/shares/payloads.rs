//! A landed DC's helper payloads, derived again from the round hotkey for
//! every delivery (contract §10, §11.1).
//!
//! The DC's seed binds the spent VAN's nullifier and the DC's delegate,
//! weight and layout, all durable, so one hotkey always derives the same
//! shares. The circuit's builder is the one share derivation. Its DC must
//! equal the landed one, which checks every share secret, since the DC hash
//! binds all 16 ciphertexts and blinds. The spent VAN's other inputs reach
//! neither the shares nor the DC, so the builder gets a VAN exactly as heavy
//! as the DC and an empty tree path.

use rusqlite::{named_params, Connection, OptionalExtension};
use voting_circuits::proxy_delegation::build_proxy_delegation_proof;
use zcash_vote_delegate::{DcSeedInputs, DcShareLayout};

use super::delivered_share_indexes;
use crate::{
    backend::pasta_curves::{group::ff::PrimeField, pallas},
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    proxy::{
        batch::{canonical_base, decode_round_id, election_key, share_layout},
        ProxyRoundKey, POOL_PROPOSAL_ID,
    },
    share::compute_nullifier,
    storage::queries,
    types::{Network, SharePayload, VotingError, WireEncryptedShare},
    vote::VAN_AUTH_PATH_LEN,
};

/// Everything a landed DC's payloads derive from, read in one transaction.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LandedDc {
    /// The round's 32-byte id.
    pub(crate) round_id: [u8; 32],
    /// The round's stored network, which the hotkey derives under.
    pub(crate) network: Network,
    /// The round's election authority key.
    pub(crate) ea_pk: [u8; 32],
    /// The hotkey address index the bundle's VANs commit to.
    pub(crate) address_index: u32,
    /// The bundle's VAN blinding.
    pub(crate) van_comm_rand: [u8; 32],
    pub(crate) bundle_index: u32,
    pub(crate) dc_slot: u8,
    /// The delegate the DC is for.
    pub(crate) delegate_index: u32,
    /// The DC's weight in ballots.
    pub(crate) weight_ballots: u64,
    /// The share layout the DC's slot committed.
    pub(crate) layout: DcShareLayout,
    /// The DC's spend-auth randomizer.
    pub(crate) alpha_v: [u8; 32],
    /// Nullifier of the VAN the DC spent, which its seed binds.
    pub(crate) van_nullifier: [u8; 32],
    /// The landed DC.
    pub(crate) delegation_commitment: [u8; 32],
    /// The DC's leaf in the round's commitment tree.
    pub(crate) tree_position: u64,
}

impl std::fmt::Debug for LandedDc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LandedDc")
            .field("bundle_index", &self.bundle_index)
            .field("dc_slot", &self.dc_slot)
            .field("layout", &self.layout)
            .field("tree_position", &self.tree_position)
            .finish_non_exhaustive()
    }
}

/// A landed DC's helper payloads, one per share its layout delivers, in
/// share order, with each share's reveal nullifier.
pub(crate) struct DcSharePayloads {
    /// The payloads, each carrying `proposal_id = 0` and `vote_decision = d`.
    pub(crate) payloads: Vec<SharePayload>,
    /// Each payload's reveal nullifier, in the same order.
    pub(crate) nullifiers: Vec<[u8; 32]>,
}

/// Reads the landed DC in `bundle_index`'s slot `dc_slot` on `conn`, or
/// fails with [`VotingError::InvalidInput`] when its batch has not landed.
pub(crate) fn load_landed_dc(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
) -> Result<LandedDc, VotingError> {
    type Row = (
        Vec<u8>,
        Option<i64>,
        Option<Vec<u8>>,
        i64,
        i64,
        i64,
        Option<Vec<u8>>,
        Vec<u8>,
        Vec<u8>,
        i64,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT r.ea_pk, b.address_index, b.van_comm_rand,
                    slot.delegate_index, slot.weight_ballots, slot.layout, slot.alpha_v,
                    dc.van_nullifier, dc.delegation_commitment, landing.tree_position
               FROM proxy_dc_landings landing
               JOIN proxy_batch_dcs dc
                 ON dc.round_id = landing.round_id AND dc.wallet_id = landing.wallet_id
                AND dc.bundle_index = landing.bundle_index AND dc.dc_slot = landing.dc_slot
               JOIN proxy_slots slot
                 ON slot.round_id = landing.round_id AND slot.wallet_id = landing.wallet_id
                AND slot.bundle_index = landing.bundle_index AND slot.dc_slot = landing.dc_slot
               JOIN bundles b
                 ON b.round_id = landing.round_id AND b.wallet_id = landing.wallet_id
                AND b.bundle_index = landing.bundle_index
               JOIN rounds r ON r.round_id = landing.round_id AND r.wallet_id = landing.wallet_id
              WHERE landing.round_id = :round_id AND landing.wallet_id = :wallet_id
                AND landing.bundle_index = :bundle_index AND landing.dc_slot = :dc_slot",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
                ":dc_slot": i64::from(dc_slot),
            },
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(|error| VotingError::from_sqlite("load landed proxy DC", &error))?;
    let Some((
        ea_pk,
        address_index,
        van_comm_rand,
        delegate_index,
        weight_ballots,
        layout,
        alpha_v,
        van_nullifier,
        delegation_commitment,
        tree_position,
    )) = row
    else {
        return Err(VotingError::InvalidInput {
            message: format!(
                "the DC in slot {dc_slot} of bundle {bundle_index} has not landed; its shares \
                 are delivered only after its batch lands"
            ),
        });
    };
    let corrupt = |what: &str| VotingError::Internal {
        message: format!("landed DC in slot {dc_slot} of bundle {bundle_index} stores {what}"),
    };
    let bytes32 = |name: &str, bytes: Vec<u8>| {
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| corrupt(&format!("a {name} that is not 32 bytes")))
    };
    Ok(LandedDc {
        round_id: decode_round_id(round_id)?,
        network: queries::load_round_network(conn, round_id, wallet_id)?,
        ea_pk: bytes32("ea_pk", ea_pk)?,
        address_index: address_index
            .and_then(|index| u32::try_from(index).ok())
            .ok_or_else(|| corrupt("no valid address index"))?,
        van_comm_rand: bytes32(
            "van_comm_rand",
            van_comm_rand.ok_or_else(|| corrupt("no van_comm_rand"))?,
        )?,
        bundle_index,
        dc_slot,
        delegate_index: u32::try_from(delegate_index)
            .map_err(|_| corrupt("an invalid delegate index"))?,
        weight_ballots: u64::try_from(weight_ballots).map_err(|_| corrupt("a negative weight"))?,
        layout: crate::proxy::layout_from_column(layout)
            .ok_or_else(|| corrupt("an unknown layout"))?,
        alpha_v: bytes32(
            "spend-auth randomizer",
            alpha_v.ok_or_else(|| corrupt("no spend-auth randomizer"))?,
        )?,
        van_nullifier: bytes32("VAN nullifier", van_nullifier)?,
        delegation_commitment: bytes32("delegation commitment", delegation_commitment)?,
        tree_position: u64::try_from(tree_position)
            .map_err(|_| corrupt("a negative tree position"))?,
    })
}

/// Derives `dc`'s helper payloads from `hotkey_seed`, the round hotkey's
/// stored secret, or fails with [`VotingError::InvalidInput`] when that
/// hotkey does not derive the landed DC.
pub(crate) fn derive_dc_share_payloads(
    dc: &LandedDc,
    hotkey_seed: &[u8],
) -> Result<DcSharePayloads, VotingError> {
    let spending_key =
        spending_key_from_hotkey_seed(hotkey_seed, dc.network, VOTING_HOTKEY_ACCOUNT_INDEX)?;
    let dc_seed =
        ProxyRoundKey::from_hotkey_secret(spending_key.to_bytes(), dc.network, &dc.round_id)
            .dc_seed(&DcSeedInputs {
                van_nullifier: dc.van_nullifier,
                delegate_index: dc.delegate_index,
                weight_ballots: dc.weight_ballots,
                layout: dc.layout,
            });
    let alpha_v = Option::from(pallas::Scalar::from_repr(dc.alpha_v)).ok_or_else(|| {
        VotingError::Internal {
            message: format!("slot {} stores a non-canonical randomizer", dc.dc_slot),
        }
    })?;
    let built = build_proxy_delegation_proof(
        &spending_key,
        dc.address_index,
        dc.weight_ballots,
        canonical_base("van_comm_rand", &dc.van_comm_rand)?,
        canonical_base("vote round id", &dc.round_id)?,
        [pallas::Base::zero(); VAN_AUTH_PATH_LEN],
        0,
        0,
        election_key(&dc.ea_pk)?,
        alpha_v,
        dc.delegate_index,
        dc.weight_ballots,
        dc.dc_slot,
        *dc_seed.expose_secret(),
        share_layout(dc.layout),
    )
    .map_err(|error| VotingError::InvalidInput {
        message: format!(
            "cannot derive the shares of the DC in slot {}: {error}",
            dc.dc_slot
        ),
    })?;
    if built.instance.delegation_commitment.to_repr() != dc.delegation_commitment {
        return Err(VotingError::InvalidInput {
            message: format!(
                "the round hotkey does not derive the landed DC in slot {} of bundle {}",
                dc.dc_slot, dc.bundle_index
            ),
        });
    }

    let all_enc_shares: Vec<WireEncryptedShare> = built
        .encrypted_shares
        .iter()
        .map(|share| WireEncryptedShare {
            c1: share.c1.to_vec(),
            c2: share.c2.to_vec(),
            share_index: share.share_index,
        })
        .collect();
    let share_comms: Vec<Vec<u8>> = built
        .share_comms
        .iter()
        .map(|comm| comm.to_repr().to_vec())
        .collect();
    let mut payloads = Vec::new();
    let mut nullifiers = Vec::new();
    for share_index in delivered_share_indexes(dc.layout) {
        let position = share_index as usize;
        let primary_blind = built.share_blinds[position].to_repr();
        nullifiers.push(compute_nullifier(
            &dc.delegation_commitment,
            share_index,
            &primary_blind,
        )?);
        payloads.push(SharePayload {
            vote_round_id: hex::encode(dc.round_id),
            shares_hash: built.shares_hash.to_repr().to_vec(),
            proposal_id: POOL_PROPOSAL_ID,
            vote_decision: dc.delegate_index,
            enc_share: all_enc_shares[position].clone(),
            tree_position: dc.tree_position,
            all_enc_shares: all_enc_shares.clone(),
            share_comms: share_comms.clone(),
            primary_blind: primary_blind.to_vec(),
        });
    }
    Ok(DcSharePayloads {
        payloads,
        nullifiers,
    })
}

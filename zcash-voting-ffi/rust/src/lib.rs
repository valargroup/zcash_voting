uniffi::setup_scaffolding!();

use librustvoting as voting;

// --- Error type ---

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum VotingError {
    #[error("Invalid input: {message}")]
    InvalidInput { message: String },
    #[error("Proof generation failed: {message}")]
    ProofFailed { message: String },
    #[error("Internal error: {message}")]
    Internal { message: String },
}

impl From<voting::VotingError> for VotingError {
    fn from(e: voting::VotingError) -> Self {
        match e {
            voting::VotingError::InvalidInput { message } => VotingError::InvalidInput { message },
            voting::VotingError::ProofFailed { message } => VotingError::ProofFailed { message },
            voting::VotingError::Internal { message } => VotingError::Internal { message },
        }
    }
}

// --- UniFFI Record types ---
// These mirror librustvoting types but with UniFFI derive macros.

#[derive(Clone, uniffi::Record)]
pub struct VotingHotkey {
    pub secret_key: Vec<u8>,
    pub public_key: Vec<u8>,
    pub address: String,
}

#[derive(Clone, uniffi::Record)]
pub struct NoteInfo {
    pub commitment: Vec<u8>,
    pub nullifier: Vec<u8>,
    pub value: u64,
    pub position: u64,
}

#[derive(Clone, uniffi::Record)]
pub struct VotingRoundParams {
    pub vote_round_id: String,
    pub snapshot_height: u64,
    pub ea_pk: Vec<u8>,
    pub nc_root: Vec<u8>,
    pub nullifier_imt_root: Vec<u8>,
}

#[derive(Clone, uniffi::Record)]
pub struct DelegationAction {
    pub action_bytes: Vec<u8>,
    pub rk: Vec<u8>,
    pub sighash: Vec<u8>,
}

#[derive(Clone, uniffi::Record)]
pub struct EncryptedShare {
    pub c1: Vec<u8>,
    pub c2: Vec<u8>,
    pub share_index: u32,
    pub plaintext_value: u64,
}

#[derive(Clone, uniffi::Record)]
pub struct VoteCommitmentBundle {
    pub van_nullifier: Vec<u8>,
    pub vote_authority_note_new: Vec<u8>,
    pub vote_commitment: Vec<u8>,
    pub proposal_id: String,
    pub proof: Vec<u8>,
}

#[derive(Clone, uniffi::Record)]
pub struct SharePayload {
    pub shares_hash: Vec<u8>,
    pub proposal_id: String,
    pub vote_decision: u32,
    pub enc_share: EncryptedShare,
    pub tree_position: u64,
}

#[derive(Clone, uniffi::Record)]
pub struct ProofResult {
    pub proof: Vec<u8>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Clone, uniffi::Record)]
pub struct WitnessData {
    pub note_commitment: Vec<u8>,
    pub position: u64,
    pub root: Vec<u8>,
    pub auth_path: Vec<Vec<u8>>,
}

// --- Conversion helpers: FFI types <-> librustvoting types ---

impl From<voting::VotingHotkey> for VotingHotkey {
    fn from(h: voting::VotingHotkey) -> Self {
        Self {
            secret_key: h.secret_key,
            public_key: h.public_key,
            address: h.address,
        }
    }
}

impl From<VotingHotkey> for voting::VotingHotkey {
    fn from(h: VotingHotkey) -> Self {
        Self {
            secret_key: h.secret_key,
            public_key: h.public_key,
            address: h.address,
        }
    }
}

impl From<NoteInfo> for voting::NoteInfo {
    fn from(n: NoteInfo) -> Self {
        Self {
            commitment: n.commitment,
            nullifier: n.nullifier,
            value: n.value,
            position: n.position,
        }
    }
}

impl From<VotingRoundParams> for voting::VotingRoundParams {
    fn from(p: VotingRoundParams) -> Self {
        Self {
            vote_round_id: p.vote_round_id,
            snapshot_height: p.snapshot_height,
            ea_pk: p.ea_pk,
            nc_root: p.nc_root,
            nullifier_imt_root: p.nullifier_imt_root,
        }
    }
}

impl From<voting::DelegationAction> for DelegationAction {
    fn from(a: voting::DelegationAction) -> Self {
        Self {
            action_bytes: a.action_bytes,
            rk: a.rk,
            sighash: a.sighash,
        }
    }
}

impl From<DelegationAction> for voting::DelegationAction {
    fn from(a: DelegationAction) -> Self {
        Self {
            action_bytes: a.action_bytes,
            rk: a.rk,
            sighash: a.sighash,
        }
    }
}

impl From<voting::EncryptedShare> for EncryptedShare {
    fn from(s: voting::EncryptedShare) -> Self {
        Self {
            c1: s.c1,
            c2: s.c2,
            share_index: s.share_index,
            plaintext_value: s.plaintext_value,
        }
    }
}

impl From<EncryptedShare> for voting::EncryptedShare {
    fn from(s: EncryptedShare) -> Self {
        Self {
            c1: s.c1,
            c2: s.c2,
            share_index: s.share_index,
            plaintext_value: s.plaintext_value,
        }
    }
}

impl From<voting::VoteCommitmentBundle> for VoteCommitmentBundle {
    fn from(b: voting::VoteCommitmentBundle) -> Self {
        Self {
            van_nullifier: b.van_nullifier,
            vote_authority_note_new: b.vote_authority_note_new,
            vote_commitment: b.vote_commitment,
            proposal_id: b.proposal_id,
            proof: b.proof,
        }
    }
}

impl From<VoteCommitmentBundle> for voting::VoteCommitmentBundle {
    fn from(b: VoteCommitmentBundle) -> Self {
        Self {
            van_nullifier: b.van_nullifier,
            vote_authority_note_new: b.vote_authority_note_new,
            vote_commitment: b.vote_commitment,
            proposal_id: b.proposal_id,
            proof: b.proof,
        }
    }
}

impl From<voting::SharePayload> for SharePayload {
    fn from(p: voting::SharePayload) -> Self {
        Self {
            shares_hash: p.shares_hash,
            proposal_id: p.proposal_id,
            vote_decision: p.vote_decision,
            enc_share: p.enc_share.into(),
            tree_position: p.tree_position,
        }
    }
}

impl From<voting::ProofResult> for ProofResult {
    fn from(r: voting::ProofResult) -> Self {
        Self {
            proof: r.proof,
            success: r.success,
            error: r.error,
        }
    }
}

impl From<voting::WitnessData> for WitnessData {
    fn from(w: voting::WitnessData) -> Self {
        Self {
            note_commitment: w.note_commitment,
            position: w.position,
            root: w.root,
            auth_path: w.auth_path,
        }
    }
}

// --- Exported functions ---

#[uniffi::export]
pub fn generate_hotkey() -> Result<VotingHotkey, VotingError> {
    Ok(voting::hotkey::generate_hotkey()?.into())
}

#[uniffi::export]
pub fn decompose_weight(weight: u64) -> Vec<u64> {
    voting::decompose::decompose_weight(weight)
}

#[uniffi::export]
pub fn encrypt_shares(shares: Vec<u64>, ea_pk: Vec<u8>) -> Result<Vec<EncryptedShare>, VotingError> {
    Ok(voting::elgamal::encrypt_shares(&shares, &ea_pk)?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[uniffi::export]
pub fn construct_delegation_action(
    hotkey: VotingHotkey,
    notes: Vec<NoteInfo>,
    params: VotingRoundParams,
) -> Result<DelegationAction, VotingError> {
    let core_notes: Vec<voting::NoteInfo> = notes.into_iter().map(Into::into).collect();
    Ok(voting::action::construct_delegation_action(&hotkey.into(), &core_notes, &params.into())?.into())
}

#[uniffi::export]
pub fn generate_note_witness(
    note_position: u64,
    snapshot_height: u32,
    tree_state_bytes: Vec<u8>,
) -> Result<WitnessData, VotingError> {
    Ok(voting::witness::generate_note_witness(note_position, snapshot_height, &tree_state_bytes)?.into())
}

#[uniffi::export]
pub fn build_delegation_witness(
    action: DelegationAction,
    inclusion_proofs: Vec<Vec<u8>>,
    exclusion_proofs: Vec<Vec<u8>>,
) -> Result<Vec<u8>, VotingError> {
    Ok(voting::zkp1::build_delegation_witness(&action.into(), &inclusion_proofs, &exclusion_proofs)?)
}

#[uniffi::export]
pub fn generate_delegation_proof(witness: Vec<u8>) -> Result<ProofResult, VotingError> {
    Ok(voting::zkp1::generate_delegation_proof(&witness)?.into())
}

#[uniffi::export]
pub fn build_vote_commitment(
    proposal_id: String,
    choice: u32,
    enc_shares: Vec<EncryptedShare>,
    van_witness: Vec<u8>,
) -> Result<VoteCommitmentBundle, VotingError> {
    let core_shares: Vec<voting::EncryptedShare> = enc_shares.into_iter().map(Into::into).collect();
    Ok(voting::zkp2::build_vote_commitment(&proposal_id, choice, &core_shares, &van_witness)?.into())
}

#[uniffi::export]
pub fn build_share_payloads(
    enc_shares: Vec<EncryptedShare>,
    commitment: VoteCommitmentBundle,
) -> Result<Vec<SharePayload>, VotingError> {
    let core_shares: Vec<voting::EncryptedShare> = enc_shares.into_iter().map(Into::into).collect();
    Ok(voting::vote_commitment::build_share_payloads(&core_shares, &commitment.into())?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[uniffi::export]
pub fn voting_ffi_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

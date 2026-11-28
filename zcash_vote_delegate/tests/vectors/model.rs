//! The JSON shape of `test-vectors/proxy_delegation_v1.json`.
//!
//! Bytes are lowercase hex, integers are JSON numbers, and an absent value is
//! `null`. Each case is `{name, inputs, expected}`: `expected` is a pure
//! function of `inputs`, computed by [`super::cases`]. An `error` field holds
//! the error's bare name ([`super::error_name`]), and `null` means success. A
//! `document` is the exact text a wallet fetches.

use serde::{Deserialize, Serialize};

/// One vector: its inputs and the outputs they must produce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case<I, E> {
    pub name: String,
    pub inputs: I,
    pub expected: E,
}

/// The whole file. Field order is the file's section order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorFile {
    pub format: String,
    pub contract: String,
    pub generator: String,
    pub d1: Vec<Case<D1Inputs, D1Expected>>,
    pub handle: Vec<Case<HandleInputs, CanonicalExpected>>,
    pub proof_id: Vec<Case<ProofIdInputs, CanonicalExpected>>,
    pub register: Vec<Case<RegisterInputs, SignedDigestExpected>>,
    pub attest: Vec<Case<AttestInputs, AttestExpected>>,
    pub retire: Vec<Case<RetireInputs, SignedDigestExpected>>,
    pub ballot: Vec<Case<BallotInputs, SignedDigestExpected>>,
    pub domains: DomainsSection,
    pub kv_keys: Vec<Case<KvKeyInputs, KvKeyExpected>>,
    pub entry: Vec<Case<EntryInputs, EntryExpected>>,
    pub ed25519_keys: Ed25519Section,
    pub delegate_key: DelegateKeySection,
    pub dc_secrets: Vec<Case<DcSecretsInputs, DcSecretsExpected>>,
    pub marker: Vec<Case<MarkerInputs, MarkerExpected>>,
    pub lookalike: LookalikeSection,
    pub config_domains: ConfigDomainsSection,
    pub wallet_list_payload: Vec<Case<WalletListPayloadInputs, WalletListPayloadExpected>>,
    pub wallet_list_document: Vec<Case<WalletListDocumentInputs, WalletListDocumentExpected>>,
    pub errors: Vec<ErrorRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct D1Inputs {
    pub chain_id: String,
    pub vote_round_id: String,
    pub vote_comm_tree_anchor_height: u64,
    pub expiry_height: u64,
    pub registration_van_cmx: Option<String>,
    pub dcs: Vec<DcActionFields>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcActionFields {
    pub r_vpk: String,
    pub van_nullifier: String,
    pub vote_authority_note_new: String,
    pub proxy_delegation_commitment: String,
    pub dc_slot_nullifier: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct D1Expected {
    pub d1: String,
    pub batch_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandleInputs {
    pub provider: u32,
    pub handle: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofIdInputs {
    pub provider: u32,
    pub proof_id: String,
}

/// Whether a handle or proof id is canonical for its provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalExpected {
    pub canonical: bool,
}

/// REGISTER inputs. `dk` is the public key of `dk_sk`, which signs the digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterInputs {
    pub chain_id: String,
    pub provider: u32,
    pub handle: String,
    pub proof_id: String,
    pub dk_sk: String,
}

/// A digest signed by the delegate key derived from the case's `dk_sk`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDigestExpected {
    pub dk: String,
    pub digest: String,
    pub dk_sig: String,
}

/// ATTEST inputs; the verifier key is derived from `verifier_secret_seed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestInputs {
    pub chain_id: String,
    pub dk: String,
    pub provider: u32,
    pub handle: String,
    pub proof_id: String,
    pub not_before: u64,
    pub expires_at: u64,
    pub verifier_secret_seed: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestExpected {
    pub digest: String,
    pub verifier_key: String,
    pub sig: String,
}

/// RETIRE inputs; the entry's key is the public key of `dk_sk`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetireInputs {
    pub chain_id: String,
    pub delegate_index: u32,
    pub dk_sk: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BallotInputs {
    pub chain_id: String,
    pub vote_round_id: String,
    pub delegate_index: u32,
    pub options: Vec<u32>,
    pub dk_sk: String,
}

/// The digest domains: the three existing `SVOTE_` sighash domains and the
/// five proxy delegation domains, which together must be prefix-free.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainsSection {
    pub existing: Vec<String>,
    pub proxy_delegation: Vec<String>,
    pub prefix_free: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvKeyInputs {
    /// `entry`, `used_key`, `handle`, `next_index`, `params`, `ballot` or
    /// `pool_share_count`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vote_round_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvKeyExpected {
    pub key: String,
}

/// The value bytes stored at an entry's key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryInputs {
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryExpected {
    pub entry: Option<EntryFields>,
    pub error: Option<String>,
}

/// A decoded `ProxyDelegateEntry`; `status` is 0 active, 1 suspended, 2 retired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryFields {
    pub index: u32,
    pub dk: String,
    pub provider: u32,
    pub handle: String,
    pub proof_id: String,
    pub status: u32,
    pub status_reason: u32,
    pub registered_height: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ed25519Section {
    pub key_rule: Vec<Case<KeyRuleInputs, KeyRuleExpected>>,
    pub zip215: Vec<Case<Zip215Inputs, Zip215Expected>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRuleInputs {
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyRuleExpected {
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zip215Inputs {
    pub public_key: String,
    pub message: String,
    pub signature: String,
}

/// `error` is the ZIP-215 verdict; `key_rule_error` is the key rule's verdict
/// on the same public key, which ZIP-215 alone does not apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zip215Expected {
    pub error: Option<String>,
    pub key_rule_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateKeySection {
    pub encoding: Vec<Case<KeyEncodingInputs, KeyEncodingExpected>>,
    pub decode: Vec<Case<KeyTextInputs, KeyDecodeExpected>>,
    pub import: Vec<Case<KeyTextInputs, KeyImportExpected>>,
}

/// `delegate_index` only feeds the display form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEncodingInputs {
    pub dk_sk: String,
    pub delegate_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEncodingExpected {
    pub dk: String,
    pub zvdk: String,
    pub zvdsk: String,
    pub fingerprint: String,
    pub display: String,
}

/// A `zvdk1…` or `zvdsk1…` string as typed, pasted or scanned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyTextInputs {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyDecodeExpected {
    pub dk: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyImportExpected {
    pub dk_sk: Option<String>,
    pub error: Option<String>,
}

/// One DC's secrets; `net_u8` is 0, 1 or 2 and `layout` is 0 or 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcSecretsInputs {
    pub hotkey_secret: String,
    pub net_u8: u8,
    pub round_id: String,
    pub van_nullifier: String,
    pub delegate_index: u32,
    pub weight_ballots: u64,
    pub layout: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcSecretsExpected {
    pub hrk: String,
    pub dc_seed: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerInputs {
    pub text: String,
}

/// For a valid marker, the parsed key and the canonical text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerExpected {
    pub dk: Option<String>,
    pub canonical_text: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookalikeSection {
    pub fold: Vec<Case<FoldInputs, FoldExpected>>,
    pub pairs: Vec<Case<PairInputs, PairExpected>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldInputs {
    pub handle: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldExpected {
    pub folded: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairInputs {
    pub a: String,
    pub b: String,
}

/// `distance` is the OSA distance between the folds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairExpected {
    pub fold_a: String,
    pub fold_b: String,
    pub distance: usize,
    pub lookalike: bool,
}

/// The signing domains of the config family, and whether they are
/// prefix-free.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDomainsSection {
    pub domains: Vec<String>,
    pub prefix_free: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletListPayloadInputs {
    pub wallet_id: String,
    pub chain_id: String,
    pub version: u64,
    pub issued_at: u64,
    pub reserved_names: Vec<String>,
    pub delegates: Vec<WalletListRowInputs>,
    pub signer_key_id: String,
    pub signer_secret_seed: String,
}

/// One wallet list row; `dk` is a `zvdk1…` string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletListRowInputs {
    pub index: u32,
    pub provider: u32,
    pub handle: String,
    pub dk: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletListPayloadExpected {
    pub payload: Option<String>,
    pub signer_key: String,
    pub signature: Option<String>,
    pub document: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletListDocumentInputs {
    pub document: String,
    pub wallet_id: String,
    pub chain_id: String,
    pub pinned_keys: Vec<PinnedKeyInputs>,
}

/// A key a wallet pins for its list, with the `key_id` its signatures carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedKeyInputs {
    pub key_id: String,
    pub public_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletListDocumentExpected {
    pub payload: Option<String>,
    pub signed_by: Option<String>,
    pub error: Option<String>,
}

/// One row of the chain's error table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorRow {
    pub name: String,
    pub code: u32,
    pub codespace: String,
}

//! Derivation of each delegation commitment's secrets from the round hotkey
//! (contract §11.1).
//!
//! ```text
//! hrk     = BLAKE2b-256(key = hotkey_secret, personal = "ZVoteProxyHRK_v1", net_u8 || round_id)
//! dc_seed = BLAKE2b-256(key = hrk, personal = "ZVoteProxySeed01",
//!                       van_nf(32) || d_le32 || w_le64 || layout_u8)
//! ```
//!
//! Nothing derives from the wallet seed, OVK or FVK, so a viewing-key holder
//! cannot reconstruct a delegation. `dc_seed` binds the VAN, delegate, weight
//! and layout, so two DCs never share El Gamal randomness and a re-prove with
//! the same inputs yields the same DC. The circuit expands it into the share
//! secrets (contract §10).

const HOTKEY_ROUND_KEY_PERSONALIZATION: &[u8; 16] = b"ZVoteProxyHRK_v1";
const DC_SEED_PERSONALIZATION: &[u8; 16] = b"ZVoteProxySeed01";

/// The Zcash network a round hotkey belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ZcashNetwork {
    /// Mainnet, `net_u8 = 0`.
    Mainnet = 0,
    /// Testnet, `net_u8 = 1`.
    Testnet = 1,
    /// Regtest, `net_u8 = 2`.
    Regtest = 2,
}

impl ZcashNetwork {
    /// The network byte, matching zcash_voting's `network_rank`.
    pub const fn net_u8(self) -> u8 {
        self as u8
    }
}

/// How a DC's weight is split into helper shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DcShareLayout {
    /// 16 shares on the standard schedule, `layout_u8 = 0`.
    Standard = 0,
    /// One share holding the whole weight (last-moment window), `layout_u8 = 1`.
    SingleShare = 1,
}

impl DcShareLayout {
    /// The layout byte bound into `dc_seed`.
    pub const fn layout_u8(self) -> u8 {
        self as u8
    }
}

/// The values one DC's seed binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DcSeedInputs {
    /// Nullifier of the VAN the DC spends.
    pub van_nullifier: [u8; 32],
    /// The delegate the DC is for.
    pub delegate_index: u32,
    /// The DC's weight in ballots.
    pub weight_ballots: u64,
    /// The batch's share layout.
    pub layout: DcShareLayout,
}

/// Computes `hrk` for one round and network. `hotkey_secret` is the round
/// hotkey's Orchard spending key (`hotkey::spending_key_from_hotkey_seed`).
pub fn hotkey_round_key(
    hotkey_secret: &[u8; 32],
    network: ZcashNetwork,
    round_id: &[u8; 32],
) -> [u8; 32] {
    let mut message = [0u8; 33];
    message[0] = network.net_u8();
    message[1..].copy_from_slice(round_id);
    keyed_blake2b_256(hotkey_secret, HOTKEY_ROUND_KEY_PERSONALIZATION, &message)
}

/// Computes `dc_seed` for one DC under `hrk` from [`hotkey_round_key`].
pub fn dc_seed(hrk: &[u8; 32], dc: &DcSeedInputs) -> [u8; 32] {
    let mut message = [0u8; 45];
    message[..32].copy_from_slice(&dc.van_nullifier);
    message[32..36].copy_from_slice(&dc.delegate_index.to_le_bytes());
    message[36..44].copy_from_slice(&dc.weight_ballots.to_le_bytes());
    message[44] = dc.layout.layout_u8();
    keyed_blake2b_256(hrk, DC_SEED_PERSONALIZATION, &message)
}

/// BLAKE2b-256 of `message` under a 32-byte `key` and a 16-byte personalization.
fn keyed_blake2b_256(key: &[u8; 32], personalization: &[u8; 16], message: &[u8]) -> [u8; 32] {
    let hash = blake2b_simd::Params::new()
        .hash_length(32)
        .key(key)
        .personal(personalization)
        .hash(message);
    let mut output = [0u8; 32];
    output.copy_from_slice(hash.as_bytes());
    output
}

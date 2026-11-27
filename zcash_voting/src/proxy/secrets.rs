//! DC secrets from the round hotkey (contract §11.1).
//!
//! ```text
//! hrk     = BLAKE2b-256(key = hotkey_secret, personal = "ZVoteProxyHRK_v1", net_u8 || round_id)
//! dc_seed = BLAKE2b-256(key = hrk, personal = "ZVoteProxySeed01",
//!                       van_nf || d_le32 || w_le64 || layout_u8)
//! ```
//!
//! `hotkey_secret` is the round hotkey's Orchard spending key, derived as the
//! vote flow derives it for ZKP2. Nothing comes from the wallet seed, OVK or
//! FVK, so a viewing-key holder cannot read a delegation and no seed material
//! crosses the wallet boundary. The formulas live in [`zcash_vote_delegate`].

use std::fmt;

use zcash_vote_delegate::{dc_seed, hotkey_round_key, DcSeedInputs, ZcashNetwork};
use zeroize::Zeroizing;

use crate::{
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    types::{Network, VotingError},
};

/// The round hotkey's proxy key `hrk` for one round and network, the root of
/// every DC secret in that round. `Debug` never prints the key.
pub struct ProxyRoundKey {
    hrk: Zeroizing<[u8; 32]>,
}

impl ProxyRoundKey {
    /// Derives `hrk` from the stored hotkey seed via ZIP-32 account
    /// [`VOTING_HOTKEY_ACCOUNT_INDEX`]. Fails with
    /// [`VotingError::InvalidInput`] when the seed is shorter than 32 bytes or
    /// yields no spending key for `network`.
    pub fn from_hotkey_seed(
        hotkey_seed: &[u8],
        network: Network,
        round_id: &[u8; 32],
    ) -> Result<Self, VotingError> {
        let spending_key =
            spending_key_from_hotkey_seed(hotkey_seed, network, VOTING_HOTKEY_ACCOUNT_INDEX)?;
        Ok(Self::from_hotkey_secret(
            spending_key.to_bytes(),
            network,
            round_id,
        ))
    }

    /// Derives `hrk` from `hotkey_secret`, the 32 bytes of the round
    /// hotkey's Orchard spending key.
    pub(crate) fn from_hotkey_secret(
        hotkey_secret: &[u8; 32],
        network: Network,
        round_id: &[u8; 32],
    ) -> Self {
        Self {
            hrk: Zeroizing::new(hotkey_round_key(
                hotkey_secret,
                zcash_network(network),
                round_id,
            )),
        }
    }

    /// Derives the seed of the DC that `dc` describes. The circuit builder
    /// expands it into every share secret of the DC (contract §10).
    pub fn dc_seed(&self, dc: &DcSeedInputs) -> DcSeed {
        DcSeed(Zeroizing::new(dc_seed(&self.hrk, dc)))
    }
}

impl fmt::Debug for ProxyRoundKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProxyRoundKey(..)")
    }
}

/// One DC's seed. It binds the VAN nullifier, delegate, weight and layout, so
/// two DCs never share El Gamal randomness. Not printed by `Debug`, zeroed on
/// drop.
pub struct DcSeed(Zeroizing<[u8; 32]>);

impl DcSeed {
    /// The seed bytes, for the circuit builder.
    pub fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for DcSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DcSeed(..)")
    }
}

/// The shared crate's network. Its `net_u8` matches the chain-submission
/// network rank (0 mainnet, 1 testnet, 2 regtest).
fn zcash_network(network: Network) -> ZcashNetwork {
    match network {
        Network::Mainnet => ZcashNetwork::Mainnet,
        Network::Testnet => ZcashNetwork::Testnet,
        Network::Regtest => ZcashNetwork::Regtest,
    }
}

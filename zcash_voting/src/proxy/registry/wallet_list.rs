//! The wallet list client (contract §12.3). It verifies a fetched list
//! under the keys the build pins for the chain and keeps the newest version.
//! The host stores its bytes per `wallet_id` and chain. The list never
//! expires, because every safety fact comes from the chain.

use zcash_vote_delegate::{verify_wallet_list, PinnedListKey, VerifiedWalletList, WalletListError};

use super::RegistryError;

/// What a wallet build pins for its list on one chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletListPin {
    /// The wallet the list must name.
    pub wallet_id: String,
    /// The chain the list must name.
    pub chain_id: String,
    /// The only keys whose signatures count, compiled into the build.
    pub keys: Vec<PinnedListKey>,
}

impl WalletListPin {
    /// Verifies `document` as this wallet's list on this chain, failing with
    /// the [`WalletListError`] of the first problem.
    pub fn verify(&self, document: &[u8]) -> Result<VerifiedWalletList, WalletListError> {
        verify_wallet_list(document, &self.wallet_id, &self.chain_id, &self.keys)
    }
}

/// What one fetched document did to the stored list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletListUpdate {
    /// No list was stored or the version is higher. The host stores its bytes.
    Adopted,
    /// The document repeats the stored list.
    Unchanged,
    /// A lower version, as a stale mirror serves. The stored list is kept.
    Older {
        /// The document's version.
        version: u64,
    },
    /// The stored version with another payload, which an honest wallet team
    /// never signs. The stored list is kept and the host reports it.
    ConflictingPayload,
    /// The document failed verification. The stored list is kept.
    Rejected(WalletListError),
}

/// The list to show after reading one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletListResolution {
    /// The document's list if adopted, else the stored one.
    pub newest: Option<VerifiedWalletList>,
    /// What the document did.
    pub update: WalletListUpdate,
}

/// Verifies `document` under `pin` and keeps the newer of it and `stored`
/// (verified again under `pin`). Only a verified list of a higher version
/// replaces the stored one. Fails with [`RegistryError::InvalidInput`] when
/// `stored` names another wallet or chain than `pin`.
pub fn resolve_wallet_list(
    pin: &WalletListPin,
    document: &[u8],
    stored: Option<&VerifiedWalletList>,
) -> Result<WalletListResolution, RegistryError> {
    if let Some(stored) = stored {
        let list = stored.list();
        if list.wallet_id != pin.wallet_id || list.chain_id != pin.chain_id {
            return Err(RegistryError::InvalidInput {
                message: format!(
                    "the stored wallet list is {} on {}, not {} on {}",
                    list.wallet_id, list.chain_id, pin.wallet_id, pin.chain_id
                ),
            });
        }
    }
    let keep_stored = |update| WalletListResolution {
        newest: stored.cloned(),
        update,
    };
    let candidate = match pin.verify(document) {
        Ok(candidate) => candidate,
        Err(error) => return Ok(keep_stored(WalletListUpdate::Rejected(error))),
    };
    let version = candidate.list().version;
    let update = match stored {
        Some(stored) if version < stored.list().version => WalletListUpdate::Older { version },
        Some(stored) if version == stored.list().version => {
            if candidate.payload() == stored.payload() {
                WalletListUpdate::Unchanged
            } else {
                WalletListUpdate::ConflictingPayload
            }
        }
        None | Some(_) => {
            return Ok(WalletListResolution {
                newest: Some(candidate),
                update: WalletListUpdate::Adopted,
            })
        }
    };
    Ok(keep_stored(update))
}

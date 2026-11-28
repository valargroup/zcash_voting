//! The proxy delegation registry client (contract §11.10, §11.11 and §12).
//! It is transport-agnostic. The host fetches bytes and runs the
//! participation reader's proofs. In the order a round uses them:
//!
//! 1. [`proxy_service_urls`] reads the dynamic config's URL lists.
//! 2. [`parse_registry_page`] fills a [`RegistryCache`].
//! 3. [`resolve_wallet_list`] verifies the wallet's signed list.
//! 4. A [`RegistryView`] browses, searches handles and answers a paste or
//!    scan.
//! 5. [`confirm_proof_requests`] and [`check_delegates`] check each chosen
//!    delegate's proven entry before a commit.
//!
//! Only proven entries are trusted. The cache can at most change what is
//! shown, never what a commit accepts.

mod listing;
mod pre_proving;
mod proofs;
mod registry_cache;
mod service_urls;
mod view;
mod wallet_list;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use thiserror::Error;

pub(in crate::proxy) use listing::{identity_provider, provider_view};
pub use pre_proving::{check_delegates, confirm_proof_requests, ConfirmContext, ConfirmedDelegate};
pub use proofs::{
    DelegateEntryError, ProvenDelegateEntry, ProvenRecord, ProvenValue, RegistryProofRequest,
    MAX_REGISTRY_ENTRY_BYTES, REGISTRY_PROOF_STORE,
};
pub use registry_cache::{
    parse_registry_page, registry_page_path, RegistryCache, RegistryPage, REGISTRY_PAGE_LIMIT,
};
pub use service_urls::{proxy_service_urls, ProxyServiceUrls};
pub use view::{PasteQuery, RegistryView};
pub use wallet_list::{resolve_wallet_list, WalletListPin, WalletListResolution, WalletListUpdate};
pub use zcash_vote_delegate::{
    PinnedListKey, VerifiedWalletList, WalletList, WalletListError, WalletListRow,
};

/// A registry input is malformed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RegistryError {
    /// The host passed a malformed or inconsistent value.
    #[error("invalid input: {message}")]
    InvalidInput {
        /// What is wrong.
        message: String,
    },
}

/// Exactly `N` bytes from canonical base64. See [`base64_bytes`].
pub(in crate::proxy) fn base64_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    base64_bytes(text)?.try_into().ok()
}

/// Bytes from padded standard base64 with zero padding bits, as Go's
/// `encoding/json` writes (contract §0). The engine rejects any other
/// encoding, so equal bytes have one accepted text.
pub(in crate::proxy) fn base64_bytes(text: &str) -> Option<Vec<u8>> {
    BASE64.decode(text).ok()
}

#[cfg(test)]
mod tests;

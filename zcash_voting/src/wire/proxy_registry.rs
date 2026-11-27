//! Registry types of the client API (proxy delegation contracts-v1 §15.1),
//! with the contract's names, bridged to Dart.

use serde::{Deserialize, Serialize};

use super::ProxyDelegateProvider;

/// A delegate key as people compare it: its `zvdk1…` string and fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DelegateKeyInfo {
    /// The key's canonical lower-case bech32m string, `zvdk1…`.
    pub dk: String,
    /// The key's fingerprint, grouped as `xxxx-xxxx-xxxx-xxxx`.
    pub fingerprint: String,
}

/// The answer to a paste or scan (contract §11.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateLookup {
    /// The text names this delegate.
    Found {
        /// The delegate.
        listing: ProxyDelegateListing,
    },
    /// The text names no delegate in the registry cache.
    NotFound,
    /// The text is not a marker post, a key, an index or a fingerprint.
    Unrecognized,
}

/// A registry entry's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateEntryStatus {
    /// The delegate can ballot.
    Active,
    /// Coordinators suspended the entry, so it cannot ballot or retire.
    Suspended,
    /// The entry is retired for good, by its own key or by coordinators.
    Retired,
}

/// One delegate as Vizor shows it (contract §15.1), from the registry cache,
/// or at confirm from the proven entry (§11.11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyDelegateListing {
    /// The registry index.
    pub index: u32,
    /// The provider of the entry's account.
    pub provider: ProxyDelegateProvider,
    /// The entry's handle, without `@`.
    pub handle: String,
    /// The entry's key. A choice of this delegate carries the index,
    /// provider, handle and key shown here.
    pub key: DelegateKeyInfo,
    /// A label: the wallet's list has a row for this index with the entry's
    /// key, provider and handle, and the entry is active.
    pub reviewed: bool,
    /// The name in the wallet list's row for this index, if it has one.
    pub display_name: Option<String>,
    /// The wallet list row's statement.
    pub statement: Option<String>,
    /// The wallet list row's WebP picture; the identicon shows without one.
    pub picture: Option<Vec<u8>>,
    /// The entry was registered after the round was created. Display only.
    pub new_this_round: bool,
    /// Whether the entry is active, suspended or retired.
    pub status: DelegateEntryStatus,
    /// A wallet list handle or reserved name the handle looks like, shown
    /// as "Looks like @member".
    pub lookalike_of: Option<String>,
}

/// What browse shows: the wallet's verified list (contract §11.10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyDelegateBrowse {
    /// The list's rows the registry cache holds, in list order.
    pub listed: Vec<ProxyDelegateListing>,
    /// The shown list's version.
    pub list_version: Option<u64>,
    /// When the shown list was signed, in unix seconds.
    pub list_issued_at: Option<u64>,
    /// No verified list is available, so browse is empty.
    pub list_unavailable: bool,
}

/// The pre-proving check's verdict on one chosen delegate, at confirm
/// (contract §11.11). Only `Ok` lets a commit through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateCheck {
    /// The proven entry is active and holds the choice's key, provider and
    /// handle.
    Ok,
    /// Coordinators suspended the entry.
    Suspended,
    /// The entry is retired.
    Retired,
    /// The chain has no entry with this index.
    NotFound,
    /// The proven entry's key, provider or handle is not the choice's, so it
    /// is not the delegate the delegator picked.
    Mismatch,
    /// The entry could not be proven. Retry later.
    CannotVerify,
}

//! Builds a [`ProxyDelegateListing`] from one registry entry (contract
//! §11.7, §11.10 and §11.11). Browse, search and paste use the cache.
//! Confirm uses the proven entry.

use zcash_vote_delegate::{
    are_lookalikes, DelegateStatus, IdentityProvider, ProxyDelegateEntry, WalletList, WalletListRow,
};

use crate::wire::{
    DelegateEntryStatus, DelegateKeyInfo, ProxyDelegateListing, ProxyDelegateProvider,
};

/// The listing of `entry`, with the content of the wallet list's row for its
/// index. `first_new_index` is the round's `next_delegate_index_at_creation`,
/// when the host knows it.
pub(super) fn listing(
    entry: &ProxyDelegateEntry,
    wallet_list: Option<&WalletList>,
    first_new_index: Option<u32>,
) -> ProxyDelegateListing {
    let row = wallet_list.and_then(|list| list.row(entry.index));
    ProxyDelegateListing {
        index: entry.index,
        provider: provider_view(entry.provider),
        handle: entry.handle.clone(),
        key: DelegateKeyInfo {
            dk: entry.dk.encode(),
            fingerprint: entry.dk.fingerprint().to_string(),
        },
        reviewed: row.is_some_and(|row| row.reviews(entry)),
        display_name: row.map(|row| row.name.clone()),
        statement: row.and_then(|row| row.statement.clone()),
        picture: row.and_then(|row| row.picture.clone()),
        new_this_round: first_new_index.is_some_and(|first_new| entry.index >= first_new),
        status: match entry.status {
            DelegateStatus::Active => DelegateEntryStatus::Active,
            DelegateStatus::Suspended => DelegateEntryStatus::Suspended,
            DelegateStatus::Retired => DelegateEntryStatus::Retired,
        },
        lookalike_of: lookalike_of(wallet_list, entry),
    }
}

/// The first wallet list handle, then reserved name, that `entry`'s handle
/// looks like (contract §11.7). The entry's own row is skipped: the one with
/// its index, provider and handle.
fn lookalike_of(wallet_list: Option<&WalletList>, entry: &ProxyDelegateEntry) -> Option<String> {
    let wallet_list = wallet_list?;
    let own = |row: &WalletListRow| {
        row.index == entry.index
            && row.provider == entry.provider.code()
            && row.handle == entry.handle
    };
    let rows = wallet_list
        .delegates
        .iter()
        .filter(|row| !own(row))
        .map(|row| row.handle.as_str());
    rows.chain(wallet_list.reserved_names.iter().map(String::as_str))
        .find(|member| are_lookalikes(&entry.handle, member))
        .map(str::to_owned)
}

/// The client API's form of a provider.
pub(in crate::proxy) fn provider_view(provider: IdentityProvider) -> ProxyDelegateProvider {
    match provider {
        IdentityProvider::X => ProxyDelegateProvider::X,
        IdentityProvider::GitHub => ProxyDelegateProvider::GitHub,
    }
}

/// The provider a client API value names.
pub(in crate::proxy) fn identity_provider(provider: ProxyDelegateProvider) -> IdentityProvider {
    match provider {
        ProxyDelegateProvider::X => IdentityProvider::X,
        ProxyDelegateProvider::GitHub => IdentityProvider::GitHub,
    }
}

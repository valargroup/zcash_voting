//! Finding a delegate (contract §11.10): browse the wallet list, exact handle
//! search and paste or scan, all over the registry cache. All of it is
//! display only. Nothing here is trusted at commit.

use zcash_vote_delegate::{
    normalize_handle, DelegateFingerprint, DelegateKey, DelegateMarker, ProxyDelegateEntry,
    VerifiedWalletList, WalletList,
};

use super::{listing::listing, RegistryCache};
use crate::wire::{DelegateLookup, ProxyDelegateBrowse, ProxyDelegateListing};

/// Characters in an ungrouped fingerprint.
const FINGERPRINT_CHARACTERS: usize = 16;

/// What a wallet has for finding delegates in one round.
#[derive(Clone, Copy, Debug)]
pub struct RegistryView<'a> {
    /// The registry cache for the round's chain.
    pub cache: &'a RegistryCache,
    /// The newest verified wallet list. Without it browse is empty.
    pub wallet_list: Option<&'a VerifiedWalletList>,
    /// The round's `next_delegate_index_at_creation`, which labels entries
    /// new this round, when the host knows it.
    pub first_new_index: Option<u32>,
}

/// What a pasted or scanned text names, after trimming whitespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteQuery {
    /// A key: typed or scanned `zvdk1…`, or a marker post's key.
    Key(DelegateKey),
    /// An index, `17` or `#17`, in canonical decimal of at least 1.
    Index(u32),
    /// A fingerprint: 16 bech32 characters, either case, grouped by `-` or not.
    Fingerprint(DelegateFingerprint),
}

impl PasteQuery {
    /// What `text` names, or `None` when it is none of the accepted forms.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Ok(marker) = DelegateMarker::parse(text) {
            return Some(Self::Key(marker.dk));
        }
        if let Ok(dk) = DelegateKey::decode(text) {
            return Some(Self::Key(dk));
        }
        let digits = text.strip_prefix('#').unwrap_or(text);
        let canonical =
            !digits.starts_with('0') && digits.bytes().all(|byte| byte.is_ascii_digit());
        if let Some(index) = digits.parse::<u32>().ok().filter(|_| canonical) {
            return Some(Self::Index(index));
        }
        fingerprint_query(text).map(Self::Fingerprint)
    }
}

impl RegistryView<'_> {
    /// The wallet list's rows in list order, each from its cached entry. Rows
    /// the cache lacks are left out.
    pub fn browse(&self) -> ProxyDelegateBrowse {
        let Some(list) = self.list() else {
            return ProxyDelegateBrowse {
                listed: Vec::new(),
                list_version: None,
                list_issued_at: None,
                list_unavailable: true,
            };
        };
        let listed = list
            .delegates
            .iter()
            .filter_map(|row| self.cache.entry(row.index))
            .map(|entry| self.listing(entry))
            .collect();
        ProxyDelegateBrowse {
            listed,
            list_version: Some(list.version),
            list_issued_at: Some(list.issued_at),
            list_unavailable: false,
        }
    }

    /// Exact handle search: the cached live entries whose handle equals
    /// `query` after [`normalize_handle`], in index order.
    pub fn search(&self, query: &str) -> Vec<ProxyDelegateListing> {
        let handle = normalize_handle(query);
        self.cache
            .live_entries_with_handle(&handle)
            .map(|entry| self.listing(entry))
            .collect()
    }

    /// Paste or scan: the cached entry `text` names by key, index or
    /// fingerprint.
    pub fn lookup(&self, text: &str) -> DelegateLookup {
        let Some(query) = PasteQuery::parse(text) else {
            return DelegateLookup::Unrecognized;
        };
        let found = match query {
            PasteQuery::Key(dk) => self.cache.entry_with_key(&dk),
            PasteQuery::Fingerprint(fingerprint) => self.cache.entry_with_fingerprint(&fingerprint),
            PasteQuery::Index(index) => self.cache.entry(index),
        };
        match found {
            Some(entry) => DelegateLookup::Found {
                listing: self.listing(entry),
            },
            None => DelegateLookup::NotFound,
        }
    }

    /// The listing of `entry` in this view.
    fn listing(&self, entry: &ProxyDelegateEntry) -> ProxyDelegateListing {
        listing(entry, self.list(), self.first_new_index)
    }

    /// The verified list's content.
    fn list(&self) -> Option<&WalletList> {
        self.wallet_list.map(VerifiedWalletList::list)
    }
}

/// The fingerprint `text` spells, in the forms of [`PasteQuery::Fingerprint`].
fn fingerprint_query(text: &str) -> Option<DelegateFingerprint> {
    let compact = text.replace('-', "").to_ascii_lowercase();
    if compact.len() != FINGERPRINT_CHARACTERS || !compact.is_ascii() {
        return None;
    }
    let grouped = [
        &compact[..4],
        &compact[4..8],
        &compact[8..12],
        &compact[12..],
    ]
    .join("-");
    DelegateFingerprint::parse(&grouped).ok()
}

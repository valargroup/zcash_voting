//! The registry cache (contract §11.10), the wallet's unproven copy of
//! the whole registry. Fetching every entry hides which delegate the user
//! looks at. The host chooses when to refresh and may persist its serde form.

use std::collections::{BTreeMap, HashMap};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use zcash_vote_delegate::{
    DelegateFingerprint, DelegateKey, DelegateStatus, IdentityProvider, ProxyDelegateEntry,
};

use super::encoding;
use crate::proxy::delegate::ChainReadError;

/// The page size hosts ask for, and the largest `limit` the chain accepts.
pub const REGISTRY_PAGE_LIMIT: u32 = 1_000;

/// The registry page of entries above `start_after`.
pub fn registry_page_path(start_after: u32) -> String {
    format!("/shielded-vote/v1/delegates?start_after={start_after}&limit={REGISTRY_PAGE_LIMIT}")
}

/// One page of the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryPage {
    /// The page's entries, in ascending index.
    pub entries: Vec<ProxyDelegateEntry>,
    /// The `start_after` of the next page, or `None` on the last page.
    pub next_start_after: Option<u32>,
}

/// Reads the answer to [`registry_page_path`]`(start_after)`.
///
/// The page must hold at most [`REGISTRY_PAGE_LIMIT`] entries, strictly
/// ascending above `start_after`, and continue after its last entry or end
/// with `next_start_after` 0, so a server cannot skip an entry. Each entry
/// must be one a registration could have written. Omitted fields read as
/// zero and unknown ones are ignored.
///
/// Fails with [`ChainReadError::Unavailable`] for a status other than 200 and
/// [`ChainReadError::Malformed`] for a page that breaks these rules.
pub fn parse_registry_page(
    start_after: u32,
    status: u16,
    body: &[u8],
) -> Result<RegistryPage, ChainReadError> {
    if status != 200 {
        return Err(ChainReadError::Unavailable { status });
    }
    let malformed = |message: String| ChainReadError::Malformed { message };
    let page: PageWire = serde_json::from_slice(body)
        .map_err(|error| malformed(format!("the registry page does not parse: {error}")))?;
    if page.delegates.len() > REGISTRY_PAGE_LIMIT as usize {
        return Err(malformed(format!(
            "the registry page holds {} entries, more than {REGISTRY_PAGE_LIMIT}",
            page.delegates.len()
        )));
    }
    let mut last_index = start_after;
    let mut entries = Vec::with_capacity(page.delegates.len());
    for wire in page.delegates {
        if wire.index <= last_index {
            return Err(malformed(format!(
                "entry {} is out of order after {last_index}",
                wire.index
            )));
        }
        last_index = wire.index;
        entries.push(wire.into_entry().map_err(malformed)?);
    }
    let next_start_after = match page.next_start_after {
        0 => None,
        next if next == last_index && !entries.is_empty() => Some(next),
        next => {
            return Err(malformed(format!(
                "the registry page continues after {next}, not after its last entry"
            )))
        }
    };
    Ok(RegistryPage {
        entries,
        next_start_after,
    })
}

/// The wallet's copy of the registry on one chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryCache {
    chain_id: String,
    entries: BTreeMap<u32, ProxyDelegateEntry>,
    /// The entry holding each key.
    entry_by_key: HashMap<DelegateKey, u32>,
}

impl RegistryCache {
    /// An empty cache for `chain_id`.
    pub fn new(chain_id: impl Into<String>) -> Self {
        Self {
            chain_id: chain_id.into(),
            entries: BTreeMap::new(),
            entry_by_key: HashMap::new(),
        }
    }

    /// The cache built from one download's pages, in order.
    pub fn from_pages(
        chain_id: impl Into<String>,
        pages: impl IntoIterator<Item = RegistryPage>,
    ) -> Self {
        let mut cache = Self::new(chain_id);
        for page in pages {
            cache.insert_page(page);
        }
        cache
    }

    /// Adds `page`'s entries, replacing cached entries with the same index.
    pub fn insert_page(&mut self, page: RegistryPage) {
        for entry in page.entries {
            self.insert(entry);
        }
    }

    /// The chain the cache describes.
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// How many entries the cache holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entry `index`, if cached.
    pub fn entry(&self, index: u32) -> Option<&ProxyDelegateEntry> {
        self.entries.get(&index)
    }

    /// The cached entry whose key is `dk`.
    pub fn entry_with_key(&self, dk: &DelegateKey) -> Option<&ProxyDelegateEntry> {
        self.entry_by_key
            .get(dk)
            .and_then(|index| self.entries.get(index))
    }

    /// The first cached entry by index whose key has `fingerprint`.
    pub fn entry_with_fingerprint(
        &self,
        fingerprint: &DelegateFingerprint,
    ) -> Option<&ProxyDelegateEntry> {
        self.entries
            .values()
            .find(|entry| entry.dk.fingerprint() == *fingerprint)
    }

    /// The cached live entries with the canonical `handle`, in index order.
    /// The chain keeps at most one per provider.
    pub fn live_entries_with_handle<'a>(
        &'a self,
        handle: &'a str,
    ) -> impl Iterator<Item = &'a ProxyDelegateEntry> {
        self.entries
            .values()
            .filter(move |entry| entry.status != DelegateStatus::Retired && entry.handle == handle)
    }

    /// Caches `entry`, dropping the key of the entry it replaces.
    fn insert(&mut self, entry: ProxyDelegateEntry) {
        if let Some(previous) = self.entries.remove(&entry.index) {
            self.entry_by_key.remove(&previous.dk);
        }
        self.entry_by_key.insert(entry.dk, entry.index);
        self.entries.insert(entry.index, entry);
    }
}

impl Serialize for RegistryCache {
    /// Writes `{chain_id, entries}`, each entry as base64 of its protobuf form.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        StoredCache {
            chain_id: self.chain_id.clone(),
            entries: self
                .entries
                .values()
                .map(|entry| STANDARD.encode(entry.encode()))
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RegistryCache {
    /// Reads what [`Serialize`] wrote, checking every entry as a page does.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = StoredCache::deserialize(deserializer)?;
        let mut cache = Self::new(stored.chain_id);
        for encoded in stored.entries {
            let entry = encoding::base64_bytes(&encoded)
                .ok_or_else(|| "an entry is not canonical base64".to_owned())
                .and_then(|bytes| {
                    ProxyDelegateEntry::decode(&bytes).map_err(|error| error.to_string())
                })
                .map_err(de::Error::custom)?;
            cache.insert(entry);
        }
        Ok(cache)
    }
}

/// The serde form of [`RegistryCache`].
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCache {
    chain_id: String,
    entries: Vec<String>,
}

/// `QueryProxyDelegatesResponse` as the REST API encodes it.
#[derive(Deserialize)]
struct PageWire {
    #[serde(default)]
    delegates: Vec<EntryWire>,
    #[serde(default)]
    next_start_after: u32,
}

/// One `ProxyDelegateEntry`, with bytes in standard base64.
#[derive(Deserialize)]
struct EntryWire {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    dk: String,
    #[serde(default)]
    provider: u32,
    #[serde(default)]
    handle: String,
    #[serde(default)]
    proof_id: String,
    #[serde(default)]
    status: u32,
    #[serde(default)]
    status_reason: u32,
    #[serde(default)]
    registered_height: u64,
}

impl EntryWire {
    /// The entry, refused unless a registration could have written it.
    fn into_entry(self) -> Result<ProxyDelegateEntry, String> {
        let index = self.index;
        let invalid = |field: &str| format!("entry {index}'s {field} is invalid");
        let entry = ProxyDelegateEntry {
            index,
            dk: encoding::base64_array::<32>(&self.dk)
                .and_then(|dk| DelegateKey::from_bytes(dk).ok())
                .ok_or_else(|| invalid("dk"))?,
            provider: IdentityProvider::from_code(self.provider)
                .ok_or_else(|| invalid("provider"))?,
            handle: self.handle,
            proof_id: self.proof_id,
            status: DelegateStatus::from_code(self.status).ok_or_else(|| invalid("status"))?,
            status_reason: self.status_reason,
            registered_height: self.registered_height,
        };
        entry
            .check_fields()
            .map_err(|error| format!("entry {index}: {error}"))?;
        Ok(entry)
    }
}

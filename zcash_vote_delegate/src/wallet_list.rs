//! The wallet list (contract §12.3): a signed file per wallet and chain
//! holding the wallet's curated delegates.
//!
//! Every member except `signatures` is signed, so unknown members are refused.
//! The caller keeps the newest verified version per wallet and chain.

use std::{collections::BTreeSet, fmt};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de, Deserialize, Deserializer};
use unicode_normalization::is_nfc;

use crate::{
    delegate_key::decode_key_string, is_canonical_handle, verify_zip215, DelegateStatus,
    IdentityProvider, ProxyDelegateEntry,
};

/// The wallet list's `format`.
pub const WALLET_LIST_FORMAT: &str = "zcash-vote-wallet-list/v1";
/// The 34-byte domain that starts every wallet list payload.
pub const WALLET_LIST_DOMAIN: [u8; 34] = *b"zcash-shielded-vote:wallet-list:v1";
/// The largest wallet list document, in bytes.
pub const MAX_WALLET_LIST_BYTES: usize = 4 * 1024 * 1024;
/// The most rows one wallet list holds.
pub const MAX_WALLET_LIST_ROWS: usize = 1_000;
/// The most reserved names one wallet list holds.
pub const MAX_RESERVED_NAMES: usize = 1_000;
/// The longest reserved name, in bytes.
pub const MAX_RESERVED_NAME_BYTES: usize = 64;
/// The longest display name, in Unicode scalar values.
pub const MAX_LIST_NAME_CHARS: usize = 50;
/// The longest statement, in Unicode scalar values.
pub const MAX_LIST_STATEMENT_CHARS: usize = 280;
/// The largest picture, in bytes of WebP.
pub const MAX_LIST_PICTURE_BYTES: usize = 12_288;
/// The most signatures one wallet list carries.
pub const MAX_WALLET_LIST_SIGNATURES: usize = 8;
const MAX_WALLET_ID_BYTES: usize = 32;
const MAX_CHAIN_ID_BYTES: usize = 64;
const MAX_KEY_ID_BYTES: usize = 64;

/// A wallet list's signed content, in the document's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletList {
    /// The wallet the list belongs to, which also names its hosting path.
    pub wallet_id: String,
    /// The chain whose indexes the rows name.
    pub chain_id: String,
    /// Strictly increasing across a wallet's lists on one chain.
    pub version: u64,
    /// When the list was signed, in unix seconds. Display only.
    pub issued_at: u64,
    /// Lowercase handles that feed lookalike warnings (contract §11.7).
    pub reserved_names: Vec<String>,
    /// The curated delegates, strictly ascending by index.
    pub delegates: Vec<WalletListRow>,
}

/// One curated delegate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletListRow {
    /// The registry index.
    pub index: u32,
    /// The provider code: 1 for X, 2 for GitHub.
    pub provider: u32,
    /// The entry's canonical handle (contracts-v1 §2.5).
    pub handle: String,
    /// The entry's key the wallet reviewed, as its canonical `zvdk1…` string.
    pub dk: String,
    /// The display name.
    pub name: String,
    /// The statement, if the delegate sent one.
    pub statement: Option<String>,
    /// The picture's WebP bytes, if the delegate sent one.
    pub picture: Option<Vec<u8>>,
}

impl WalletListRow {
    /// Whether `entry` shows as reviewed: it is the entry at the row's index,
    /// active, with the row's key, provider and handle.
    pub fn reviews(&self, entry: &ProxyDelegateEntry) -> bool {
        entry.index == self.index
            && entry.status == DelegateStatus::Active
            && entry.dk.encode() == self.dk
            && entry.provider.code() == self.provider
            && entry.handle == self.handle
    }
}

impl WalletList {
    /// The row for delegate `index`, if the list names one.
    pub fn row(&self, index: u32) -> Option<&WalletListRow> {
        self.delegates
            .binary_search_by_key(&index, |row| row.index)
            .ok()
            .map(|position| &self.delegates[position])
    }

    /// Checks the field rules and builds `WalletListPayloadV1`, the bytes every
    /// signature covers: the domain, then each field raw or little-endian in
    /// list order, the key as its 32 bytes, and absent statements and pictures
    /// empty. Each field is checked just before it is written, which is the
    /// contract's table order, so the error is the first rule broken, row by
    /// row.
    pub fn payload(&self) -> Result<Vec<u8>, WalletListError> {
        if !is_wallet_id(&self.wallet_id) {
            return Err(WalletListError::InvalidWalletId);
        }
        if !is_chain_id(&self.chain_id) {
            return Err(WalletListError::InvalidChainId);
        }
        if self.version == 0 {
            return Err(WalletListError::InvalidVersion);
        }
        if self.issued_at == 0 {
            return Err(WalletListError::InvalidIssuedAt);
        }
        if self.reserved_names.len() > MAX_RESERVED_NAMES {
            return Err(WalletListError::TooManyReservedNames);
        }
        let mut payload = WALLET_LIST_DOMAIN.to_vec();
        push_lp8(&mut payload, self.wallet_id.as_bytes());
        push_lp8(&mut payload, self.chain_id.as_bytes());
        payload.extend_from_slice(&self.version.to_le_bytes());
        payload.extend_from_slice(&self.issued_at.to_le_bytes());
        push_count16(&mut payload, self.reserved_names.len());
        for (position, name) in self.reserved_names.iter().enumerate() {
            if !is_reserved_name(name) {
                return Err(WalletListError::InvalidReservedName);
            }
            if self.reserved_names[..position].contains(name) {
                return Err(WalletListError::RepeatedReservedName);
            }
            push_lp8(&mut payload, name.as_bytes());
        }
        if self.delegates.len() > MAX_WALLET_LIST_ROWS {
            return Err(WalletListError::TooManyDelegates);
        }
        push_count16(&mut payload, self.delegates.len());
        let mut previous_index = 0;
        for row in &self.delegates {
            if row.index == 0 {
                return Err(WalletListError::InvalidIndex);
            }
            if row.index <= previous_index {
                return Err(WalletListError::IndexNotAscending);
            }
            previous_index = row.index;
            let provider = IdentityProvider::from_code(row.provider)
                .ok_or(WalletListError::InvalidProvider)?;
            if !is_canonical_handle(provider, &row.handle) {
                return Err(WalletListError::InvalidHandle);
            }
            let dk = decode_key_string(&row.dk).map_err(|_| WalletListError::InvalidKey)?;
            if !is_display_text(&row.name, MAX_LIST_NAME_CHARS, false) {
                return Err(WalletListError::InvalidName);
            }
            if let Some(statement) = &row.statement {
                if !is_display_text(statement, MAX_LIST_STATEMENT_CHARS, true) {
                    return Err(WalletListError::InvalidStatement);
                }
            }
            if let Some(picture) = &row.picture {
                if picture.len() > MAX_LIST_PICTURE_BYTES {
                    return Err(WalletListError::PictureTooLarge);
                }
                if !is_webp_container(picture) {
                    return Err(WalletListError::InvalidPicture);
                }
            }
            payload.extend_from_slice(&row.index.to_le_bytes());
            payload.extend_from_slice(&row.provider.to_le_bytes());
            push_lp8(&mut payload, row.handle.as_bytes());
            payload.extend_from_slice(&dk);
            push_lp8(&mut payload, row.name.as_bytes());
            push_lp16(
                &mut payload,
                row.statement.as_deref().unwrap_or_default().as_bytes(),
            );
            push_lp16(&mut payload, row.picture.as_deref().unwrap_or_default());
        }
        Ok(payload)
    }
}

/// A key a wallet build pins for its list on one chain.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PinnedListKey {
    /// The `key_id` a signature object must carry.
    pub key_id: String,
    /// The Ed25519 public key that signature must verify under.
    pub public_key: [u8; 32],
}

/// A wallet list a pinned key signed. Only [`verify_wallet_list`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedWalletList {
    list: WalletList,
    payload: Vec<u8>,
    signed_by: String,
}

impl VerifiedWalletList {
    /// The list's signed content.
    pub fn list(&self) -> &WalletList {
        &self.list
    }

    /// The signed payload, which tells two lists of one version apart.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// The pinned `key_id` whose signature verified.
    pub fn signed_by(&self) -> &str {
        &self.signed_by
    }
}

/// Verifies a fetched wallet list for `wallet_id` on `chain_id`. Reports the
/// first problem in this order: size, top level and `format`, shape, field
/// rules, wallet, chain, signature.
///
/// A signature counts only when its `key_id` is a pinned key's and it passes
/// [`verify_zip215`] over [`WalletList::payload`] under that pinned key. Other
/// signatures are ignored, so keys trusted elsewhere never widen who signs.
pub fn verify_wallet_list(
    document: &[u8],
    wallet_id: &str,
    chain_id: &str,
    pinned_keys: &[PinnedListKey],
) -> Result<VerifiedWalletList, WalletListError> {
    if document.len() > MAX_WALLET_LIST_BYTES {
        return Err(WalletListError::TooLarge);
    }
    let format = document_format(document).ok_or(WalletListError::Malformed)?;
    if format != WALLET_LIST_FORMAT {
        return Err(WalletListError::UnsupportedFormat);
    }
    let wire: WalletListWire =
        serde_json::from_slice(document).map_err(|_| WalletListError::Malformed)?;
    let signatures = wire.signatures()?;
    let list = wire.into_list()?;
    let payload = list.payload()?;
    if list.wallet_id != wallet_id {
        return Err(WalletListError::WrongWallet);
    }
    if list.chain_id != chain_id {
        return Err(WalletListError::WrongChain);
    }
    let signed_by = pinned_keys
        .iter()
        .find(|pinned| {
            signatures.iter().any(|(key_id, sig)| {
                *key_id == pinned.key_id && verify_zip215(&pinned.public_key, &payload, sig).is_ok()
            })
        })
        .ok_or(WalletListError::NoPinnedSignature)?;
    Ok(VerifiedWalletList {
        signed_by: signed_by.key_id.clone(),
        list,
        payload,
    })
}

/// Why a wallet list document is refused, named as in contract §12.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WalletListError {
    /// The document is longer than [`MAX_WALLET_LIST_BYTES`].
    TooLarge,
    /// Not the contract's shape: not JSON, a repeated, unknown or absent
    /// member, `null`, a wrong type, non-canonical base64, or bad `signatures`.
    Malformed,
    /// `format` is not [`WALLET_LIST_FORMAT`].
    UnsupportedFormat,
    /// `wallet_id` is not 1 to 32 bytes of a lowercase letter, then lowercase
    /// letters, digits or `-`.
    InvalidWalletId,
    /// `chain_id` is not 1 to 64 printable non-space ASCII characters.
    InvalidChainId,
    /// `version` is 0.
    InvalidVersion,
    /// `issued_at` is 0.
    InvalidIssuedAt,
    /// More than [`MAX_RESERVED_NAMES`] reserved names.
    TooManyReservedNames,
    /// A reserved name is not 1 to [`MAX_RESERVED_NAME_BYTES`] lowercase
    /// ASCII letters, digits, `_` or `-`.
    InvalidReservedName,
    /// A reserved name appears twice.
    RepeatedReservedName,
    /// More than [`MAX_WALLET_LIST_ROWS`] rows.
    TooManyDelegates,
    /// A row's index is 0.
    InvalidIndex,
    /// A row's index is not above the previous row's.
    IndexNotAscending,
    /// A row's provider is not 1 or 2.
    InvalidProvider,
    /// A row's handle is not canonical for its provider.
    InvalidHandle,
    /// A row's key is not a canonical `zvdk1…` string. The Ed25519 key rule
    /// is not applied here: the chain holds only keys that pass it.
    InvalidKey,
    /// A row's name is not 1 to 50 scalar values of NFC display text.
    InvalidName,
    /// A row's statement is not 1 to 280 scalar values of NFC display text.
    InvalidStatement,
    /// A row's picture is larger than [`MAX_LIST_PICTURE_BYTES`].
    PictureTooLarge,
    /// A row's picture is not a WebP container whose size field covers it.
    InvalidPicture,
    /// The list belongs to another wallet.
    WrongWallet,
    /// The list is for another chain.
    WrongChain,
    /// No signature from a pinned key verifies.
    NoPinnedSignature,
}

impl WalletListError {
    /// The error's bare name in the contract and the golden vectors.
    pub const fn name(self) -> &'static str {
        match self {
            Self::TooLarge => "TooLarge",
            Self::Malformed => "Malformed",
            Self::UnsupportedFormat => "UnsupportedFormat",
            Self::InvalidWalletId => "InvalidWalletId",
            Self::InvalidChainId => "InvalidChainId",
            Self::InvalidVersion => "InvalidVersion",
            Self::InvalidIssuedAt => "InvalidIssuedAt",
            Self::TooManyReservedNames => "TooManyReservedNames",
            Self::InvalidReservedName => "InvalidReservedName",
            Self::RepeatedReservedName => "RepeatedReservedName",
            Self::TooManyDelegates => "TooManyDelegates",
            Self::InvalidIndex => "InvalidIndex",
            Self::IndexNotAscending => "IndexNotAscending",
            Self::InvalidProvider => "InvalidProvider",
            Self::InvalidHandle => "InvalidHandle",
            Self::InvalidKey => "InvalidKey",
            Self::InvalidName => "InvalidName",
            Self::InvalidStatement => "InvalidStatement",
            Self::PictureTooLarge => "PictureTooLarge",
            Self::InvalidPicture => "InvalidPicture",
            Self::WrongWallet => "WrongWallet",
            Self::WrongChain => "WrongChain",
            Self::NoPinnedSignature => "NoPinnedSignature",
        }
    }
}

impl fmt::Display for WalletListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wallet list refused: {}", self.name())
    }
}

impl std::error::Error for WalletListError {}

/// The document as published.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletListWire {
    #[allow(dead_code)]
    format: String,
    wallet_id: String,
    chain_id: String,
    version: u64,
    issued_at: u64,
    reserved_names: Vec<String>,
    delegates: Vec<WalletListRowWire>,
    signatures: Vec<SignatureWire>,
}

/// One row as published. `picture` is canonical base64.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletListRowWire {
    index: u32,
    provider: u32,
    handle: String,
    dk: String,
    name: String,
    #[serde(default, deserialize_with = "present_string")]
    statement: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    picture: Option<String>,
}

/// One signature, in the dynamic config's signature shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureWire {
    key_id: String,
    alg: String,
    sig: String,
}

impl WalletListWire {
    /// The signatures as `(key_id, sig)`. A bad `signatures` member is malformed.
    fn signatures(&self) -> Result<Vec<(String, [u8; 64])>, WalletListError> {
        if !(1..=MAX_WALLET_LIST_SIGNATURES).contains(&self.signatures.len()) {
            return Err(WalletListError::Malformed);
        }
        self.signatures
            .iter()
            .map(|signature| {
                let sig = STANDARD
                    .decode(&signature.sig)
                    .ok()
                    .and_then(|sig| <[u8; 64]>::try_from(sig).ok());
                match sig {
                    Some(sig)
                        if signature.alg == "ed25519"
                            && (1..=MAX_KEY_ID_BYTES).contains(&signature.key_id.len()) =>
                    {
                        Ok((signature.key_id.clone(), sig))
                    }
                    _ => Err(WalletListError::Malformed),
                }
            })
            .collect()
    }

    /// The signed content. A non-canonical base64 picture is malformed.
    fn into_list(self) -> Result<WalletList, WalletListError> {
        let delegates = self
            .delegates
            .into_iter()
            .map(|row| {
                let picture = row
                    .picture
                    .map(|text| {
                        STANDARD
                            .decode(&text)
                            .map_err(|_| WalletListError::Malformed)
                    })
                    .transpose()?;
                Ok(WalletListRow {
                    index: row.index,
                    provider: row.provider,
                    handle: row.handle,
                    dk: row.dk,
                    name: row.name,
                    statement: row.statement,
                    picture,
                })
            })
            .collect::<Result<_, WalletListError>>()?;
        Ok(WalletList {
            wallet_id: self.wallet_id,
            chain_id: self.chain_id,
            version: self.version,
            issued_at: self.issued_at,
            reserved_names: self.reserved_names,
            delegates,
        })
    }
}

/// Whether `id` is a wallet id (see [`WalletListError::InvalidWalletId`]).
fn is_wallet_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=MAX_WALLET_ID_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Whether `id` is a chain id (see [`WalletListError::InvalidChainId`]).
fn is_chain_id(id: &str) -> bool {
    (1..=MAX_CHAIN_ID_BYTES).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Whether `name` is a reserved name (see
/// [`WalletListError::InvalidReservedName`]).
fn is_reserved_name(name: &str) -> bool {
    (1..=MAX_RESERVED_NAME_BYTES).contains(&name.len())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Whether `character` is a bidirectional formatting character.
fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Whether `text` is display text: 1 to `max_chars` scalar values in NFC, with
/// no bidi formatting or control character (a line feed only if allowed).
fn is_display_text(text: &str, max_chars: usize, allow_line_feed: bool) -> bool {
    (1..=max_chars).contains(&text.chars().count())
        && is_nfc(text)
        && text.chars().all(|character| {
            !is_bidi_formatting(character)
                && (!character.is_control() || (allow_line_feed && character == '\n'))
        })
}

/// Whether `picture` is a WebP container: `RIFF`, a size field equal to the
/// length minus 8, then `WEBP`. The wallet decodes the image itself.
fn is_webp_container(picture: &[u8]) -> bool {
    let (Some(riff), Some(size), Some(form)) =
        (picture.get(..4), picture.get(4..8), picture.get(8..12))
    else {
        return false;
    };
    let size = u32::from_le_bytes(size.try_into().expect("a 4-byte slice"));
    riff == b"RIFF" && form == b"WEBP" && usize::try_from(size).ok() == picture.len().checked_sub(8)
}

/// Appends `lp8(bytes)`. The field rules keep the length below 256.
fn push_lp8(payload: &mut Vec<u8>, bytes: &[u8]) {
    let length = u8::try_from(bytes.len()).expect("the field rules bound every one-byte count");
    payload.push(length);
    payload.extend_from_slice(bytes);
}

/// Appends `lp16(bytes)` (a `u16le` length). The field rules bound it.
fn push_lp16(payload: &mut Vec<u8>, bytes: &[u8]) {
    push_count16(payload, bytes.len());
    payload.extend_from_slice(bytes);
}

/// Appends a count the field rules keep below 65,536, as `u16le`.
fn push_count16(payload: &mut Vec<u8>, count: usize) {
    let count = u16::try_from(count).expect("the field rules bound every two-byte count");
    payload.extend_from_slice(&count.to_le_bytes());
}

/// A document's top-level `format`, read first so a later format is told
/// apart from a malformed document. `None` when the top level is not a JSON
/// object, `format` is not a string, or a key repeats (parsers disagree on
/// which value wins).
fn document_format(document: &[u8]) -> Option<String> {
    match serde_json::from_slice::<TopLevel>(document) {
        Ok(TopLevel(Some(serde_json::Value::String(format)))) => Some(format),
        _ => None,
    }
}

/// The top-level object's `format` member, if it has one.
struct TopLevel(Option<serde_json::Value>);

/// Visits the top-level object, refusing a repeated key.
struct TopLevelVisitor;

impl<'de> de::Visitor<'de> for TopLevelVisitor {
    type Value = TopLevel;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<TopLevel, A::Error> {
        let mut keys = BTreeSet::new();
        let mut format = None;
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("repeated key {key:?}")));
            }
            if key == "format" {
                format = Some(map.next_value()?);
            } else {
                map.next_value::<de::IgnoredAny>()?;
            }
        }
        Ok(TopLevel(format))
    }
}

impl<'de> Deserialize<'de> for TopLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(TopLevelVisitor)
    }
}

/// Deserializes an optional string member, refusing `null` so absence is the
/// only way to omit it. Use with `#[serde(default)]`.
fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

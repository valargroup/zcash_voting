//! Proxy delegation formats shared by the vote chain, wallets, Vizor and
//! delegate tooling (proxy delegation contracts-v1).
//!
//! The crate is pure. It encodes, hashes, parses and checks, and never touches
//! the network, storage or a proving system. By contract section:
//!
//! - Digests (§0, §4): [`proxy_batch_sighash`] (D1), [`proxy_batch_digest`]
//!   and the registry digests such as [`register_digest`].
//! - Canonical handles and proof ids (§2.5): [`is_canonical_handle`] and
//!   [`is_canonical_proof_id`].
//! - Chain state (§3): the store keys and [`ProxyDelegateEntry`].
//! - Ed25519 (§0): [`validate_ed25519_key`] and [`verify_zip215`].
//! - Delegate key (§11.2): [`DelegateSigningKey`] and [`DelegateKey`].
//! - DC secrets (§11.1): [`hotkey_round_key`] and [`dc_seed`].
//! - Marker and lookalikes (§11.5, §11.7): [`DelegateMarker`] and
//!   [`are_lookalikes`].
//! - Wallet list (§12.3): [`verify_wallet_list`].
//! - Chain errors (§6): [`ProxyChainError`].
//!
//! Golden vectors live in `test-vectors/proxy_delegation_v1.json` and
//! `test-vectors/proxy_delegation_chain_v1.json` (copied from vote-sdk).

mod chain_errors;
mod dc_secrets;
mod delegate_entry;
mod delegate_key;
mod digest_encoding;
mod digests;
mod ed25519_rule;
mod handle;
mod lookalike;
mod marker;
mod store_keys;
mod wallet_list;

pub use chain_errors::{ProxyChainError, VOTE_CODESPACE};
pub use dc_secrets::{dc_seed, hotkey_round_key, DcSeedInputs, DcShareLayout, ZcashNetwork};
pub use delegate_entry::{DelegateStatus, EntryDecodeError, ProxyDelegateEntry};
pub use delegate_key::{
    DelegateFingerprint, DelegateKey, DelegateKeyDecodeError, DelegateSigningKey,
    FingerprintParseError, DELEGATE_KEY_HRP, DELEGATE_SECRET_HRP,
};
pub use digest_encoding::DigestError;
use digest_encoding::DigestPreimage;
pub use digests::{
    attest_digest, ballot_digest, proxy_batch_digest, proxy_batch_sighash, register_digest,
    retire_digest, AttestationWindow, DelegateBallotFields, DelegateRegisterFields,
    DelegateRetireFields, IdentityProvider, ProxyDelegationActionFields,
    ProxyDelegationBatchFields, DELEGATE_ATTEST_DOMAIN, DELEGATE_BALLOT_DOMAIN,
    DELEGATE_REGISTER_DOMAIN, DELEGATE_RETIRE_DOMAIN, PROXY_DELEGATION_BATCH_SIGHASH_DOMAIN,
    PROXY_DELEGATION_DOMAINS,
};
pub use ed25519_rule::{validate_ed25519_key, verify_zip215, Ed25519KeyError, SignatureError};
pub use handle::{is_canonical_handle, is_canonical_proof_id, normalize_handle};
pub use lookalike::{are_lookalikes, fold_handle};
pub use marker::{DelegateMarker, MarkerError, MARKER_PREFIX, MARKER_VERSION};
pub use store_keys::{
    delegate_ballot_store_key, delegate_entry_store_key, delegate_handle_store_key,
    pool_share_count_store_key, used_delegate_key_store_key, DELEGATE_BALLOT_PREFIX,
    DELEGATE_REGISTRY_PREFIX, NEXT_DELEGATE_INDEX_STORE_KEY, POOL_PROPOSAL_ID,
    PROXY_PARAMS_STORE_KEY, SHARE_COUNT_PREFIX,
};
pub use wallet_list::{
    verify_wallet_list, PinnedListKey, VerifiedWalletList, WalletList, WalletListError,
    WalletListRow, MAX_LIST_NAME_CHARS, MAX_RESERVED_NAMES, MAX_RESERVED_NAME_BYTES,
    MAX_WALLET_LIST_BYTES, MAX_WALLET_LIST_ROWS, MAX_WALLET_LIST_SIGNATURES, WALLET_LIST_DOMAIN,
    WALLET_LIST_FORMAT,
};

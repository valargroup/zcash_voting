//! Keys of the delegate registry and ballot records in the chain's `vote`
//! store (contract §3.2). Integers are big-endian, as in vote-sdk's
//! `x/vote/types/keys.go`.

use crate::IdentityProvider;

/// Prefix byte of every delegate registry record.
pub const DELEGATE_REGISTRY_PREFIX: u8 = 0x1A;
/// Prefix byte of the per-round delegate ballot records.
pub const DELEGATE_BALLOT_PREFIX: u8 = 0x1B;
/// Prefix byte of the share-count records, per `(round, proposal, decision)`.
pub const SHARE_COUNT_PREFIX: u8 = 0x0B;
/// The proposal id pools are tallied under (contract §10). A DC share names
/// its delegate in `vote_decision`.
pub const POOL_PROPOSAL_ID: u32 = 0;

/// Key of the next delegate index (`0x1A 0x04`), a `u32`. Absent reads as 1.
pub const NEXT_DELEGATE_INDEX_STORE_KEY: [u8; 2] = [DELEGATE_REGISTRY_PREFIX, 0x04];
/// Key of `ProxyDelegationParams` (`0x1A 0x06`). Absent reads as unpaused,
/// with no verifiers and `max_delegates = 0`.
pub const PROXY_PARAMS_STORE_KEY: [u8; 2] = [DELEGATE_REGISTRY_PREFIX, 0x06];

/// Key of entry `d`'s `ProxyDelegateEntry` record: `0x1A 0x01 u32be(d)`.
pub fn delegate_entry_store_key(delegate_index: u32) -> [u8; 6] {
    let mut key = [0u8; 6];
    key[..2].copy_from_slice(&[DELEGATE_REGISTRY_PREFIX, 0x01]);
    key[2..].copy_from_slice(&delegate_index.to_be_bytes());
    key
}

/// Key of the record marking `dk` as registered: `0x1A 0x02 dk(32)`. Its value
/// is the entry's `u32` index. It outlives retirement, so a key is never
/// registered twice.
pub fn used_delegate_key_store_key(dk: &[u8; 32]) -> [u8; 34] {
    let mut key = [0u8; 34];
    key[..2].copy_from_slice(&[DELEGATE_REGISTRY_PREFIX, 0x02]);
    key[2..].copy_from_slice(dk);
    key
}

/// Key of the record naming a handle's live entry:
/// `0x1A 0x03 u8(provider) handle`. Its value is the entry's `u32` index. It
/// is deleted at retirement.
pub fn delegate_handle_store_key(provider: IdentityProvider, handle: &str) -> Vec<u8> {
    let mut key = vec![DELEGATE_REGISTRY_PREFIX, 0x03, provider.code() as u8];
    key.extend_from_slice(handle.as_bytes());
    key
}

/// Key of delegate `d`'s ballot record in a round: `0x1B round(32) u32be(d)`.
pub fn delegate_ballot_store_key(vote_round_id: &[u8; 32], delegate_index: u32) -> [u8; 37] {
    let mut key = [0u8; 37];
    key[0] = DELEGATE_BALLOT_PREFIX;
    key[1..33].copy_from_slice(vote_round_id);
    key[33..].copy_from_slice(&delegate_index.to_be_bytes());
    key
}

/// Key of delegate `d`'s pool share count in a round:
/// `0x0B round(32) u32be(0) u32be(d)`. Its value is a `u64`, absent reads as 0.
/// Unproven, read by a delegate's dashboard.
pub fn pool_share_count_store_key(vote_round_id: &[u8; 32], delegate_index: u32) -> [u8; 41] {
    let mut key = [0u8; 41];
    key[0] = SHARE_COUNT_PREFIX;
    key[1..33].copy_from_slice(vote_round_id);
    key[33..37].copy_from_slice(&POOL_PROPOSAL_ID.to_be_bytes());
    key[37..].copy_from_slice(&delegate_index.to_be_bytes());
    key
}

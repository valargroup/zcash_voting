//! Proxy delegation: handing part of a round's voting weight to public
//! delegates through delegation commitments (DCs), per proxy delegation
//! contracts-v1.
//!
//! So far it holds the registry client: [`registry`] reads the delegate
//! lists and checks each chosen delegate's proven entry. No existing flow
//! calls into this module yet.

pub mod delegate;
pub mod registry;

/// Number of DC slots in one bundle's batch: `dc_slot` is in
/// `0..MAX_DC_SLOTS` (contract §10, `DC_SLOT_BITS = 3`).
pub const MAX_DC_SLOTS: u8 = 8;

pub use zcash_vote_delegate::{DelegateKey, POOL_PROPOSAL_ID};

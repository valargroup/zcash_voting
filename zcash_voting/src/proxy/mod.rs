//! Proxy delegation: handing part of a round's voting weight to public
//! delegates through delegation commitments (DCs), per proxy delegation
//! contracts-v1.
//!
//! So far it holds the registry client: [`registry`] reads the delegate
//! lists and checks each chosen delegate's proven entry. No existing flow
//! calls into this module yet.

pub mod delegate;
pub mod registry;

pub use zcash_vote_delegate::DelegateKey;

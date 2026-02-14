//! Vote proof ZKP circuit (ZKP #2).
//!
//! Proves that a vote is well-formed and authorized with respect to
//! delegation and the vote commitment tree. The circuit verifies 11
//! conditions; constraint logic is added incrementally.
//!
//! Currently implemented:
//! - **Condition 2**: VAN Integrity (Poseidon hash).

pub mod circuit;

pub use circuit::{
    van_integrity_hash, Circuit, Config, Instance, K, DOMAIN_VAN, VOTE_COMM_TREE_DEPTH,
};

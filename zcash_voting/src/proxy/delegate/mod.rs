//! Delegate mode (plan §4.4, §4.5): what a public delegate's own device
//! needs to register, ballot and retire.
//!
//! So far it holds only [`ChainReadError`], which the registry client's
//! chain reads return too.

mod chain_reads;

pub use chain_reads::ChainReadError;

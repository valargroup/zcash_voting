//! Vote proof ZKP circuit (ZKP #2).
//!
//! Proves that a vote is well-formed and authorized with respect to
//! delegation and the vote commitment tree. Constraint logic to be added
//! per the written spec (Gov Steps V1) and Figma constraint flow.

pub mod circuit;

pub use circuit::{Circuit, Config, Instance, K};

//! The shared golden vectors, `test-vectors/proxy_delegation_v1.json`
//! (contracts-v1 §13).
//!
//! [`model`] is the file's JSON shape and [`cases`] recomputes each case's
//! expected outputs from its inputs.

pub mod cases;
pub mod model;

use std::fmt::Debug;

use serde::Serialize;

/// The committed vector file, embedded at compile time.
pub const COMMITTED_VECTORS: &str = include_str!("../../test-vectors/proxy_delegation_v1.json");

/// Where the writer test rewrites the vector file.
pub const VECTOR_FILE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/test-vectors/proxy_delegation_v1.json"
);

/// The bare name the file records for an error (contracts-v1 §13): the
/// variant name that starts its derived `Debug` form, without the variant's
/// fields or wrapped error.
pub fn error_name(error: impl Debug) -> String {
    let debug = format!("{error:?}");
    let end = debug
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(debug.len());
    debug[..end].to_owned()
}

/// Decodes lowercase hex, panicking on malformed input.
pub fn decode_hex(hex: &str) -> Vec<u8> {
    hex::decode(hex).unwrap_or_else(|error| panic!("invalid hex {hex:?}: {error}"))
}

/// Decodes hex of exactly `N` bytes.
pub fn bytes<const N: usize>(hex: &str) -> [u8; N] {
    decode_hex(hex)
        .try_into()
        .unwrap_or_else(|value: Vec<u8>| panic!("expected {N} bytes, got {}", value.len()))
}

/// Decodes hex of exactly 32 bytes.
pub fn hex32(hex: &str) -> [u8; 32] {
    bytes(hex)
}

/// Encodes bytes as lowercase hex.
pub fn hex_of(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// A file's exact text: pretty-printed JSON with a trailing newline.
pub fn render(file: &impl Serialize) -> String {
    let mut text = serde_json::to_string_pretty(file).expect("the vector model serializes");
    text.push('\n');
    text
}

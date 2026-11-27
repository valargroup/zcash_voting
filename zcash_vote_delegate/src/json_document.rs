//! Helpers for the off-chain JSON documents (contract §0).

use std::{collections::BTreeSet, fmt};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{de, Deserialize, Deserializer};

/// A document's top-level `format`, read first so a later format is told
/// apart from a malformed document. `None` when the top level is not a JSON
/// object, `format` is not a string, or a key repeats (parsers disagree on
/// which value wins).
pub(crate) fn document_format(document: &[u8]) -> Option<String> {
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

/// Bytes from canonical standard base64 (padded, zero padding bits), so equal
/// bytes have one accepted text.
pub(crate) fn decode_canonical_base64(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// Deserializes an optional string member, refusing `null` so absence is the
/// only way to omit it. Use with `#[serde(default)]`.
pub(crate) fn present_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

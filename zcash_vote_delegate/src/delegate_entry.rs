//! The delegate registry entry (contract §3.1), stored at
//! [`delegate_entry_store_key`](crate::delegate_entry_store_key) and proven by
//! wallets before a commit (§11.6).
//!
//! The chain writes deterministic protobuf: fields in number order, zero values
//! omitted, minimal varints. [`ProxyDelegateEntry::decode`] accepts only those
//! bytes, so a value the chain could not have written is refused.

use std::fmt;

use crate::{is_canonical_handle, is_canonical_proof_id, DelegateKey, IdentityProvider};

const WIRE_VARINT: u8 = 0;
const WIRE_LENGTH_DELIMITED: u8 = 2;
const MAX_VARINT_BYTES: usize = 10;

/// A registry entry's status. A live entry is active or suspended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DelegateStatus {
    /// The delegate can ballot. Wire code 0.
    Active,
    /// Suspended by coordinators: the delegate cannot ballot or retire. Wire
    /// code 1.
    Suspended,
    /// Retired by its own key or by coordinators, for good. Wire code 2.
    Retired,
}

impl DelegateStatus {
    /// The status's wire code.
    pub const fn code(self) -> u32 {
        match self {
            Self::Active => 0,
            Self::Suspended => 1,
            Self::Retired => 2,
        }
    }

    /// The status for a wire code, or `None` for a code above 2.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Active),
            1 => Some(Self::Suspended),
            2 => Some(Self::Retired),
            _ => None,
        }
    }
}

/// A delegate registry entry as the chain stores it (`ProxyDelegateEntry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyDelegateEntry {
    /// The entry's index `d`, at least 1.
    pub index: u32,
    /// The entry's one key, for life.
    pub dk: DelegateKey,
    /// The account's provider.
    pub provider: IdentityProvider,
    /// The account's canonical handle (contracts-v1 §2.5).
    pub handle: String,
    /// The canonical id of the post or gist that carries the marker.
    pub proof_id: String,
    /// Whether the entry is active, suspended or retired.
    pub status: DelegateStatus,
    /// The coordinators' reason code for a suspension or retirement, else 0.
    pub status_reason: u32,
    /// The height the entry was registered at.
    pub registered_height: u64,
}

/// Bytes that are not the chain's encoding of a [`ProxyDelegateEntry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryDecodeError {
    /// The bytes end inside a field.
    Truncated,
    /// A varint is longer than ten bytes or overflows 64 bits.
    MalformedVarint,
    /// A field number the message does not define.
    UnknownField {
        /// The undefined field number.
        field_number: u64,
    },
    /// A defined field carries the wrong protobuf wire type.
    WrongWireType {
        /// The field, such as `dk`.
        field: &'static str,
        /// The wire type found.
        wire_type: u8,
    },
    /// A `uint32` field holds a value above `u32::MAX`.
    IntegerOverflow {
        /// The field, such as `index`.
        field: &'static str,
    },
    /// A field holds a value no registration could have written: index 0, a
    /// key that is not 32 bytes or fails the Ed25519 key rule, an unknown
    /// provider or status, or a handle or proof id that is not canonical for
    /// the provider.
    InvalidValue {
        /// The field, such as `handle`.
        field: &'static str,
    },
    /// The bytes decode but are not the chain's deterministic encoding.
    NonCanonical,
}

impl fmt::Display for EntryDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("registry entry bytes end inside a field"),
            Self::MalformedVarint => f.write_str("registry entry has a malformed varint"),
            Self::UnknownField { field_number } => {
                write!(f, "registry entry has no field {field_number}")
            }
            Self::WrongWireType { field, wire_type } => {
                write!(f, "registry entry field {field} has wire type {wire_type}")
            }
            Self::IntegerOverflow { field } => {
                write!(f, "registry entry field {field} does not fit a u32")
            }
            Self::InvalidValue { field } => {
                write!(f, "registry entry field {field} holds an invalid value")
            }
            Self::NonCanonical => {
                f.write_str("registry entry bytes are not the chain's deterministic encoding")
            }
        }
    }
}

impl std::error::Error for EntryDecodeError {}

impl ProxyDelegateEntry {
    /// Checks the field rules the types do not already hold: an index of at
    /// least 1, and a handle and proof id canonical for the provider.
    pub fn check_fields(&self) -> Result<(), EntryDecodeError> {
        let invalid = |field| Err(EntryDecodeError::InvalidValue { field });
        if self.index == 0 {
            return invalid("index");
        }
        if !is_canonical_handle(self.provider, &self.handle) {
            return invalid("handle");
        }
        if !is_canonical_proof_id(self.provider, &self.proof_id) {
            return invalid("proof_id");
        }
        Ok(())
    }

    /// Decodes a registry entry's value bytes. Fails unless `bytes` equal
    /// [`Self::encode`] of the result and pass [`Self::check_fields`].
    pub fn decode(bytes: &[u8]) -> Result<Self, EntryDecodeError> {
        let mut reader = Reader::new(bytes);
        let mut index = 0;
        let mut dk: &[u8] = &[];
        let mut provider = 0;
        let mut handle: &[u8] = &[];
        let mut proof_id: &[u8] = &[];
        let mut status = 0;
        let mut status_reason = 0;
        let mut registered_height = 0;
        while !reader.is_empty() {
            let (field_number, wire_type) = reader.field_key()?;
            match field_number {
                1 => index = reader.uint32(wire_type, "index")?,
                2 => dk = reader.length_delimited(wire_type, "dk")?,
                3 => provider = reader.uint32(wire_type, "provider")?,
                4 => handle = reader.length_delimited(wire_type, "handle")?,
                5 => proof_id = reader.length_delimited(wire_type, "proof_id")?,
                6 => status = reader.uint32(wire_type, "status")?,
                7 => status_reason = reader.uint32(wire_type, "status_reason")?,
                8 => registered_height = reader.varint_of(wire_type, "registered_height")?,
                _ => return Err(EntryDecodeError::UnknownField { field_number }),
            }
        }
        let invalid = |field| EntryDecodeError::InvalidValue { field };
        let text =
            |bytes: &[u8], field| String::from_utf8(bytes.to_vec()).map_err(|_| invalid(field));
        let entry = Self {
            index,
            dk: <[u8; 32]>::try_from(dk)
                .ok()
                .and_then(|dk| DelegateKey::from_bytes(dk).ok())
                .ok_or(invalid("dk"))?,
            provider: IdentityProvider::from_code(provider).ok_or(invalid("provider"))?,
            handle: text(handle, "handle")?,
            proof_id: text(proof_id, "proof_id")?,
            status: DelegateStatus::from_code(status).ok_or(invalid("status"))?,
            status_reason,
            registered_height,
        };
        entry.check_fields()?;
        if entry.encode() != bytes {
            return Err(EntryDecodeError::NonCanonical);
        }
        Ok(entry)
    }

    /// The chain's deterministic encoding (see the module doc).
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        put_varint_field(&mut bytes, 1, u64::from(self.index));
        put_bytes_field(&mut bytes, 2, &self.dk.to_bytes());
        put_varint_field(&mut bytes, 3, u64::from(self.provider.code()));
        put_bytes_field(&mut bytes, 4, self.handle.as_bytes());
        put_bytes_field(&mut bytes, 5, self.proof_id.as_bytes());
        put_varint_field(&mut bytes, 6, u64::from(self.status.code()));
        put_varint_field(&mut bytes, 7, u64::from(self.status_reason));
        put_varint_field(&mut bytes, 8, self.registered_height);
        bytes
    }
}

/// Appends a varint field unless `value` is zero, as proto3 omits it.
fn put_varint_field(bytes: &mut Vec<u8>, field_number: u8, value: u64) {
    if value != 0 {
        put_varint(bytes, u64::from((field_number << 3) | WIRE_VARINT));
        put_varint(bytes, value);
    }
}

/// Appends a length-delimited field. Never omitted, since every caller's
/// value is non-empty.
fn put_bytes_field(bytes: &mut Vec<u8>, field_number: u8, value: &[u8]) {
    put_varint(
        bytes,
        u64::from((field_number << 3) | WIRE_LENGTH_DELIMITED),
    );
    put_varint(bytes, value.len() as u64);
    bytes.extend_from_slice(value);
}

/// Appends the minimal varint encoding of `value`.
fn put_varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

/// A cursor over protobuf bytes.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Whether every byte has been read.
    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Reads a field key and splits it into field number and wire type.
    fn field_key(&mut self) -> Result<(u64, u8), EntryDecodeError> {
        let key = self.varint()?;
        Ok((key >> 3, (key & 0x7) as u8))
    }

    /// Reads a varint of at most ten bytes that fits 64 bits.
    fn varint(&mut self) -> Result<u64, EntryDecodeError> {
        let mut value = 0u64;
        for (position, &byte) in self.bytes.iter().enumerate().take(MAX_VARINT_BYTES) {
            // The tenth byte holds bit 63 alone, and must end the varint.
            if position == MAX_VARINT_BYTES - 1 && byte > 1 {
                return Err(EntryDecodeError::MalformedVarint);
            }
            value |= u64::from(byte & 0x7f) << (7 * position);
            if byte < 0x80 {
                self.bytes = &self.bytes[position + 1..];
                return Ok(value);
            }
        }
        Err(EntryDecodeError::Truncated)
    }

    /// Reads the varint value of `field`, which must have the varint wire type.
    fn varint_of(&mut self, wire_type: u8, field: &'static str) -> Result<u64, EntryDecodeError> {
        if wire_type != WIRE_VARINT {
            return Err(EntryDecodeError::WrongWireType { field, wire_type });
        }
        self.varint()
    }

    /// Reads the value of the `uint32` field `field`.
    fn uint32(&mut self, wire_type: u8, field: &'static str) -> Result<u32, EntryDecodeError> {
        u32::try_from(self.varint_of(wire_type, field)?)
            .map_err(|_| EntryDecodeError::IntegerOverflow { field })
    }

    /// Reads the payload of the length-delimited field `field`.
    fn length_delimited(
        &mut self,
        wire_type: u8,
        field: &'static str,
    ) -> Result<&'a [u8], EntryDecodeError> {
        if wire_type != WIRE_LENGTH_DELIMITED {
            return Err(EntryDecodeError::WrongWireType { field, wire_type });
        }
        let length = self.varint()?;
        let length = usize::try_from(length)
            .ok()
            .filter(|length| *length <= self.bytes.len())
            .ok_or(EntryDecodeError::Truncated)?;
        let (payload, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        Ok(payload)
    }
}

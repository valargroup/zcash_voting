//! The field encoding every proxy delegation digest shares (contract §0).
//!
//! A preimage is a raw ASCII domain (no length prefix, no NUL), then
//! `lp8(chain_id)`, then fixed-width fields, hashed with unkeyed BLAKE2b-256.
//! The helpers match vote-sdk's `x/vote/types/sighash.go`, except that
//! [`DigestPreimage::write32`] rejects a wrong length instead of padding.

use std::fmt;

/// A digest preimage could not be encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestError {
    /// A `write32` field was not exactly 32 bytes long.
    FieldLength {
        /// Length of the rejected field.
        actual: usize,
    },
    /// An `lp8` value, such as the chain id, is longer than 255 bytes.
    LengthPrefixOverflow {
        /// Length of the rejected value.
        actual: usize,
    },
    /// A list has more entries than its `writeU32As32` count can hold.
    CountOverflow {
        /// Number of entries in the rejected list.
        actual: usize,
    },
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldLength { actual } => {
                write!(f, "digest field must be exactly 32 bytes, got {actual}")
            }
            Self::LengthPrefixOverflow { actual } => {
                write!(
                    f,
                    "length-prefixed value must be at most 255 bytes, got {actual}"
                )
            }
            Self::CountOverflow { actual } => {
                write!(f, "list of {actual} entries does not fit a u32 count")
            }
        }
    }
}

impl std::error::Error for DigestError {}

/// The canonical preimage of one digest, built field by field in the order
/// the digest defines (see the module doc for the layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestPreimage {
    bytes: Vec<u8>,
}

impl DigestPreimage {
    /// Starts a preimage with the raw ASCII `domain` followed by `lp8(chain_id)`.
    pub fn new(domain: &str, chain_id: &str) -> Result<Self, DigestError> {
        let mut preimage = Self {
            bytes: Vec::with_capacity(domain.len() + 1 + chain_id.len() + 8 * 32),
        };
        preimage.bytes.extend_from_slice(domain.as_bytes());
        preimage.write_lp8(chain_id.as_bytes())?;
        Ok(preimage)
    }

    /// `write32`: appends `field`, which must be exactly 32 bytes.
    pub fn write32(&mut self, field: &[u8]) -> Result<(), DigestError> {
        if field.len() != 32 {
            return Err(DigestError::FieldLength {
                actual: field.len(),
            });
        }
        self.bytes.extend_from_slice(field);
        Ok(())
    }

    /// `writeU32As32`: `value` little-endian, zero-padded to 32 bytes.
    pub fn write_u32_as_32(&mut self, value: u32) {
        let mut slot = [0u8; 32];
        slot[..4].copy_from_slice(&value.to_le_bytes());
        self.bytes.extend_from_slice(&slot);
    }

    /// `writeU64As32`: `value` little-endian, zero-padded to 32 bytes.
    pub fn write_u64_as_32(&mut self, value: u64) {
        let mut slot = [0u8; 32];
        slot[..8].copy_from_slice(&value.to_le_bytes());
        self.bytes.extend_from_slice(&slot);
    }

    /// `u8`: appends one raw byte.
    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// `lp8`: one length byte, then `value`, which must be at most 255 bytes.
    pub fn write_lp8(&mut self, value: &[u8]) -> Result<(), DigestError> {
        let length = u8::try_from(value.len()).map_err(|_| DigestError::LengthPrefixOverflow {
            actual: value.len(),
        })?;
        self.bytes.push(length);
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    /// Appends a list length with `writeU32As32`, rejecting one above `u32::MAX`.
    pub(crate) fn write_count(&mut self, count: usize) -> Result<(), DigestError> {
        let count =
            u32::try_from(count).map_err(|_| DigestError::CountOverflow { actual: count })?;
        self.write_u32_as_32(count);
        Ok(())
    }

    /// The preimage bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Hashes the preimage with unkeyed BLAKE2b-256.
    pub fn finish(&self) -> [u8; 32] {
        let hash = blake2b_simd::Params::new()
            .hash_length(32)
            .hash(&self.bytes);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(hash.as_bytes());
        digest
    }
}

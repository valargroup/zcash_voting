//! The delegate key, its `zvdk1` and `zvdsk1` encodings and its fingerprint
//! (contract §11.2).
//!
//! `dk_sk` is 32 random bytes, an RFC 8032 secret seed, and `dk` is its
//! Ed25519 public key. `dk` is shown as bech32m with HRP `zvdk`, and `dk_sk`
//! is exported and imported as bech32m with HRP `zvdsk`. Both are canonical
//! lowercase only. See [`DelegateFingerprint`] for the fingerprint.

use std::fmt;

use bech32::{primitives::decode::CheckedHrpstring, Bech32m, Hrp};
use zeroize::{Zeroize, Zeroizing};

use crate::{validate_ed25519_key, Ed25519KeyError};

/// Human-readable part of a bech32m-encoded delegate key.
pub const DELEGATE_KEY_HRP: &str = "zvdk";
/// Human-readable part of a bech32m-encoded delegate secret seed.
pub const DELEGATE_SECRET_HRP: &str = "zvdsk";

/// The bech32 data alphabet. A character's position is its 5-bit value.
const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// A `zvdk1` or `zvdsk1` string could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelegateKeyDecodeError {
    /// Not a bech32m string, or its checksum fails.
    NotBech32m,
    /// The human-readable part is not the expected `zvdk` or `zvdsk`.
    WrongHrp,
    /// The data does not decode to exactly 32 bytes.
    WrongLength {
        /// Number of bytes the data decodes to.
        actual: usize,
    },
    /// Not the canonical encoding (upper case or nonzero padding bits).
    NonCanonical,
    /// The 32 bytes of a `zvdk1` string fail the Ed25519 key rule.
    InvalidKey(Ed25519KeyError),
}

impl fmt::Display for DelegateKeyDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBech32m => f.write_str("delegate key is not a bech32m string"),
            Self::WrongHrp => f.write_str("delegate key has the wrong prefix"),
            Self::WrongLength { actual } => {
                write!(f, "delegate key must be 32 bytes, got {actual}")
            }
            Self::NonCanonical => f.write_str("delegate key is not canonically encoded"),
            Self::InvalidKey(error) => write!(f, "invalid delegate key: {error}"),
        }
    }
}

impl std::error::Error for DelegateKeyDecodeError {}

/// A delegate key fingerprint could not be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FingerprintParseError {
    /// Not four groups of four characters joined by `-`.
    WrongFormat,
    /// A character is not a lower-case bech32 data character.
    InvalidCharacter,
}

impl fmt::Display for FingerprintParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WrongFormat => "fingerprint must be four groups of four characters",
            Self::InvalidCharacter => "fingerprint has a character outside the bech32 alphabet",
        })
    }
}

impl std::error::Error for FingerprintParseError {}

/// The secret half of a delegate key.
///
/// `Debug` prints only the public key. The secret is zeroed on drop, best
/// effort: ed25519-zebra's copies of its `Copy` key are not wiped.
#[derive(Clone)]
pub struct DelegateSigningKey {
    signing_key: ed25519_zebra::SigningKey,
}

impl DelegateSigningKey {
    /// Wraps `dk_sk`, an RFC 8032 secret seed: 32 bytes from the OS RNG.
    pub fn from_secret_seed(dk_sk: [u8; 32]) -> Self {
        Self {
            signing_key: ed25519_zebra::SigningKey::from(dk_sk),
        }
    }

    /// Imports a `zvdsk1…` string. Only the canonical lowercase encoding of
    /// 32 bytes is accepted.
    pub fn import(text: &str) -> Result<Self, DelegateKeyDecodeError> {
        let dk_sk = Zeroizing::new(decode_canonical(DELEGATE_SECRET_HRP, text)?);
        Ok(Self::from_secret_seed(*dk_sk))
    }

    /// The canonical `zvdsk1…` string of `dk_sk`, the one form a delegate
    /// backs up. Zeroed on drop.
    pub fn export(&self) -> Zeroizing<String> {
        Zeroizing::new(encode_canonical(DELEGATE_SECRET_HRP, self.secret_seed()))
    }

    /// `dk_sk`, the RFC 8032 secret seed.
    pub fn secret_seed(&self) -> &[u8; 32] {
        self.signing_key.as_bytes()
    }

    /// The public delegate key `dk`.
    pub fn delegate_key(&self) -> DelegateKey {
        let verification_key = ed25519_zebra::VerificationKey::from(&self.signing_key);
        // A clamped RFC 8032 scalar is never a multiple of the group order, so
        // `[a]B` always passes the key rule.
        DelegateKey(verification_key.into())
    }

    /// Signs a 32-byte digest (REGISTER, RETIRE or BALLOT) with RFC 8032 Ed25519.
    pub fn sign_digest(&self, digest: &[u8; 32]) -> [u8; 64] {
        self.signing_key.sign(digest).to_bytes()
    }
}

impl fmt::Debug for DelegateSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelegateSigningKey")
            .field("dk", &self.delegate_key())
            .finish_non_exhaustive()
    }
}

impl Drop for DelegateSigningKey {
    fn drop(&mut self) {
        self.signing_key.zeroize();
    }
}

/// A delegate public key that passes the Ed25519 key rule.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DelegateKey([u8; 32]);

impl DelegateKey {
    /// Wraps `dk` after checking the Ed25519 key rule.
    pub fn from_bytes(dk: [u8; 32]) -> Result<Self, Ed25519KeyError> {
        validate_ed25519_key(&dk)?;
        Ok(Self(dk))
    }

    /// The 32-byte key as the chain stores it.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    /// The canonical `zvdk1…` string: lower-case bech32m.
    pub fn encode(&self) -> String {
        encode_canonical(DELEGATE_KEY_HRP, &self.0)
    }

    /// Decodes a `zvdk1…` string. Only the canonical encoding is accepted, so
    /// the case-sensitive marker comparison stays exact.
    pub fn decode(text: &str) -> Result<Self, DelegateKeyDecodeError> {
        let dk = decode_key_string(text)?;
        Self::from_bytes(dk).map_err(DelegateKeyDecodeError::InvalidKey)
    }

    /// The key's [`DelegateFingerprint`].
    pub fn fingerprint(&self) -> DelegateFingerprint {
        let mut prefix = [0u8; 10];
        prefix.copy_from_slice(&self.0[..10]);
        DelegateFingerprint(prefix)
    }
}

impl fmt::Display for DelegateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl fmt::Debug for DelegateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DelegateKey({})", self.encode())
    }
}

/// The first 80 bits of a delegate key, shown as the first 16 data characters
/// of its `zvdk1` string grouped as `xxxx-xxxx-xxxx-xxxx` (contract §11.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DelegateFingerprint([u8; 10]);

impl DelegateFingerprint {
    /// Parses the grouped form, which must be lower case.
    pub fn parse(text: &str) -> Result<Self, FingerprintParseError> {
        let groups: Vec<&str> = text.split('-').collect();
        if groups.len() != 4 || groups.iter().any(|group| group.len() != 4) {
            return Err(FingerprintParseError::WrongFormat);
        }
        let mut bits = [0u8; 10];
        for (position, character) in groups.concat().bytes().enumerate() {
            let value = BECH32_CHARSET
                .iter()
                .position(|&candidate| candidate == character)
                .ok_or(FingerprintParseError::InvalidCharacter)?;
            for bit in 0..5 {
                if (value >> (4 - bit)) & 1 == 1 {
                    let offset = position * 5 + bit;
                    bits[offset / 8] |= 0x80 >> (offset % 8);
                }
            }
        }
        Ok(Self(bits))
    }

    /// The canonical display with the entry index, such as
    /// `#17 · qqqs-yqcy-q5rq-wzqf`.
    pub fn display_with_index(&self, delegate_index: u32) -> String {
        format!("#{delegate_index} \u{b7} {self}")
    }
}

impl fmt::Display for DelegateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for position in 0..16 {
            if position > 0 && position % 4 == 0 {
                f.write_str("-")?;
            }
            let value = (0..5).fold(0usize, |value, bit| {
                let offset = position * 5 + bit;
                (value << 1) | usize::from((self.0[offset / 8] >> (7 - offset % 8)) & 1)
            });
            write!(f, "{}", char::from(BECH32_CHARSET[value]))?;
        }
        Ok(())
    }
}

impl fmt::Debug for DelegateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DelegateFingerprint({self})")
    }
}

/// The 32 bytes of a canonical `zvdk1…` string, without the Ed25519 key
/// rule. The wallet list checks a row's key this far (contract §12.3).
pub(crate) fn decode_key_string(text: &str) -> Result<[u8; 32], DelegateKeyDecodeError> {
    decode_canonical(DELEGATE_KEY_HRP, text)
}

/// Lower-case bech32m of 32 bytes under `hrp`.
fn encode_canonical(hrp: &str, bytes: &[u8; 32]) -> String {
    bech32::encode::<Bech32m>(Hrp::parse_unchecked(hrp), bytes)
        .expect("32 bytes under a short HRP is far below the bech32m length limit")
}

/// The 32 bytes of a bech32m string under `hrp`, accepting only the text
/// [`encode_canonical`] writes.
fn decode_canonical(hrp: &str, text: &str) -> Result<[u8; 32], DelegateKeyDecodeError> {
    let checked =
        CheckedHrpstring::new::<Bech32m>(text).map_err(|_| DelegateKeyDecodeError::NotBech32m)?;
    if checked.hrp() != Hrp::parse_unchecked(hrp) {
        return Err(DelegateKeyDecodeError::WrongHrp);
    }
    let decoded = Zeroizing::new(checked.byte_iter().collect::<Vec<u8>>());
    let bytes: [u8; 32] =
        decoded
            .as_slice()
            .try_into()
            .map_err(|_| DelegateKeyDecodeError::WrongLength {
                actual: decoded.len(),
            })?;
    if *Zeroizing::new(encode_canonical(hrp, &bytes)) != text {
        return Err(DelegateKeyDecodeError::NonCanonical);
    }
    Ok(bytes)
}

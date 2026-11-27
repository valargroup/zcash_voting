//! The Ed25519 key rule and ZIP-215 signature verification (contract §0).
//!
//! Delegate and verifier keys must pass [`validate_ed25519_key`], which the
//! chain enforces at registration and in the params payload. Signatures use
//! [`verify_zip215`], CometBFT's rule, so the chain and every client accept the
//! same signatures. ZIP-215 alone accepts small-order and mixed-order keys, so
//! the key rule is a separate, earlier check.

use std::fmt;

use curve25519_dalek::{edwards::CompressedEdwardsY, scalar::Scalar};

/// A public key failed the Ed25519 key rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ed25519KeyError {
    /// The bytes encode no point on the curve.
    NotOnCurve,
    /// The bytes decode to a point, but are not its canonical encoding: `y` is
    /// at least the field prime, or `x = 0` with the sign bit set.
    NonCanonical,
    /// The point has order dividing 8, including the identity.
    SmallOrder,
    /// The point has a nonzero torsion component: `[ℓ]A ≠ O`.
    NotTorsionFree,
}

impl fmt::Display for Ed25519KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotOnCurve => "Ed25519 key is not a curve point",
            Self::NonCanonical => "Ed25519 key is not canonically encoded",
            Self::SmallOrder => "Ed25519 key has small order",
            Self::NotTorsionFree => "Ed25519 key has a torsion component",
        })
    }
}

impl std::error::Error for Ed25519KeyError {}

/// A signature failed ZIP-215 verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// The public key encodes no point on the curve.
    PublicKeyNotOnCurve,
    /// The signature's `s` half is not less than the group order `ℓ`.
    ScalarNotCanonical,
    /// The signature's `R` half encodes no point on the curve.
    CommitmentNotOnCurve,
    /// The cofactored equation `[8][s]B = [8]R + [8][k]A` does not hold.
    EquationMismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PublicKeyNotOnCurve => "signature public key is not a curve point",
            Self::ScalarNotCanonical => "signature scalar is not canonical",
            Self::CommitmentNotOnCurve => "signature R is not a curve point",
            Self::EquationMismatch => "signature does not verify",
        })
    }
}

impl std::error::Error for SignatureError {}

/// Checks the Ed25519 key rule: `key` canonically encodes a point that is not
/// of small order and is torsion-free. Returns the first failure in
/// [`Ed25519KeyError`]'s variant order.
pub fn validate_ed25519_key(key: &[u8; 32]) -> Result<(), Ed25519KeyError> {
    let point = CompressedEdwardsY(*key)
        .decompress()
        .ok_or(Ed25519KeyError::NotOnCurve)?;
    // Decompression accepts every encoding of a point. Only one round-trips.
    if point.compress().to_bytes() != *key {
        return Err(Ed25519KeyError::NonCanonical);
    }
    if point.is_small_order() {
        return Err(Ed25519KeyError::SmallOrder);
    }
    if !point.is_torsion_free() {
        return Err(Ed25519KeyError::NotTorsionFree);
    }
    Ok(())
}

/// Verifies an Ed25519 `signature` on `message` under `public_key` with the
/// ZIP-215 rules. Non-canonical `A` and `R` are accepted and hashed as given,
/// `s` must be canonical, and the equation is cofactored. The key rule is not
/// applied. Call [`validate_ed25519_key`] first where it must hold.
pub fn verify_zip215(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), SignatureError> {
    let verification_key = ed25519_zebra::VerificationKey::try_from(*public_key)
        .map_err(|_| SignatureError::PublicKeyNotOnCurve)?;
    // ed25519-zebra has one error for every malformed signature, so these two
    // checks run first to name the failure.
    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);
    if Option::<Scalar>::from(Scalar::from_canonical_bytes(s_bytes)).is_none() {
        return Err(SignatureError::ScalarNotCanonical);
    }
    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    if CompressedEdwardsY(r_bytes).decompress().is_none() {
        return Err(SignatureError::CommitmentNotOnCurve);
    }
    verification_key
        .verify(&ed25519_zebra::Signature::from_bytes(signature), message)
        .map_err(|_| SignatureError::EquationMismatch)
}

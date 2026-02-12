// Copyright (c) zkMove Authors
// SPDX-License-Identifier: Apache-2.0

//! Poseidon2 hash function implementation for the Pallas field.
//!
//! Native (off-circuit) Poseidon2 hasher used for IMT Merkle tree hashing.
//!
//! # Parameters
//!
//! - Width `t = 3`, Rate = 2, Capacity = 1
//! - Full rounds `R_F = 8` (4 beginning + 4 ending)
//! - Partial rounds `R_P = 56`
//! - S-box degree `d = 5` (i.e., `x^5`)
//! - External matrix: `circ(2, 1, 1)`
//! - Internal matrix: `[[2,1,1],[1,2,1],[1,1,3]]`
//!
//! # References
//!
//! - Poseidon2 paper: <https://eprint.iacr.org/2023/323>
//! - Reference implementation: <https://github.com/amit0365/poseidon2>
//! - Round constants generated via Grain LFSR (see `poseidon2_params.rs`).

use super::poseidon2_params::{MAT_INTERNAL_DIAG_M_1, ROUND_CONSTANTS};
use ff::PrimeField;

// ---- Poseidon2 constants ----

/// State width.
pub const T: usize = 3;
/// Absorption rate.
pub const RATE: usize = 2;
/// Full rounds.
pub const R_F: usize = 8;
/// Partial rounds.
pub const R_P: usize = 56;
/// Total rounds.
pub const ROUNDS: usize = R_F + R_P;

// ---- Hex-to-field parsing ----

/// Parse a `0x`-prefixed big-endian hex string into a `PrimeField` element.
///
/// The hex string is converted to a little-endian byte representation suitable
/// for `PrimeField::from_repr`.
pub fn from_hex<F: PrimeField>(s: &str) -> F {
    let s = s.strip_prefix("0x").unwrap_or(s);
    assert!(s.len() <= 64, "hex string too long");

    // Parse hex pairs into big-endian bytes
    let padded = format!("{:0>64}", s);
    let be_bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&padded[2 * i..2 * i + 2], 16).expect("invalid hex digit"))
        .collect();

    // Write as little-endian into PrimeField::Repr
    let mut repr = F::Repr::default();
    let le_bytes = repr.as_mut();
    for (i, &b) in be_bytes.iter().rev().enumerate() {
        if i < le_bytes.len() {
            le_bytes[i] = b;
        }
    }

    Option::from(F::from_repr(repr)).expect("hex value is not a valid field element")
}

// ---- Parsed parameter cache ----

/// Parsed Poseidon2 parameters (round constants + internal matrix diagonal).
#[derive(Clone, Debug)]
pub struct Poseidon2Params<F: PrimeField> {
    /// Round constants for all 64 rounds.
    pub round_constants: [[F; T]; ROUNDS],
    /// Internal matrix diagonal minus identity.
    pub mat_internal_diag_m_1: [F; T],
}

impl<F: PrimeField> Poseidon2Params<F> {
    /// Parse the hex-encoded constants into field elements.
    pub fn new() -> Self {
        let mut round_constants = [[F::ZERO; T]; ROUNDS];
        for (i, rc_hex) in ROUND_CONSTANTS.iter().enumerate() {
            for (j, hex_str) in rc_hex.iter().enumerate() {
                round_constants[i][j] = from_hex(hex_str);
            }
        }

        let mut mat_internal_diag_m_1 = [F::ZERO; T];
        for (i, hex_str) in MAT_INTERNAL_DIAG_M_1.iter().enumerate() {
            mat_internal_diag_m_1[i] = from_hex(hex_str);
        }

        Poseidon2Params {
            round_constants,
            mat_internal_diag_m_1,
        }
    }
}

// ---- Permutation primitives ----

/// S-box: `x -> x^5`.
#[inline]
pub fn sbox<F: PrimeField>(x: F) -> F {
    let x2 = x.square();
    let x4 = x2.square();
    x4 * x
}

/// Apply S-box to all state elements (full round).
#[inline]
pub fn sbox_full<F: PrimeField>(state: &mut [F; T]) {
    for s in state.iter_mut() {
        *s = sbox(*s);
    }
}

/// Add round constants to the state.
#[inline]
pub fn add_round_constants<F: PrimeField>(state: &mut [F; T], rc: &[F; T]) {
    for i in 0..T {
        state[i] += rc[i];
    }
}

/// External matrix multiply for `t = 3`: `circ(2, 1, 1)`.
///
/// Each output element is `2 * self + sum(all)`, i.e., `self + sum`.
#[inline]
pub fn matmul_external<F: PrimeField>(state: &mut [F; T]) {
    let sum = state[0] + state[1] + state[2];
    state[0] += sum;
    state[1] += sum;
    state[2] += sum;
}

/// Internal matrix multiply for `t = 3` using diagonal-minus-1 form.
///
/// Matrix: `[[2,1,1],[1,2,1],[1,1,3]]`
/// Computes: `output[i] = input[i] * diag_m_1[i] + sum(input)`.
#[inline]
pub fn matmul_internal<F: PrimeField>(state: &mut [F; T], diag_m_1: &[F; T]) {
    let sum = state[0] + state[1] + state[2];
    for i in 0..T {
        state[i] *= diag_m_1[i];
        state[i] += sum;
    }
}

// ---- Core permutation ----

/// Apply the Poseidon2 permutation to a width-3 state.
///
/// Structure:
/// 1. Initial external linear layer
/// 2. First `R_F/2` full rounds: add RC -> S-box (all) -> external MDS
/// 3. `R_P` partial rounds: add RC[0] -> S-box (first) -> internal MDS
/// 4. Last `R_F/2` full rounds: add RC -> S-box (all) -> external MDS
pub(crate) fn poseidon2_permutation<F: PrimeField>(state: &mut [F; T], params: &Poseidon2Params<F>) {
    // Initial external linear layer
    matmul_external(state);

    let rf_half = R_F / 2;

    // First half full rounds
    for r in 0..rf_half {
        add_round_constants(state, &params.round_constants[r]);
        sbox_full(state);
        matmul_external(state);
    }

    // Partial rounds
    for r in rf_half..(rf_half + R_P) {
        state[0] += params.round_constants[r][0];
        state[0] = sbox(state[0]);
        matmul_internal(state, &params.mat_internal_diag_m_1);
    }

    // Second half full rounds
    for r in (rf_half + R_P)..ROUNDS {
        add_round_constants(state, &params.round_constants[r]);
        sbox_full(state);
        matmul_external(state);
    }
}

// ---- Sponge construction ----

/// Poseidon2 sponge hash for constant-length input.
///
/// Uses width = 3, rate = 2. Domain separation: `state[RATE] = L` (input length
/// encoded in the capacity element). Input is absorbed in rate-sized chunks,
/// then the first state element is squeezed as output.
pub(crate) fn poseidon2_hash<F: PrimeField, const L: usize>(inputs: [F; L], params: &Poseidon2Params<F>) -> F {
    // Initialise state with domain separation in capacity
    let mut state = [F::ZERO; T];
    state[RATE] = F::from(L as u64);

    // Absorb input in rate-sized chunks
    let mut i = 0;
    while i < L {
        for j in 0..RATE {
            if i + j < L {
                state[j] += inputs[i + j];
            }
        }
        poseidon2_permutation(&mut state, params);
        i += RATE;
    }

    // Squeeze
    state[0]
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use pasta_curves::Fp;

    /// Known-Answer Test: verify the raw permutation output for input [0, 1, 2]
    /// matches the reference Poseidon2 implementation (amit0365/poseidon2) for
    /// the Pallas field with t=3, R_F=8, R_P=56, d=5.
    #[test]
    fn poseidon2_permutation_kat() {
        let params = Poseidon2Params::<Fp>::new();
        let mut state = [Fp::from(0u64), Fp::from(1u64), Fp::from(2u64)];
        poseidon2_permutation(&mut state, &params);

        let expected_0: Fp =
            from_hex("0x1a9b54c7512a914dd778282c44b3513fea7251420b9d95750baae059b2268d7a");
        let expected_1: Fp =
            from_hex("0x1c48ea0994a7d7984ea338a54dbf0c8681f5af883fe988d59ba3380c9f7901fc");
        let expected_2: Fp =
            from_hex("0x079ddd0a80a3e9414489b526a2770448964766685f4c4842c838f8a23120b401");

        assert_eq!(state[0], expected_0, "perm[0] mismatch");
        assert_eq!(state[1], expected_1, "perm[1] mismatch");
        assert_eq!(state[2], expected_2, "perm[2] mismatch");
    }

    /// Determinism: same input always produces same output.
    #[test]
    fn poseidon2_hash_deterministic() {
        let params = Poseidon2Params::<Fp>::new();
        let a = Fp::from(6u64);
        let b = Fp::from(42u64);

        let h1 = poseidon2_hash([a, b], &params);
        let h2 = poseidon2_hash([a, b], &params);
        assert_eq!(h1, h2, "hash should be deterministic");
    }

    /// Non-zero: hash output is not the zero element.
    #[test]
    fn poseidon2_hash_nonzero() {
        let params = Poseidon2Params::<Fp>::new();
        let a = Fp::from(6u64);
        let b = Fp::from(42u64);
        let h = poseidon2_hash([a, b], &params);
        assert_ne!(h, Fp::from(0u64), "hash should be non-zero");
    }

    /// Collision resistance: different inputs produce different outputs.
    #[test]
    fn poseidon2_hash_collision() {
        let params = Poseidon2Params::<Fp>::new();
        let h1 = poseidon2_hash([Fp::from(6u64), Fp::from(42u64)], &params);
        let h2 = poseidon2_hash([Fp::from(7u64), Fp::from(42u64)], &params);
        let h3 = poseidon2_hash([Fp::from(6u64), Fp::from(43u64)], &params);
        assert_ne!(h1, h2, "different first input should produce different hash");
        assert_ne!(h1, h3, "different second input should produce different hash");
    }

    /// Round constant count: verify we have exactly 64 rounds of constants.
    #[test]
    fn poseidon2_round_constant_count() {
        assert_eq!(ROUND_CONSTANTS.len(), ROUNDS);
        assert_eq!(ROUNDS, 64);
    }

    /// Partial round constants: verify that partial rounds have zeros in
    /// positions [1] and [2].
    #[test]
    fn poseidon2_partial_round_zeros() {
        let rf_half = R_F / 2; // 4
        for r in rf_half..(rf_half + R_P) {
            assert_eq!(
                ROUND_CONSTANTS[r][1], "0x0000000000000000000000000000000000000000000000000000000000000000",
                "partial round {} should have zero at index 1",
                r
            );
            assert_eq!(
                ROUND_CONSTANTS[r][2], "0x0000000000000000000000000000000000000000000000000000000000000000",
                "partial round {} should have zero at index 2",
                r
            );
        }
    }
}

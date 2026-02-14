//! The Vote Proof circuit implementation.
//!
//! Boilerplate only: column configuration and Circuit/Instance types.
//! No constraints or key logic yet — to be filled from the ZKP #2 spec.

use alloc::vec::Vec;

use halo2_proofs::{
    circuit::{floor_planner, Layouter, Value},
    plonk::{self, Advice, Column, ConstraintSystem, Instance as InstanceColumn},
};
use pasta_curves::{pallas, vesta};

/// Circuit size (2^K rows). Increase when constraints are added.
pub const K: u32 = 4;

// ================================================================
// Config
// ================================================================

/// Configuration for the Vote Proof circuit.
///
/// Holds the instance column (public inputs) and advice columns
/// (private witness). No gates are configured yet.
#[derive(Clone, Debug)]
pub struct Config {
    /// Public input column.
    primary: Column<InstanceColumn>,
    /// Advice columns for private witness data.
    advices: [Column<Advice>; 2],
}

// ================================================================
// Circuit
// ================================================================

/// The Vote Proof circuit.
///
/// Witness and constraint logic will be added here per ZKP #2.
#[derive(Clone, Debug, Default)]
pub struct Circuit {
    /// Placeholder for future private witness (e.g. vote choice, randomness).
    #[allow(dead_code)]
    dummy_witness: Value<pallas::Base>,
}

impl Circuit {
    /// Builds a circuit with the given witness (for prover use).
    /// For now only supports empty/unknown witness.
    pub fn new(dummy_witness: Value<pallas::Base>) -> Self {
        Circuit {
            dummy_witness,
        }
    }
}

impl plonk::Circuit<pallas::Base> for Circuit {
    type Config = Config;
    type FloorPlanner = floor_planner::V1;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut ConstraintSystem<pallas::Base>) -> Self::Config {
        let primary = meta.instance_column();
        meta.enable_equality(primary);

        let advices = [
            meta.advice_column(),
            meta.advice_column(),
        ];
        for col in &advices {
            meta.enable_equality(*col);
        }

        Config { primary, advices }
    }

    fn synthesize(
        &self,
        _config: Self::Config,
        _layouter: impl Layouter<pallas::Base>,
    ) -> Result<(), plonk::Error> {
        // No constraints or assignments yet.
        Ok(())
    }
}

// ================================================================
// Instance (public inputs)
// ================================================================

/// Public inputs to the Vote Proof circuit.
///
/// Structure will be extended when ZKP #2 public inputs are defined
/// in the spec (e.g. vote commitment, nullifier, round id).
#[derive(Clone, Debug)]
pub struct Instance {
    /// Placeholder for future public inputs (same type as halo2 instance column).
    #[allow(dead_code)]
    dummy_public: vesta::Scalar,
}

impl Instance {
    /// Constructs an [`Instance`] from its constituent parts.
    pub fn from_parts(dummy_public: vesta::Scalar) -> Self {
        Instance {
            dummy_public,
        }
    }

    /// Serializes public inputs for halo2 proof creation/verification.
    /// Order must match the instance column layout in the circuit.
    pub fn to_halo2_instance(&self) -> Vec<vesta::Scalar> {
        alloc::vec![self.dummy_public]
    }
}

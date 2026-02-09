//! The Delegation circuit implementation.
//!
//! Proves nullifier integrity, spend authority, and diversified address integrity
//! for a single note:
//! - Given private witness data `(nk, rho_old, psi_old, cm_old)`, derives
//!   `nf_old` in-circuit and constrains it to match the public input.
//! - Given private witness data `(ak, alpha)`, derives `rk = [alpha] * SpendAuthG + ak`
//!   and constrains it to match the public input. This links the ZKP to the
//!   keystone signature verified out-of-circuit.
//! - Given private witness data `(ak, nk, rivk, g_d_signed, pk_d_signed)`, derives
//!   `ivk = CommitIvk_rivk(ExtractP(ak), nk)` and constrains `pk_d_signed = [ivk] * g_d_signed`.
//!   This proves the note's address belongs to the same key material.
//!
//! Follows the 1-circuit-per-note pattern from the vote module. For multiple
//! notes, the builder layer creates multiple independent proofs.

use group::{Curve, GroupEncoding};
use halo2_proofs::{
    circuit::{floor_planner, Layouter, Value},
    plonk::{self, Advice, Column, Instance as InstanceColumn},
};
use pasta_curves::{arithmetic::CurveAffine, pallas, vesta};

use crate::{
    circuit::{
        commit_ivk::{CommitIvkChip, CommitIvkConfig},
        gadget::{
            add_chip::{AddChip, AddConfig},
            assign_free_advice, commit_ivk, derive_nullifier,
        },
    },
    constants::{OrchardCommitDomains, OrchardFixedBases, OrchardFixedBasesFull, OrchardHashDomains},
    keys::{
        CommitIvkRandomness, DiversifiedTransmissionKey, FullViewingKey, NullifierDerivingKey,
        Scope, SpendValidatingKey,
    },
    note::{
        commitment::NoteCommitment,
        nullifier::Nullifier,
        Note,
    },
    primitives::redpallas::{SpendAuth, VerificationKey},
    spec::NonIdentityPallasPoint,
};
use halo2_gadgets::{
    ecc::{
        chip::{EccChip, EccConfig},
        FixedPoint, NonIdentityPoint, Point, ScalarFixed, ScalarVar,
    },
    poseidon::{primitives as poseidon, Pow5Chip as PoseidonChip, Pow5Config as PoseidonConfig},
    sinsemilla::chip::{SinsemillaChip, SinsemillaConfig},
    utilities::lookup_range_check::LookupRangeCheckConfig,
};

/// Public input offset for the derived nullifier.
const NF_OLD: usize = 0;
/// Public input offset for rk (x-coordinate).
const RK_X: usize = 1;
/// Public input offset for rk (y-coordinate).
const RK_Y: usize = 2;

/// Size of the delegation circuit.
/// 4096 rows
/// Sinsemilla lookup table ~1024 rows
/// ECC chip ~1000 rows
/// Poseidon chip ~200-300 rows
/// Range checks a few more rows but relatively negligible.
const K: u32 = 12;

/// Configuration for the Delegation circuit.
#[derive(Clone, Debug)]
pub struct Config {
    // The instance column (public inputs)
    primary: Column<InstanceColumn>,
    // 10 advice columns for private witness data.
    // This is the scratch space where the prover places intermediate values during computation.
    // Various chips use these columns
    // Poseidon: [5..9]
    // ECC: uses all 10
    // AddChip: uses [6..9]
    advices: [Column<Advice>; 10],
    // Configuration for the AddChip which constrains a + b = c over field elements.
    // Used inside DeriveNullifier to combine intermediate values.
    add_config: AddConfig,
    // Configuration for the ECCChip which provides elliptic curve operations
    // (point addition, scalar multiplication) on the Pallas curve with Orchard's fixes bases.
    // We use it to convert cm_old from NoteCommitment to a Field point for the DeriveNullifier function.
    ecc_config: EccConfig<OrchardFixedBases>,
    // Poseidon chip config. Used in the DeriveNullifier.
    poseidon_config: PoseidonConfig<pallas::Base, 3, 2>,
    // Sinsemilla config — used for loading the lookup table that
    // LookupRangeCheckConfig (and thus EccChip) depends on, and for CommitIvk.
    sinsemilla_config: SinsemillaConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    // Configuration to handle decomposition and canonicity checking for CommitIvk.
    commit_ivk_config: CommitIvkConfig,
}

impl Config {
    fn add_chip(&self) -> AddChip {
        AddChip::construct(self.add_config.clone())
    }

    fn ecc_chip(&self) -> EccChip<OrchardFixedBases> {
        EccChip::construct(self.ecc_config.clone())
    }

    // Operating over the Pallas base field, with a width of 3 (state size) and rate of 2
    // 3 comes from the P128Pow5T3 construction used throughout Orchard (i.e. 3 is width)
    // Rate of 2 means that two elements are absorbed per permutation, so the hash completes
    // in fewer rounds than rate 1, roughly halving the number of Poseidon permutations.
    fn poseidon_chip(&self) -> PoseidonChip<pallas::Base, 3, 2> {
        PoseidonChip::construct(self.poseidon_config.clone())
    }

    fn commit_ivk_chip(&self) -> CommitIvkChip {
        CommitIvkChip::construct(self.commit_ivk_config.clone())
    }

    fn sinsemilla_chip(
        &self,
    ) -> SinsemillaChip<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases> {
        SinsemillaChip::construct(self.sinsemilla_config.clone())
    }
}

/// The Delegation circuit.
///
/// Proves nullifier integrity, spend authority, and diversified address integrity:
/// - The prover knows `(nk, rho, psi, cm)` such that `nf_old = DeriveNullifier(nk, rho, psi, cm)`.
/// - The prover knows `(ak, alpha)` such that `rk = [alpha] * SpendAuthG + ak`.
/// - The prover knows `(ak, nk, rivk, g_d_signed, pk_d_signed)` such that
///   `pk_d_signed = [CommitIvk_rivk(ExtractP(ak), nk)] * g_d_signed`.
#[derive(Clone, Debug, Default)]
pub struct Circuit {
    nk: Value<NullifierDerivingKey>,
    rho_old: Value<pallas::Base>,
    psi_old: Value<pallas::Base>,
    cm_old: Value<NoteCommitment>,
    ak: Value<SpendValidatingKey>,
    alpha: Value<pallas::Scalar>,
    rivk: Value<CommitIvkRandomness>,
    g_d_signed: Value<NonIdentityPallasPoint>,
    pk_d_signed: Value<DiversifiedTransmissionKey>,
}

impl Circuit {
    /// Constructs a `Circuit` from a note, its full viewing key, and the spend auth randomizer.
    pub fn from_note_unchecked(fvk: &FullViewingKey, note: &Note, alpha: pallas::Scalar) -> Self {
        let sender_address = note.recipient();
        let rho_old = note.rho();
        let psi_old = note.rseed().psi(&rho_old);
        Circuit {
            nk: Value::known(*fvk.nk()),
            rho_old: Value::known(rho_old.0),
            psi_old: Value::known(psi_old),
            cm_old: Value::known(note.commitment()),
            ak: Value::known(fvk.clone().into()),
            alpha: Value::known(alpha),
            rivk: Value::known(fvk.rivk(Scope::External)),
            g_d_signed: Value::known(sender_address.g_d()),
            pk_d_signed: Value::known(*sender_address.pk_d()),
        }
    }
}

impl plonk::Circuit<pallas::Base> for Circuit {
    type Config = Config;
    type FloorPlanner = floor_planner::V1;

    fn without_witnesses(&self) -> Self {
        Self::default()
    }

    fn configure(meta: &mut plonk::ConstraintSystem<pallas::Base>) -> Self::Config {
        // Advice columns used in the circuit.
        let advices = [
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
            meta.advice_column(),
        ];

        // Addition of two field elements.
        // 7,8,6 are chosen to share columns with the Poseidon chip to minimize column usage.
        let add_config = AddChip::configure(meta, advices[7], advices[8], advices[6]);

        // Fixed columns for the Sinsemilla generator lookup table.
        let table_idx = meta.lookup_table_column();
        let lookup = (
            table_idx,
            meta.lookup_table_column(),
            meta.lookup_table_column(),
        );

        // Instance column used for public inputs.
        let primary = meta.instance_column();
        meta.enable_equality(primary);

        // Permutation over all advice columns.
        for advice in advices.iter() {
            meta.enable_equality(*advice);
        }

        // Fixed columns shared between ECC and Poseidon chips.
        let lagrange_coeffs = [
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
            meta.fixed_column(),
        ];
        let rc_a = lagrange_coeffs[2..5].try_into().unwrap();
        let rc_b = lagrange_coeffs[5..8].try_into().unwrap();

        // Use the first Lagrange coefficient column for loading global constants.
        meta.enable_constant(lagrange_coeffs[0]);

        // Range check configuration using the right-most advice column.
        let range_check = LookupRangeCheckConfig::configure(meta, advices[9], table_idx);

        // Configuration for curve point operations.
        let ecc_config =
            EccChip::<OrchardFixedBases>::configure(meta, advices, lagrange_coeffs, range_check);

        // Configuration for the Poseidon hash.
        let poseidon_config = PoseidonChip::configure::<poseidon::P128Pow5T3>(
            meta,
            advices[6..9].try_into().unwrap(),
            advices[5],
            rc_a,
            rc_b,
        );

        // Sinsemilla config — used for loading the lookup table that the range check
        // (and thus ECC operations) depend on, and for CommitIvk.
        let sinsemilla_config = SinsemillaChip::configure(
            meta,
            advices[..5].try_into().unwrap(),
            advices[6],
            lagrange_coeffs[0],
            lookup,
            range_check,
        );

        // Configuration to handle decomposition and canonicity checking for CommitIvk.
        let commit_ivk_config = CommitIvkChip::configure(meta, advices);

        Config {
            primary,
            advices,
            add_config,
            ecc_config,
            poseidon_config,
            sinsemilla_config,
            commit_ivk_config,
        }
    }

    #[allow(non_snake_case)]
    fn synthesize(
        &self,
        config: Self::Config,
        mut layouter: impl Layouter<pallas::Base>,
    ) -> Result<(), plonk::Error> {
        // Load the Sinsemilla generator lookup table (needed by ECC range checks).
        SinsemillaChip::load(config.sinsemilla_config.clone(), &mut layouter)?;

        // Construct the ECC chip.
        // It is needed to derive cm_old ECC point from NoteCommitment.
        let ecc_chip = config.ecc_chip();

        // Witness ak_P (spend validating key) as a non-identity curve point.
        // Shared between spend authority and CommitIvk.
        // If ak_P were allowed to be the identity point (zero of the curve group), it would be a degenerate
        // key with no cryptographic strength - any signature would trivially verify against it.
        // By constraining, we ensure that the delegated spend authority is backed by a real meaningful
        // public key.
        let ak_P: Value<pallas::Point> = self.ak.as_ref().map(|ak| ak.into());
        let ak_P = NonIdentityPoint::new(
            ecc_chip.clone(),
            layouter.namespace(|| "witness ak_P"),
            ak_P.map(|ak_P| ak_P.to_affine()),
        )?;

        // Witness g_d_signed (diversified generator from the note's address).
        // Shared between diversified address integrity check and (future) note commitment.
        let g_d_signed = NonIdentityPoint::new(
            ecc_chip.clone(),
            layouter.namespace(|| "witness g_d_signed"),
            self.g_d_signed.as_ref().map(|gd| gd.to_affine()),
        )?;

        // Witness nk (nullifier deriving key).
        let nk = assign_free_advice(
            layouter.namespace(|| "witness nk"),
            config.advices[0],
            self.nk.map(|nk| nk.inner()),
        )?;

        // Witness rho_old.
        // This is the nullifier of the note that was spent to create this note. It is
        // a Nullifier type (a Pallas base field element) that serves as a unique, per-note domain
        // separator.
        // rho ensures that even if two notes have identical contents, they will produce
        // different nullifiers because they were created by spending different input notes.
        // rho provides deterministic, structural uniqueness. It is the nullifier of the
        // spend input note so it chains each note to its creation context. A single tx
        // can create multiple output notes from the same input. All those outputs share the same
        // rho. If nullifier derivation only used rho (no psi), outputs from the same input could collide.
        let rho_old = assign_free_advice(
            layouter.namespace(|| "witness rho_old"),
            config.advices[0],
            self.rho_old,
        )?;

        // Witness psi_old.
        // Pseudorandom field element derived from the note's random
        // seed rseed and its nullifier domain separator rho.
        // It adds randomness to the nullifier so that even if two notes share the same
        // rho and nk, they produce different nullifiers.
        // We provide it as input instead of deriving in-circuit since derivation
        // would require an expensive Blake2b.
        // psi provides randomized uniqueness. It is derived from rseed which is
        // freshly random per note. So, even if multiple outputs are derived from the same note,
        // different rseed values produce different psi values. But if uniqueness relied only on psi
        // (i.e. only randomness), a faulty RNG would cause nullifier collisions. Together with rho,
        // they cover each other's weaknesses.
        // Additionally, there is a structural reason, if we only used psi, there would be an implicit chain:
        // each note's identity is linked to the note that was spend to create it. The randomized psi
        // breaks the chain, unblocking a requirement used in Orchard's security proof.
        let psi_old = assign_free_advice(
            layouter.namespace(|| "witness psi_old"),
            config.advices[0],
            self.psi_old,
        )?;

        // Witness cm_old as an ECC point, which is the form DeriveNullifier expects.
        let cm_old = Point::new(
            ecc_chip.clone(),
            layouter.namespace(|| "witness cm_old"),
            self.cm_old.as_ref().map(|cm| cm.inner().to_affine()),
        )?;

        // Nullifier integrity: derive nf_old = DeriveNullifier(nk, rho_old, psi_old, cm_old).
        let nf_old = derive_nullifier(
            layouter.namespace(|| "nf_old = DeriveNullifier_nk(rho_old, psi_old, cm_old)"),
            config.poseidon_chip(),
            config.add_chip(),
            ecc_chip.clone(),
            rho_old,
            &psi_old,
            &cm_old,
            nk.clone(), // clone so nk remains available for commit_ivk
        )?;

        // Constrain nf_old to equal the public input.
        // Enforce that the nullifier computed inside the circuit matches the nullifier provided
        // as a public input from outside the circuit (supplied at NF_OLD of the public input)
        layouter.constrain_instance(nf_old.inner().cell(), config.primary, NF_OLD)?;

        // Spend authority
        // Proves that the public rk is a valid rerandomization of the prover's ak.
        // The out-of-circuit verifier checks that the keystone signature is valid under rk,
        // so this links the ZKP to the signature without revealing ak.
        {
            // Witness alpha (spend auth randomizer) as a full-width fixed scalar.
            let alpha = ScalarFixed::new(
                ecc_chip.clone(),
                layouter.namespace(|| "alpha"),
                self.alpha,
            )?;

            // alpha_commitment = [alpha] SpendAuthG
            let (alpha_commitment, _) = {
                // SpendAuthG is a fixed generator point on the Pallas elliptic curve, used
                // specifically for spend authorization in the Orchard protocol.
                let spend_auth_g = OrchardFixedBasesFull::SpendAuthG;
                let spend_auth_g = FixedPoint::from_inner(ecc_chip.clone(), spend_auth_g);
                spend_auth_g.mul(layouter.namespace(|| "[alpha] SpendAuthG"), alpha)?
            };

            // rk = [alpha] SpendAuthG + ak_P
            let rk = alpha_commitment.add(layouter.namespace(|| "rk"), &ak_P)?;

            // Constrain rk to equal the public inputs (x and y coordinates).
            layouter.constrain_instance(rk.inner().x().cell(), config.primary, RK_X)?;
            layouter.constrain_instance(rk.inner().y().cell(), config.primary, RK_Y)?;
        }

        // Diversified address integrity.
        // ivk = ⊥ or pk_d_signed = [ivk] * g_d_signed
        // where ivk = CommitIvk_rivk(ExtractP(ak_P), nk)
        //
        // The ⊥ case is handled internally by CommitDomain::short_commit:
        // incomplete addition allows ⊥ to occur, and synthesis detects
        // these edge cases and aborts proof creation.
        {
            let ivk = {
                // ExtractP(ak_P) -- extract the x-coordinate from the curve point
                let ak = ak_P.extract_p().inner().clone();
                let rivk = ScalarFixed::new(
                    ecc_chip.clone(),
                    layouter.namespace(|| "rivk"),
                    self.rivk.map(|rivk| rivk.inner()),
                )?;

                // Commit ak and nk with rivk randomness, creating an ivk.
                commit_ivk(
                    config.sinsemilla_chip(),
                    ecc_chip.clone(),
                    config.commit_ivk_chip(),
                    layouter.namespace(|| "CommitIvk"),
                    ak,
                    nk,
                    rivk,
                )?
            };

            // Convert ivk (an x-coordinate) to a variable-base scalar for EC multiplication.
            let ivk = ScalarVar::from_base(
                ecc_chip.clone(),
                layouter.namespace(|| "ivk"),
                ivk.inner(),
            )?;

            // [ivk] g_d_signed - derive the expected pk_d
            let (derived_pk_d_signed, _ivk) =
                g_d_signed.mul(layouter.namespace(|| "[ivk] g_d_signed"), ivk)?;

            // Witness pk_d_signed and constrain it to equal the derived value.
            let pk_d_signed = NonIdentityPoint::new(
                ecc_chip.clone(),
                layouter.namespace(|| "witness pk_d_signed"),
                self.pk_d_signed.map(|pk_d_signed| pk_d_signed.inner().to_affine()),
            )?;
            derived_pk_d_signed
                .constrain_equal(layouter.namespace(|| "pk_d_signed equality"), &pk_d_signed)?;
        }

        Ok(())
    }
}

/// Public inputs to the Delegation circuit.
#[derive(Clone, Debug)]
pub struct Instance {
    /// The derived nullifier (temporary public input; will be replaced by gov_null).
    pub nf_old: Nullifier,
    /// The randomized spend validating key, used for signature verification out-of-circuit.
    pub rk: VerificationKey<SpendAuth>,
}

impl Instance {
    /// Constructs an [`Instance`] from its constituent parts.
    pub fn from_parts(nf_old: Nullifier, rk: VerificationKey<SpendAuth>) -> Self {
        Instance { nf_old, rk }
    }

    /// Returns the public inputs as a vector of field elements for halo2.
    pub fn to_halo2_instance(&self) -> Vec<vesta::Scalar> {
        let rk = pallas::Point::from_bytes(&self.rk.clone().into())
            .unwrap()
            .to_affine()
            .coordinates()
            .unwrap();

        vec![self.nf_old.0, *rk.x(), *rk.y()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        keys::{FullViewingKey, Scope, SpendValidatingKey, SpendingKey},
        note::Note,
    };
    use ff::Field;
    use halo2_proofs::dev::MockProver;
    use rand::rngs::OsRng;

    /// Helper: create a dummy note and its corresponding circuit + expected public inputs.
    fn make_test_note() -> (Circuit, Nullifier, VerificationKey<SpendAuth>) {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);

        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        (circuit, nf, rk)
    }

    #[test]
    fn nullifier_integrity_happy_path() {
        let (circuit, nf, rk) = make_test_note();

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();

        let prover = MockProver::run(
            K,
            &circuit,
            vec![public_inputs],
        )
        .unwrap();

        assert_eq!(prover.verify(), Ok(()));
    }

    #[test]
    fn nullifier_integrity_wrong_key() {
        let mut rng = OsRng;

        // Create note with one key
        let (_sk1, fvk1, note) = Note::dummy(&mut rng, None);
        let alpha = pallas::Scalar::random(&mut rng);
        let circuit = Circuit::from_note_unchecked(&fvk1, &note, alpha);

        // Derive the correct rk from fvk1 (so spend authority passes)
        let ak1: SpendValidatingKey = fvk1.clone().into();
        let rk = ak1.randomize(&alpha);

        // But derive the expected nullifier with a different key
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        let wrong_nf = note.nullifier(&fvk2);

        let instance = Instance::from_parts(wrong_nf, rk);
        let public_inputs = instance.to_halo2_instance();

        let prover = MockProver::run(
            K,
            &circuit,
            vec![public_inputs],
        )
        .unwrap();

        // The proof should fail: the derived nullifier won't match the public input.
        assert!(prover.verify().is_err());
    }

    #[test]
    fn nullifier_integrity_dummy_note() {
        // A dummy note (value = 0) should work identically.
        let (circuit, nf, rk) = make_test_note();

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();

        let prover = MockProver::run(
            K,
            &circuit,
            vec![public_inputs],
        )
        .unwrap();

        assert_eq!(prover.verify(), Ok(()));
    }

    #[test]
    fn spend_authority_wrong_rk() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);

        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();

        // Build the circuit with one alpha
        let alpha_circuit = pallas::Scalar::random(&mut rng);
        let circuit = Circuit::from_note_unchecked(&fvk, &note, alpha_circuit);

        // But compute rk with a different alpha
        let alpha_wrong = pallas::Scalar::random(&mut rng);
        let wrong_rk = ak.randomize(&alpha_wrong);

        let instance = Instance::from_parts(nf, wrong_rk);
        let public_inputs = instance.to_halo2_instance();

        let prover = MockProver::run(
            K,
            &circuit,
            vec![public_inputs],
        )
        .unwrap();

        // The proof should fail: the derived rk won't match the public input.
        assert!(prover.verify().is_err());
    }

    #[test]
    fn address_integrity_happy_path() {
        let (circuit, nf, rk) = make_test_note();

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();

        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert_eq!(prover.verify(), Ok(()));
    }

    #[test]
    fn address_integrity_wrong_rivk() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace rivk with a different key's rivk
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        circuit.rivk = Value::known(fvk2.rivk(Scope::External));

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn address_integrity_wrong_pk_d() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace pk_d with a different key's pk_d
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        let other_address = fvk2.address_at(0u32, Scope::External);
        circuit.pk_d_signed = Value::known(*other_address.pk_d());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }
}

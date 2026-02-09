//! The Delegation circuit implementation.
//!
//! Proves nullifier integrity, spend authority, and diversified address integrity
//! for a single note:
//! - Given private witness data `(nk, rho_signed, psi_signed, cm_signed)`, derives
//!   `nf_signed` in-circuit and constrains it to match the public input.
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
            assign_free_advice, commit_ivk, derive_nullifier, note_commit,
        },
        note_commit::{NoteCommitChip, NoteCommitConfig},
    },
    constants::{OrchardCommitDomains, OrchardFixedBases, OrchardFixedBasesFull, OrchardHashDomains},
    keys::{
        CommitIvkRandomness, DiversifiedTransmissionKey, FullViewingKey, NullifierDerivingKey,
        Scope, SpendValidatingKey,
    },
    note::{
        commitment::{NoteCommitTrapdoor, NoteCommitment},
        nullifier::Nullifier,
        Note,
    },
    primitives::redpallas::{SpendAuth, VerificationKey},
    spec::NonIdentityPallasPoint,
    value::NoteValue,
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
const NF_SIGNED: usize = 0;
/// Public input offset for rk (x-coordinate).
const RK_X: usize = 1;
/// Public input offset for rk (y-coordinate).
const RK_Y: usize = 2;

/// Size of the delegation circuit (2^K rows).
///
/// Current usage fits within K=11 (2048 rows) but K=12 (4096 rows)
/// is chosen to leave headroom for future conditions (Merkle path,
/// SMT non-membership, governance nullifiers).
///
/// Row budget breakdown:
/// - Sinsemilla lookup table: ~1024 rows (loaded once, shared)
/// - NoteCommit (Sinsemilla hash + decomposition/canonicity + rcm scalar mul): ~950-1150 rows
/// - Nullifier derivation (Poseidon + ECC): ~400 rows
/// - Spend authority (fixed-base scalar mul + point add): ~260 rows
/// - CommitIvk (Sinsemilla short commit + decomposition): ~200 rows
/// - Address integrity (variable-base scalar mul): ~260 rows
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
    // We use it to convert cm_signed from NoteCommitment to a Field point for the DeriveNullifier function.
    ecc_config: EccConfig<OrchardFixedBases>,
    // Poseidon chip config. Used in the DeriveNullifier.
    poseidon_config: PoseidonConfig<pallas::Base, 3, 2>,
    // Sinsemilla config — used for loading the lookup table that
    // LookupRangeCheckConfig (and thus EccChip) depends on, and for CommitIvk.
    sinsemilla_config: SinsemillaConfig<OrchardHashDomains, OrchardCommitDomains, OrchardFixedBases>,
    // Configuration to handle decomposition and canonicity checking for CommitIvk.
    commit_ivk_config: CommitIvkConfig,
    // Configuration for decomposition and canonicity checking for NoteCommit.
    note_commit_config: NoteCommitConfig,
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

    fn note_commit_chip(&self) -> NoteCommitChip {
        NoteCommitChip::construct(self.note_commit_config.clone())
    }
}

/// The Delegation circuit.
///
/// Proves nullifier integrity, spend authority, and diversified address integrity:
/// - The prover knows `(nk, rho, psi, cm)` such that `nf_signed = DeriveNullifier(nk, rho, psi, cm)`.
/// - The prover knows `(ak, alpha)` such that `rk = [alpha] * SpendAuthG + ak`.
/// - The prover knows `(ak, nk, rivk, g_d_signed, pk_d_signed)` such that
///   `pk_d_signed = [CommitIvk_rivk(ExtractP(ak), nk)] * g_d_signed`.
#[derive(Clone, Debug, Default)]
pub struct Circuit {
    nk: Value<NullifierDerivingKey>,
    rho_signed: Value<pallas::Base>,
    psi_signed: Value<pallas::Base>,
    cm_signed: Value<NoteCommitment>,
    ak: Value<SpendValidatingKey>,
    alpha: Value<pallas::Scalar>,
    rivk: Value<CommitIvkRandomness>,
    rcm_signed: Value<NoteCommitTrapdoor>,
    g_d_signed: Value<NonIdentityPallasPoint>,
    pk_d_signed: Value<DiversifiedTransmissionKey>,
}

impl Circuit {
    /// Constructs a `Circuit` from a note, its full viewing key, and the spend auth randomizer.
    pub fn from_note_unchecked(fvk: &FullViewingKey, note: &Note, alpha: pallas::Scalar) -> Self {
        let sender_address = note.recipient();
        let rho_signed = note.rho();
        let psi_signed = note.rseed().psi(&rho_signed);
        let rcm_signed = note.rseed().rcm(&rho_signed);
        Circuit {
            nk: Value::known(*fvk.nk()),
            rho_signed: Value::known(rho_signed.0),
            psi_signed: Value::known(psi_signed),
            cm_signed: Value::known(note.commitment()),
            ak: Value::known(fvk.clone().into()),
            alpha: Value::known(alpha),
            rivk: Value::known(fvk.rivk(Scope::External)),
            rcm_signed: Value::known(rcm_signed),
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

        // Configuration for decomposition and canonicity checking for NoteCommit.
        let note_commit_config =
            NoteCommitChip::configure(meta, advices, sinsemilla_config.clone());

        Config {
            primary,
            advices,
            add_config,
            ecc_config,
            poseidon_config,
            sinsemilla_config,
            commit_ivk_config,
            note_commit_config,
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
        // It is needed to derive cm_signed ECC point from NoteCommitment.
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

        // Witness rho_signed.
        // This is the nullifier of the note that was spent to create this note. It is
        // a Nullifier type (a Pallas base field element) that serves as a unique, per-note domain
        // separator.
        // rho ensures that even if two notes have identical contents, they will produce
        // different nullifiers because they were created by spending different input notes.
        // rho provides deterministic, structural uniqueness. It is the nullifier of the
        // spend input note so it chains each note to its creation context. A single tx
        // can create multiple output notes from the same input. All those outputs share the same
        // rho. If nullifier derivation only used rho (no psi), outputs from the same input could collide.
        let rho_signed = assign_free_advice(
            layouter.namespace(|| "witness rho_signed"),
            config.advices[0],
            self.rho_signed,
        )?;

        // Witness psi_signed.
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
        let psi_signed = assign_free_advice(
            layouter.namespace(|| "witness psi_signed"),
            config.advices[0],
            self.psi_signed,
        )?;

        // Witness cm_signed as an ECC point, which is the form DeriveNullifier expects.
        let cm_signed = Point::new(
            ecc_chip.clone(),
            layouter.namespace(|| "witness cm_signed"),
            self.cm_signed.as_ref().map(|cm| cm.inner().to_affine()),
        )?;

        // Nullifier integrity: derive nf_signed = DeriveNullifier(nk, rho_signed, psi_signed, cm_signed).
        let nf_signed = derive_nullifier(
            layouter.namespace(|| "nf_signed = DeriveNullifier_nk(rho_signed, psi_signed, cm_signed)"),
            config.poseidon_chip(),
            config.add_chip(),
            ecc_chip.clone(),
            rho_signed.clone(), // clone so rho_signed remains available for note_commit
            &psi_signed,
            &cm_signed,
            nk.clone(), // clone so nk remains available for commit_ivk
        )?;

        // Constrain nf_signed to equal the public input.
        // Enforce that the nullifier computed inside the circuit matches the nullifier provided
        // as a public input from outside the circuit (supplied at NF_SIGNED of the public input)
        layouter.constrain_instance(nf_signed.inner().cell(), config.primary, NF_SIGNED)?;

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
        let pk_d_signed = {
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

            pk_d_signed
        };

        // signed note commitment integrity.
        // NoteCommit_rcm_signed(repr(g_d_signed), repr(pk_d_signed), 0,
        //                        rho_signed, psi_signed) = cm_signed
        // No null option: the signed note must have a valid commitment.
        {
            let rcm_signed = ScalarFixed::new(
                ecc_chip.clone(),
                layouter.namespace(|| "rcm_signed"),
                self.rcm_signed.as_ref().map(|rcm| rcm.inner()),
            )?;

            // The signed note's value is always 0.
            let v_signed = assign_free_advice(
                layouter.namespace(|| "v_signed = 0"),
                config.advices[0],
                Value::known(NoteValue::zero()),
            )?;

            // Compute NoteCommit from witness data.
            let derived_cm_signed = note_commit(
                layouter.namespace(|| "NoteCommit_rcm_signed(g_d, pk_d, 0, rho, psi)"),
                config.sinsemilla_chip(),
                config.ecc_chip(),
                config.note_commit_chip(),
                g_d_signed.inner(),
                pk_d_signed.inner(),
                v_signed,
                rho_signed,
                psi_signed,
                rcm_signed,
            )?;

            // Strict equality — no null/bottom option.
            derived_cm_signed.constrain_equal(
                layouter.namespace(|| "cm_signed integrity"),
                &cm_signed,
            )?;
        }

        Ok(())
    }
}

/// Public inputs to the Delegation circuit.
#[derive(Clone, Debug)]
pub struct Instance {
    /// The derived nullifier (temporary public input; will be replaced by gov_null).
    pub nf_signed: Nullifier,
    /// The randomized spend validating key, used for signature verification out-of-circuit.
    pub rk: VerificationKey<SpendAuth>,
}

impl Instance {
    /// Constructs an [`Instance`] from its constituent parts.
    pub fn from_parts(nf_signed: Nullifier, rk: VerificationKey<SpendAuth>) -> Self {
        Instance { nf_signed, rk }
    }

    /// Returns the public inputs as a vector of field elements for halo2.
    pub fn to_halo2_instance(&self) -> Vec<vesta::Scalar> {
        let rk = pallas::Point::from_bytes(&self.rk.clone().into())
            .unwrap()
            .to_affine()
            .coordinates()
            .unwrap();

        vec![self.nf_signed.0, *rk.x(), *rk.y()]
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
    use halo2_proofs::{dev::MockProver, plonk::Circuit as Halo2Circuit};
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

    #[test]
    fn note_commit_integrity_happy_path() {
        let (circuit, nf, rk) = make_test_note();
        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert_eq!(prover.verify(), Ok(()));
    }

    #[test]
    fn note_commit_integrity_wrong_rcm() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace rcm_signed with a different note's rcm (wrong trapdoor)
        let (_sk2, _fvk2, note2) = Note::dummy(&mut rng, None);
        let rho2 = note2.rho();
        let wrong_rcm = note2.rseed().rcm(&rho2);
        circuit.rcm_signed = Value::known(wrong_rcm);

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn note_commit_integrity_wrong_cm() {
        let mut rng = OsRng;
        let (_sk1, fvk1, note1) = Note::dummy(&mut rng, None);
        let (_sk2, _fvk2, note2) = Note::dummy(&mut rng, None);

        // Build circuit from note1 but use note2's commitment
        let nf = note1.nullifier(&fvk1);
        let ak: SpendValidatingKey = fvk1.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk1, &note1, alpha);
        circuit.cm_signed = Value::known(note2.commitment());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    // ================================================================
    // Witness-tampering tests for previously uncovered private inputs.
    // Each test modifies exactly ONE witness while keeping the correct
    // public inputs, verifying that the circuit rejects the proof.
    // ================================================================

    #[test]
    fn wrong_rho_signed_witness() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Tamper rho_signed — feeds into both nullifier derivation and note commitment.
        circuit.rho_signed = Value::known(pallas::Base::random(&mut rng));

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn wrong_psi_signed_witness() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Tamper psi_signed — feeds into both nullifier derivation and note commitment.
        circuit.psi_signed = Value::known(pallas::Base::random(&mut rng));

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn wrong_g_d_signed_witness() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace g_d_signed with a different key's diversified generator.
        // g_d feeds into both address integrity ([ivk] * g_d = pk_d) and
        // note commitment (NoteCommit includes repr(g_d)).
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        let other_address = fvk2.address_at(0u32, Scope::External);
        circuit.g_d_signed = Value::known(other_address.g_d());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn wrong_ak_witness() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace ak with a different key's ak.
        // ak feeds into both spend authority (rk = [alpha]*G + ak) and
        // address integrity (ivk = CommitIvk(ExtractP(ak), nk, rivk)).
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        circuit.ak = Value::known(fvk2.clone().into());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn wrong_nk_witness() {
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);

        // Replace nk with a different key's nk.
        // nk feeds into both nullifier derivation and CommitIvk for address integrity.
        // This is the "attacker knows the correct public inputs but tries to prove with wrong nk"
        // scenario — more realistic than nullifier_integrity_wrong_key which tampers the public input.
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        circuit.nk = Value::known(*fvk2.nk());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    // ================================================================
    // Cross-constraint binding tests.
    // Verify that shared witnesses (nk, ak, rho, g_d) are actually the
    // SAME cell across constraints. An attacker who could "split" a
    // shared witness would break soundness.
    // ================================================================

    #[test]
    fn cross_constraint_ak_binds_spend_authority_and_address_integrity() {
        // If ak were not shared, an attacker could satisfy spend authority with ak1
        // (matching rk) and address integrity with ak2 (matching ivk/pk_d).
        // With shared ak, both constraints must agree — so wrong ak must fail BOTH.
        //
        // We construct a scenario where ak2 would satisfy address integrity
        // (by using fvk2's rivk, g_d, pk_d) but the rk public input was derived
        // from ak1. The circuit should still fail because it uses a single ak cell.
        let mut rng = OsRng;
        let (_sk1, fvk1, note1) = Note::dummy(&mut rng, None);
        let nf = note1.nullifier(&fvk1);
        let ak1: SpendValidatingKey = fvk1.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak1.randomize(&alpha); // rk derived from ak1

        let mut circuit = Circuit::from_note_unchecked(&fvk1, &note1, alpha);

        // Swap in ak from a different key — breaks spend authority
        // even if we also swap rivk/g_d/pk_d to make address integrity
        // consistent with the new ak (the circuit only has one ak cell).
        let sk2 = SpendingKey::random(&mut rng);
        let fvk2: FullViewingKey = (&sk2).into();
        let addr2 = fvk2.address_at(0u32, Scope::External);
        circuit.ak = Value::known(fvk2.clone().into());
        circuit.rivk = Value::known(fvk2.rivk(Scope::External));
        circuit.g_d_signed = Value::known(addr2.g_d());
        circuit.pk_d_signed = Value::known(*addr2.pk_d());

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn cross_constraint_rho_binds_nullifier_and_note_commit() {
        // rho is used in both DeriveNullifier and NoteCommit. If they could
        // diverge, an attacker could use rho1 for nullifier (matching nf) and
        // rho2 for NoteCommit (matching cm). Verify the shared binding rejects this.
        let mut rng = OsRng;
        let (_sk, fvk, note) = Note::dummy(&mut rng, None);
        let nf = note.nullifier(&fvk);
        let ak: SpendValidatingKey = fvk.clone().into();
        let alpha = pallas::Scalar::random(&mut rng);
        let rk = ak.randomize(&alpha);

        // Build a second note to get a different rho/commitment pair
        let (_sk2, _fvk2, note2) = Note::dummy(&mut rng, None);

        // Construct circuit from note1 but inject note2's rho and cm.
        // If rho were split, the prover could route note2's rho to NoteCommit
        // and note1's rho to DeriveNullifier. Shared cell prevents this.
        let mut circuit = Circuit::from_note_unchecked(&fvk, &note, alpha);
        let rho2 = note2.rho();
        circuit.rho_signed = Value::known(rho2.0);
        circuit.psi_signed = Value::known(note2.rseed().psi(&rho2));
        circuit.rcm_signed = Value::known(note2.rseed().rcm(&rho2));
        circuit.cm_signed = Value::known(note2.commitment());
        // g_d and pk_d still from note1's address — NoteCommit will mismatch.

        let instance = Instance::from_parts(nf, rk);
        let public_inputs = instance.to_halo2_instance();
        let prover = MockProver::run(K, &circuit, vec![public_inputs]).unwrap();
        assert!(prover.verify().is_err());
    }

    // ================================================================
    // Edge-case and structural tests.
    // ================================================================

    #[test]
    fn multiple_independent_notes_verify() {
        // Two independent notes should each produce valid proofs.
        // Verifies that the circuit is properly parameterised and stateless.
        let (circuit1, nf1, rk1) = make_test_note();
        let (circuit2, nf2, rk2) = make_test_note();

        let pi1 = Instance::from_parts(nf1, rk1).to_halo2_instance();
        let pi2 = Instance::from_parts(nf2, rk2).to_halo2_instance();

        let prover1 = MockProver::run(K, &circuit1, vec![pi1]).unwrap();
        let prover2 = MockProver::run(K, &circuit2, vec![pi2]).unwrap();

        assert_eq!(prover1.verify(), Ok(()));
        assert_eq!(prover2.verify(), Ok(()));
    }

    #[test]
    fn swapped_public_inputs_fail() {
        // Swapping the public inputs between two valid circuits should fail.
        // Guards against confused-deputy scenarios.
        let (circuit1, nf1, rk1) = make_test_note();
        let (_circuit2, nf2, rk2) = make_test_note();

        // Feed circuit1 the public inputs of circuit2
        let wrong_pi = Instance::from_parts(nf2, rk2).to_halo2_instance();
        let prover = MockProver::run(K, &circuit1, vec![wrong_pi]).unwrap();
        assert!(prover.verify().is_err());

        // And vice versa
        let wrong_pi = Instance::from_parts(nf1, rk1).to_halo2_instance();
        let prover = MockProver::run(K, &_circuit2, vec![wrong_pi]).unwrap();
        assert!(prover.verify().is_err());
    }

    #[test]
    fn instance_to_halo2_roundtrip() {
        // Verify that to_halo2_instance produces the expected three-element vector
        // [nf, rk.x, rk.y] and that the values are consistent with the inputs.
        let (_, nf, rk) = make_test_note();
        let instance = Instance::from_parts(nf, rk.clone());
        let pi = instance.to_halo2_instance();

        assert_eq!(pi.len(), 3, "Expected exactly 3 public inputs (nf, rk.x, rk.y)");
        assert_eq!(pi[NF_SIGNED], nf.0, "First element must be nf");

        // Reconstruct rk coordinates independently and compare.
        let rk_point = pallas::Point::from_bytes(&rk.into())
            .unwrap()
            .to_affine()
            .coordinates()
            .unwrap();
        assert_eq!(pi[RK_X], *rk_point.x(), "Second element must be rk.x");
        assert_eq!(pi[RK_Y], *rk_point.y(), "Third element must be rk.y");
    }

    #[test]
    fn default_circuit_is_consistent_with_without_witnesses() {
        // Verify that without_witnesses() returns a default circuit,
        // and that the default circuit can be used for keygen (configure + FloorPlanner)
        // without panicking. halo2 relies on this for keygen_vk / keygen_pk.
        let (circuit, _, _) = make_test_note();
        let empty = Halo2Circuit::without_witnesses(&circuit);

        // The default circuit should be able to produce verification keys.
        // This is the same codepath halo2 uses during keygen.
        let params = halo2_proofs::poly::commitment::Params::<vesta::Affine>::new(K);
        let vk = halo2_proofs::plonk::keygen_vk(&params, &empty);
        assert!(vk.is_ok(), "keygen_vk must succeed on without_witnesses circuit");
    }
}

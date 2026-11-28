//! A real ZKP1 and its batch's real ZKP4 proofs through the `ProxyDelegate`
//! step, in release only (`make proofs`).

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use voting_circuits::{
    delegation::{ImtProofData, ImtProvider, SpacedLeafImtProvider},
    proxy_delegation::{create_proxy_delegation_proof, verify_proxy_delegation_proof},
};
use voting_crypto_deps::incrementalmerkletree::{Hashable, Level};

use super::super::fixtures::{host_with_driver_target, Interrupt, RecordingProgress, ROUND_ID};
use super::fixtures::{proxy_database, proxy_executor, van_comm_rand, HOTKEY_SECRET, NETWORK};
use crate::{
    backend::{
        orchard::{
            self,
            keys::Scope,
            note::{ExtractedNoteCommitment, NoteVersion},
            tree::MerkleHashOrchard,
            value::NoteValue,
            NOTE_COMMITMENT_TREE_DEPTH,
        },
        pasta_curves::{group::ff::PrimeField, pallas},
        zcash_keys::keys::UnifiedSpendingKey,
    },
    delegate::{DelegationProofStatus, SignedDelegationBundle},
    delegation_pipeline::{DelegationDriver, DelegationSigner},
    governance::{BALLOT_DIVISOR, BUNDLE_NOTE_SLOTS},
    proxy::batch::{build_dc_chain, load_chain_inputs, load_proved_batch, BatchAnchor},
    round::VotingDb,
    session::NextStep,
    types::{DelegationProgressReporter, WitnessData},
    ChainSubmissionControl, Network, NoteInfo, PirFleet, RoundStepDisposition, RoundStepProgress,
    VotingError, VotingHotkey, VotingHotkeyTarget,
};

/// One full note-slot bundle of synthetic wallet notes, with the note
/// commitment tree witnesses and IMT non-membership proofs ZKP1 takes.
struct RegistrationNotes {
    notes: Vec<NoteInfo>,
    witnesses: Vec<WitnessData>,
    imt_proofs: Vec<ImtProofData>,
    total: u64,
}

/// Builds the notes as `zkp1::test_real_delegation_proof` does: every note
/// slot filled, so no padding or PIR server is needed. The notes are mainnet
/// encoded; ZKP1 decodes their viewing key with that network.
fn registration_notes() -> RegistrationNotes {
    let account = voting_crypto_deps::zip32::AccountId::ZERO;
    let usk = UnifiedSpendingKey::from_seed(&Network::Mainnet, &[0x42; 32], account).unwrap();
    let ufvk = usk.to_unified_full_viewing_key();
    let ufvk_str = crate::backend::encode_full_viewing_key(&ufvk, &Network::Mainnet).unwrap();
    let fvk = ufvk.orchard().unwrap().clone();
    let address = fvk.address_at(0u32, Scope::External);
    let value = 25 * BALLOT_DIVISOR + 7;
    let mut rng = voting_crypto_deps::rand::rngs::OsRng;
    let notes: Vec<orchard::Note> = (0..BUNDLE_NOTE_SLOTS)
        .map(|_| {
            let (_, _, parent) = voting_crypto_deps::dummy_note(&mut rng, None, NoteVersion::V3);
            voting_crypto_deps::new_note(
                address,
                NoteValue::from_raw(value),
                voting_crypto_deps::rho_from_nullifier(parent.nullifier(&fvk)),
                NoteVersion::V3,
                &mut rng,
            )
        })
        .collect();

    // Eight leaves under one subtree, then empty siblings to the root.
    let mut leaves = [MerkleHashOrchard::empty_leaf(); 8];
    for (leaf, note) in leaves.iter_mut().zip(&notes) {
        *leaf = MerkleHashOrchard::from_cmx(&ExtractedNoteCommitment::from(note.commitment()));
    }
    let level_one: Vec<_> = leaves
        .chunks(2)
        .map(|pair| MerkleHashOrchard::combine(Level::from(0), &pair[0], &pair[1]))
        .collect();
    let level_two: Vec<_> = level_one
        .chunks(2)
        .map(|pair| MerkleHashOrchard::combine(Level::from(1), &pair[0], &pair[1]))
        .collect();
    let mut root = MerkleHashOrchard::combine(Level::from(2), &level_two[0], &level_two[1]);
    for level in 3..NOTE_COMMITMENT_TREE_DEPTH {
        let sibling = MerkleHashOrchard::empty_root(Level::from(level as u8));
        root = MerkleHashOrchard::combine(Level::from(level as u8), &root, &sibling);
    }
    let root = root.to_bytes().to_vec();

    let imt = SpacedLeafImtProvider::new();
    let mut witnesses = Vec::new();
    let mut imt_proofs = Vec::new();
    let mut infos = Vec::new();
    for (index, note) in notes.iter().enumerate() {
        let mut path = [MerkleHashOrchard::empty_leaf(); NOTE_COMMITMENT_TREE_DEPTH];
        path[0] = leaves[index ^ 1];
        path[1] = level_one[(index >> 1) ^ 1];
        path[2] = level_two[(index >> 2) ^ 1];
        for (level, sibling) in path.iter_mut().enumerate().skip(3) {
            *sibling = MerkleHashOrchard::empty_root(Level::from(level as u8));
        }
        let cmx = ExtractedNoteCommitment::from(note.commitment());
        witnesses.push(WitnessData {
            note_commitment: MerkleHashOrchard::from_cmx(&cmx).to_bytes().to_vec(),
            position: index as u64,
            root: root.clone(),
            auth_path: path.iter().map(|hash| hash.to_bytes().to_vec()).collect(),
        });
        let nullifier = note.nullifier(&fvk).to_bytes();
        imt_proofs.push(
            imt.non_membership_proof(pallas::Base::from_repr(nullifier).unwrap())
                .unwrap(),
        );
        infos.push(NoteInfo {
            commitment: cmx.to_bytes().to_vec(),
            nullifier: nullifier.to_vec(),
            value,
            position: index as u64,
            diversifier: note.recipient().diversifier().as_array().to_vec(),
            rho: note.rho().to_bytes().to_vec(),
            rseed: note.rseed().as_bytes().to_vec(),
            scope: 0,
            ufvk_str: ufvk_str.clone(),
        });
    }
    RegistrationNotes {
        notes: infos,
        witnesses,
        imt_proofs,
        total: value * BUNDLE_NOTE_SLOTS as u64,
    }
}

/// A delegation driver whose proof-only preparation proves the bundle's real
/// ZKP1 and stores it. It never signs.
struct RealRegistrationDriver {
    db: Arc<VotingDb>,
    notes: RegistrationNotes,
    target: VotingHotkeyTarget,
    prepared: AtomicUsize,
}

impl DelegationDriver for RealRegistrationDriver {
    fn round_id(&self) -> &str {
        ROUND_ID
    }

    fn network(&self) -> Network {
        NETWORK
    }

    fn delegation_target(&self) -> Option<VotingHotkeyTarget> {
        Some(self.target)
    }

    fn wallet_id(&self) -> &str {
        "wallet"
    }

    fn shares_database_with(&self, database: &VotingDb) -> bool {
        self.db.shares_connection_with(database)
    }

    fn prepare_blocking(
        &self,
        bundle_index: u32,
        _pir: &PirFleet,
        progress: &dyn DelegationProgressReporter,
    ) -> Result<DelegationProofStatus, VotingError> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        let result = crate::zkp1::build_and_prove_delegation(
            &self.notes.notes,
            self.target.raw_orchard_address(),
            &pallas::Scalar::from(7u64).to_repr(),
            &van_comm_rand(),
            &hex::decode(ROUND_ID).unwrap(),
            &self.notes.witnesses,
            &self.notes.imt_proofs,
            &[],
            Network::Mainnet,
            progress,
            None,
        )?;
        let stored_van: Vec<u8> = self
            .db
            .conn()
            .query_row("SELECT gov_comm FROM bundles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            result.van_comm, stored_van,
            "the real ZKP1 mints the registration VAN the setup stored"
        );
        crate::storage::queries::store_proof(
            &self.db.conn(),
            ROUND_ID,
            "wallet",
            bundle_index,
            &result.proof,
            Some(&result.key_fingerprint),
        )?;
        Ok(DelegationProofStatus::Generated)
    }

    fn prove_and_sign_blocking(
        &self,
        _bundle_index: u32,
        _signer: &DelegationSigner,
        _pir: &PirFleet,
        _progress: &dyn DelegationProgressReporter,
    ) -> Result<SignedDelegationBundle, VotingError> {
        panic!("a proxy batch never signs its registration while proving")
    }

    fn resign_blocking(
        &self,
        _bundle_index: u32,
        _signer: &DelegationSigner,
    ) -> Result<[u8; 64], VotingError> {
        panic!("a proxy batch never signs its registration while proving")
    }
}

/// The step proves the real registration, then both DCs against it, and
/// stores a batch whose proofs verify. Rebuilding the same slots and proving
/// them again yields byte-identical DCs whose new proofs verify too.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "generates a real ZKP1 and real ZKP4 proofs; run make proofs"]
async fn a_real_registration_and_its_batch_prove_through_the_step() {
    let notes = registration_notes();
    let db = proxy_database(notes.total);
    let executor = proxy_executor(db.clone(), true);
    let target = VotingHotkey::from_stored_secret(&HOTKEY_SECRET, NETWORK)
        .unwrap()
        .delegation_target();
    let driver = Arc::new(RealRegistrationDriver {
        db: db.clone(),
        notes,
        target,
        prepared: AtomicUsize::new(0),
    });
    let control = ChainSubmissionControl::new(1);
    let mut host = host_with_driver_target(
        &control,
        Interrupt::Cancel,
        NETWORK,
        Some(target),
        "wallet",
        &db,
    );
    host.delegation.as_mut().unwrap().driver = driver.clone();
    host.max_proof_concurrency = 4;
    let progress = RecordingProgress::default();

    let started = Instant::now();
    let outcome = executor
        .advance_step_in_epoch(
            NextStep::ProxyDelegate { bundle_index: 0 },
            &host,
            &control,
            control.operation_epoch(),
            &progress,
        )
        .await
        .unwrap();
    let elapsed = started.elapsed();
    println!(
        "ZKP1 + 2 x ZKP4 through ProxyDelegate, keygen included: {:.2} s",
        elapsed.as_secs_f64()
    );

    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    assert_eq!(driver.prepared.load(Ordering::SeqCst), 1);
    assert!(progress.events.lock().unwrap().iter().any(|event| matches!(
        event,
        RoundStepProgress::ProxyBatchProved { bundle_index: 0 }
    )));
    assert!(
        !outcome
            .plan
            .next_steps
            .contains(&NextStep::ProxyDelegate { bundle_index: 0 }),
        "a proved batch owes no further proof"
    );

    let batch = load_proved_batch(&db.conn(), ROUND_ID, "wallet", 0)
        .unwrap()
        .unwrap();
    assert_eq!(batch.anchor_height, 0, "the batch carries the registration");
    assert_eq!(batch.dcs.len(), 2);
    let inputs = db
        .read_transaction("test", |tx| {
            load_chain_inputs(tx, ROUND_ID, "wallet", 0, NETWORK)
        })
        .unwrap();
    let rebuilt = build_dc_chain(&inputs, &BatchAnchor::Registration, &HOTKEY_SECRET).unwrap();
    for (stored, again) in batch.dcs.iter().zip(&rebuilt) {
        let instance = &again.bundle.instance;
        assert_eq!(stored.van_nullifier, instance.van_nullifier.to_repr());
        assert_eq!(stored.r_vpk, again.bundle.r_vpk_bytes);
        assert_eq!(
            stored.vote_authority_note_new,
            instance.vote_authority_note_new.to_repr()
        );
        assert_eq!(
            stored.delegation_commitment,
            instance.delegation_commitment.to_repr()
        );
        assert_eq!(
            stored.dc_slot_nullifier,
            instance.dc_slot_nullifier.to_repr()
        );
        verify_proxy_delegation_proof(&stored.proof, instance).expect("the stored proof verifies");
        let reproved = create_proxy_delegation_proof(again.bundle.circuit.clone(), instance)
            .expect("the rebuilt DC proves again");
        verify_proxy_delegation_proof(&reproved, instance).expect("the re-proof verifies");
    }
}

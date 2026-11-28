//! The `AdvanceProxyBatch` step: an attempt is signed and durable before its
//! POST, an expired one is re-signed with no new proof or device signature,
//! the hold lifts only when the batch lands, a refused batch stays pending
//! and is sent again, and a lifted registration splits the batch off. The
//! status view shows a refused batch as submitting until the round closes.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use super::fixtures::{host, unreachable_pir, RecordingProgress};
use crate::{
    chain_submission::tests::scripted_chain::{
        accepted, chain_height, count, expect_code_two_pass, expect_tree_pass, landed,
        registration_van, rejected, unrelated_leaf, ScriptedChain,
    },
    delegate::{DelegationProofStatus, SignedDelegationBundle},
    delegation_pipeline::{DelegationDriver, DelegationSigner, KeystoneSignatureSource},
    phases::DelegationPhase,
    pir::PirFleet,
    proxy::{
        progress, status,
        tests::submission_fixtures::{
            batch_digest, bound_executor, proved_batch, scripted_chain_config, FixtureAnchor,
            ProvedBatchFixture, HOTKEY_SECRET, NETWORK, ROUND_ID, WALLET_ID,
        },
        DelegateBallotList, ProxyRoundPhase,
    },
    round::VotingDb,
    session::{Decision, NextStep, ProxyBatchStage},
    types::DelegationProgressReporter,
    wire::{ProxyDelegationProgress, ProxyDelegationStage},
    BallotIntent, ChainSubmissionControl, DelegationStepInputs, HelperClient, HelperHealth,
    HyperTransport, RoundExecutor, RoundHostContext, RoundStepDisposition, RoundStepFailureKind,
    RoundStepProgress, VotingError, VotingHotkey,
};

const STEP: NextStep = NextStep::AdvanceProxyBatch { bundle_index: 0 };

/// A delegation driver that only re-signs, with the fixture registration's
/// real SpendAuth signature, and counts how often the device was asked.
struct RegistrationDriver {
    db: Arc<VotingDb>,
    signature: [u8; 64],
    signatures: AtomicUsize,
}

impl DelegationDriver for RegistrationDriver {
    fn round_id(&self) -> &str {
        ROUND_ID
    }

    fn network(&self) -> crate::Network {
        NETWORK
    }

    fn delegation_target(&self) -> Option<crate::VotingHotkeyTarget> {
        Some(
            VotingHotkey::from_stored_secret(&HOTKEY_SECRET, NETWORK)
                .unwrap()
                .delegation_target(),
        )
    }

    fn wallet_id(&self) -> &str {
        WALLET_ID
    }

    fn shares_database_with(&self, database: &VotingDb) -> bool {
        self.db.shares_connection_with(database)
    }

    fn prepare_blocking(
        &self,
        _bundle_index: u32,
        _pir: &PirFleet,
        _progress: &dyn DelegationProgressReporter,
    ) -> Result<DelegationProofStatus, VotingError> {
        panic!("a proved proxy batch never proves its registration again")
    }

    fn prove_and_sign_blocking(
        &self,
        _bundle_index: u32,
        _signer: &DelegationSigner,
        _pir: &PirFleet,
        _progress: &dyn DelegationProgressReporter,
    ) -> Result<SignedDelegationBundle, VotingError> {
        panic!("a proxy batch submission never proves a delegation")
    }

    fn resign_blocking(
        &self,
        _bundle_index: u32,
        _signer: &DelegationSigner,
    ) -> Result<[u8; 64], VotingError> {
        self.signatures.fetch_add(1, Ordering::SeqCst);
        Ok(self.signature)
    }
}

/// The fixture round with its executor, its scripted chain, the driver and a
/// host that uses it.
struct Round {
    fixture: ProvedBatchFixture,
    executor: RoundExecutor<Arc<ScriptedChain>>,
    chain: Arc<ScriptedChain>,
    driver: Arc<RegistrationDriver>,
    host: RoundHostContext,
    digest: [u8; 32],
}

fn round(anchor: FixtureAnchor) -> Round {
    let fixture = proved_batch(anchor);
    let chain = Arc::new(ScriptedChain::default());
    let executor = bound_executor(
        &fixture.db,
        Arc::clone(&chain),
        scripted_chain_config(),
        HelperClient::new(Arc::new(HyperTransport::new()), HelperHealth::default()),
        &[(1, 2)],
    );
    let driver = Arc::new(RegistrationDriver {
        db: Arc::clone(&fixture.db),
        signature: fixture.registration_signature,
        signatures: AtomicUsize::new(0),
    });
    let host = RoundHostContext {
        delegation: Some(DelegationStepInputs {
            driver: driver.clone(),
            signer: DelegationSigner::Keystone(KeystoneSignatureSource::Stored),
            pir: unreachable_pir(),
        }),
        ..host()
    };
    let digest = batch_digest(&fixture);
    Round {
        fixture,
        executor,
        chain,
        driver,
        host,
        digest,
    }
}

impl Round {
    /// Runs the batch's step once, which must succeed.
    async fn step(&self) -> crate::RoundStepOutcome {
        self.step_recording(&RecordingProgress::default()).await
    }

    /// Runs the batch's step once, reporting to `progress`.
    async fn step_recording(&self, progress: &RecordingProgress) -> crate::RoundStepOutcome {
        let control = ChainSubmissionControl::new(1);
        self.executor
            .advance_step_in_epoch(STEP, &self.host, &control, 1, progress)
            .await
            .unwrap()
    }

    /// Runs the batch's step once, which must fail.
    async fn failing_step(&self) -> crate::RoundStepFailure {
        let control = ChainSubmissionControl::new(1);
        self.executor
            .advance_step_in_epoch(STEP, &self.host, &control, 1, &RecordingProgress::default())
            .await
            .unwrap_err()
    }

    fn db(&self) -> &VotingDb {
        &self.fixture.db
    }

    fn stage(&self) -> ProxyBatchStage {
        self.executor.plan().unwrap().proxy_batches[0].stage
    }

    fn expiry(&self) -> i64 {
        self.db()
            .conn()
            .query_row(
                "SELECT expiry_height FROM proxy_batch_attempts",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
}

/// The proofs and proving time of the stored batch, to show nothing was
/// proved again.
fn proofs(db: &VotingDb) -> (Vec<Vec<u8>>, i64) {
    let conn = db.conn();
    let proofs = conn
        .prepare("SELECT proof FROM proxy_batch_dcs ORDER BY dc_slot")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let proved_at = conn
        .query_row("SELECT proved_at FROM proxy_batches", [], |row| row.get(0))
        .unwrap();
    (proofs, proved_at)
}

fn plans_cast_on_bundle_0(executor: &RoundExecutor<Arc<ScriptedChain>>) -> bool {
    executor.plan().unwrap().next_steps.iter().any(|step| {
        matches!(
            step,
            NextStep::CastVote {
                bundle_index: 0,
                ..
            }
        )
    })
}

/// The delegation job's progress, with no ballot published.
fn job(round: &Round) -> ProxyDelegationProgress {
    progress(round.db(), ROUND_ID, &no_ballots(), ProxyRoundPhase::Active)
        .unwrap()
        .unwrap()
}

/// The round's complete ballot list, with no ballot in it.
fn no_ballots() -> DelegateBallotList {
    let mut ballots = DelegateBallotList::new(ROUND_ID).unwrap();
    ballots.add_page(b"{}").unwrap();
    ballots
}

/// Each delegation's `(delegate, ballots, stage)` in the status view.
fn delegation_stages(
    round: &Round,
    phase: ProxyRoundPhase,
) -> Vec<(u32, u64, ProxyDelegationStage)> {
    status(round.db(), ROUND_ID, &no_ballots(), phase)
        .unwrap()
        .unwrap()
        .delegations
        .into_iter()
        .map(|view| (view.delegate_index, view.ballots, view.stage))
        .collect()
}

/// Records a choice for the round's one proposal.
fn choose<T: crate::ChainTransport>(executor: &RoundExecutor<T>) {
    executor
        .set_ballot_intents(&[BallotIntent {
            proposal_id: 1,
            decision: Decision::Choice(1),
        }])
        .unwrap();
}

#[tokio::test]
async fn an_attempt_is_signed_and_durable_before_its_post_and_landing_lifts_the_hold() {
    let round = round(FixtureAnchor::Registration);
    choose(&round.executor);
    assert_eq!(round.executor.plan().unwrap().next_steps, vec![STEP]);
    assert!(!plans_cast_on_bundle_0(&round.executor));
    round.chain.expect("GET", "/v1/height", chain_height(1_000));
    {
        let db = Arc::clone(&round.fixture.db);
        round.chain.before_post(move || {
            let expiry: i64 = db
                .conn()
                .query_row(
                    "SELECT expiry_height FROM proxy_batch_attempts",
                    [],
                    |row| row.get(0),
                )
                .expect("the attempt is durable before its POST");
            assert_eq!(expiry, 1_500);
        });
    }
    round
        .chain
        .expect("POST", "/proxy-delegation-batch", accepted(round.digest));
    round
        .chain
        .expect("GET", "/tx/", landed(&round.fixture, round.digest, 5));
    let progress = RecordingProgress::default();

    let outcome = round.step_recording(&progress).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    round.chain.assert_drained();
    assert_eq!(round.driver.signatures.load(Ordering::SeqCst), 1);
    let body = &round.chain.post_bodies()[0];
    assert_eq!(body["expiry_height"], 1_500);
    assert!(body.get("registration").is_some());
    {
        let events = progress.events.lock().unwrap();
        let signed = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    RoundStepProgress::ProxyBatchSigned { bundle_index: 0 }
                )
            })
            .expect("the signed attempt is reported");
        let chain = events
            .iter()
            .position(|event| matches!(event, RoundStepProgress::ChainOutcome(_)))
            .unwrap();
        assert!(signed < chain);
    }
    assert_eq!(round.stage(), ProxyBatchStage::Landed);
    assert_eq!(
        round.db().delegation_phase(ROUND_ID, 0).unwrap(),
        DelegationPhase::Confirmed
    );
    assert!(!round.executor.plan().unwrap().next_steps.contains(&STEP));
    assert!(
        plans_cast_on_bundle_0(&round.executor),
        "landing lifts the hold: the remainder is cast from the successor VAN"
    );
}

#[tokio::test]
async fn an_expired_attempt_is_re_signed_with_no_new_proof_or_device_signature() {
    let round = round(FixtureAnchor::Registration);
    let proved = proofs(round.db());
    round.chain.expect("GET", "/v1/height", chain_height(1_000));
    round.chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(round.digest, "vote", 52),
    );
    round.chain.expect("GET", "/v1/height", chain_height(1_600));
    round
        .chain
        .expect("POST", "/proxy-delegation-batch", accepted(round.digest));
    round
        .chain
        .expect("GET", "/tx/", landed(&round.fixture, round.digest, 5));

    let outcome = round.step().await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    round.chain.assert_drained();
    assert_eq!(
        round.driver.signatures.load(Ordering::SeqCst),
        1,
        "the registration is signed once"
    );
    assert_eq!(proofs(round.db()), proved, "nothing is proved again");
    assert_eq!(round.expiry(), 2_100);
    let bodies = round.chain.post_bodies();
    assert_eq!(bodies[0]["expiry_height"], 1_500);
    assert_eq!(bodies[1]["expiry_height"], 2_100);
    assert_eq!(bodies[0]["registration"], bodies[1]["registration"]);
    assert_eq!(bodies[0]["dcs"][0]["proof"], bodies[1]["dcs"][0]["proof"]);
    assert_ne!(
        bodies[0]["dcs"][0]["vote_auth_sig"],
        bodies[1]["dcs"][0]["vote_auth_sig"]
    );
    assert_eq!(round.stage(), ProxyBatchStage::Landed);
}

#[tokio::test]
async fn a_new_attempt_after_the_vote_end_is_refused() {
    let mut round = round(FixtureAnchor::Chain);
    round.host.now_seconds = round.host.vote_end_time_seconds.unwrap();

    let failure = round.failing_step().await;

    assert_eq!(failure.kind, RoundStepFailureKind::VoteEnded);
    assert_eq!(round.chain.posts(), 0);
    assert_eq!(
        count(round.db(), "SELECT COUNT(*) FROM proxy_batch_attempts"),
        0
    );
}

/// A refused batch stays pending on the wire and is sent again until it
/// lands. Its DCs show as submitting while the round runs, and as not
/// counted once it closes. A pause takes the same path (see
/// `a_paused_batch_stays_recoverable_and_retries_once_the_pause_lifts`).
#[tokio::test]
async fn a_refused_batch_stays_pending_and_is_sent_again() {
    use ProxyDelegationStage::{NotCounted, Submitting};
    let round = round(FixtureAnchor::Chain);
    round.chain.expect("GET", "/v1/height", chain_height(1_000));
    round.chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(round.digest, "vote", 5),
    );
    assert_eq!(
        round.step().await.disposition,
        RoundStepDisposition::Pending
    );

    let refused = job(&round);
    assert!(refused.broadcast_started && refused.refused_by_chain);
    assert_eq!(round.stage(), ProxyBatchStage::Refused);
    assert!(round.executor.plan().unwrap().next_steps.contains(&STEP));
    assert_eq!(
        delegation_stages(&round, ProxyRoundPhase::Active),
        [(17, 23, Submitting), (5, 17, Submitting)]
    );
    assert_eq!(
        delegation_stages(&round, ProxyRoundPhase::Closed),
        [(17, 23, NotCounted), (5, 17, NotCounted)]
    );

    expect_tree_pass(&round.chain, &[unrelated_leaf(1)]);
    round
        .chain
        .expect("POST", "/proxy-delegation-batch", accepted(round.digest));
    round
        .chain
        .expect("GET", "/tx/", landed(&round.fixture, round.digest, 9));
    assert_eq!(
        round.step().await.disposition,
        RoundStepDisposition::Advanced
    );
    round.chain.assert_drained();
    assert_eq!(round.chain.posts(), 2);
    assert_eq!(round.stage(), ProxyBatchStage::Landed);
    assert!(!job(&round).refused_by_chain);
}

#[tokio::test]
async fn a_lifted_registration_splits_the_batch_off_to_be_proved_again() {
    let round = round(FixtureAnchor::Registration);
    let registration = registration_van(round.db());
    round.chain.expect("GET", "/v1/height", chain_height(1_000));
    round.chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(round.digest, "vote", 2),
    );
    expect_code_two_pass(&round.chain, 1_000, &[unrelated_leaf(1), registration]);

    let outcome = round.step().await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    round.chain.assert_drained();
    assert_eq!(round.db().load_van_position_u64(ROUND_ID, 0).unwrap(), 1);
    assert_eq!(
        round.db().delegation_phase(ROUND_ID, 0).unwrap(),
        DelegationPhase::Confirmed
    );
    assert_eq!(round.stage(), ProxyBatchStage::Planned);
    assert_eq!(
        round.executor.plan().unwrap().next_steps,
        vec![NextStep::ProxyDelegate { bundle_index: 0 }],
        "the same DCs are proved again against the chain anchor"
    );
}

//! DC share delivery through `SubmitShares`: a landed batch's DCs are
//! planned once, before their first POST, and delivered with shares derived
//! from the round hotkey; a share is handed off once its target helpers
//! definitely hold it, a short one is retried and reconciled, and a helper's
//! refusal is an ordinary failure retried on a later pass.
//! Background tracking then confirms each handed-off share and resends one
//! no helper reveals, and the status view follows each delegation through
//! those steps.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::json;

use super::fixtures::{host, RecordingProgress};
use crate::{
    chain_submission::tests::scripted_chain::{accepted, chain_height, landed, ScriptedChain},
    helper::transport::{HelperFuture, HelperResponse, HelperTransport, HelperTransportError},
    proxy::{
        progress, status,
        tests::submission_fixtures::{
            batch_digest, bound_executor, proved_batch_with, scripted_chain_config, FixtureAnchor,
            ProvedBatchFixture, HOTKEY_SECRET, ROUND_ID, WALLET_ID,
        },
        DcShareLayout, DelegateBallotList, ProxyRoundPhase, POOL_PROPOSAL_ID,
    },
    round::VotingDb,
    round_drive::{NoopRoundDriveReporter, RoundDriver, RoundHostSourceBridge, RoundQuiescence},
    session::{NextStep, ProxyBatchStage},
    share::pending_rounds_for_accounts,
    share_policy::ShareTimingPolicy,
    share_tracking::{
        os_random_bytes, track_pending_shares, CommitmentRef, ShareKey, ShareTrackingParams,
        ShareTrackingReport,
    },
    wire::{DelegateBallotView, ProxyDelegationProgress, ProxyDelegationStage},
    ChainSubmissionControl, HelperClient, HelperHealth, RoundExecutor, RoundHostContext,
    RoundStepDisposition, RoundStepFailureKind,
};

/// The leaf the fixture batch's final VAN lands at; its DCs follow.
const FINAL_VAN: u64 = 9;

/// How a scripted helper answers one share POST.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Queued,
    Duplicate,
    /// A definite refusal.
    Refused,
    /// No answer before the deadline: the outcome is unknown.
    Lost,
    /// 422 `delegate_not_found` (contract §11.8), a definite refusal.
    DelegateNotFound,
    /// 422 `proxy_delegation_disabled` (contract §11.8), a definite refusal.
    ProxyDisabled,
    /// 503 `leaf_not_found` (contract §11.8): like any 5xx, the outcome is
    /// unknown.
    LeafNotFound,
}

/// A helper fleet that answers share POSTs from a script of the next POSTs,
/// whichever helpers they go to, then from a per-helper script (`queued`
/// once both run out), and reports every share revealed or pending.
#[derive(Default)]
struct Helpers {
    posts: Mutex<Vec<(String, serde_json::Value)>>,
    next_answers: Mutex<VecDeque<Answer>>,
    answers: Mutex<HashMap<String, VecDeque<Answer>>>,
    revealed: AtomicBool,
    before_post: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// How long a readiness probe takes to answer.
    status_delay: Mutex<Duration>,
    /// How long a share POST takes to answer.
    post_delay: Mutex<Duration>,
}

impl Helpers {
    fn urls() -> Vec<String> {
        (1..=3)
            .map(|index| format!("https://helper-{index}.example"))
            .collect()
    }

    fn script(&self, helper: &str, answers: &[Answer]) {
        self.answers
            .lock()
            .unwrap()
            .insert(helper.to_string(), answers.iter().copied().collect());
    }

    /// Answers the next POSTs, in order, whichever helpers they go to; a
    /// share's first POSTs go to its planned helpers.
    fn script_next_posts(&self, answers: &[Answer]) {
        self.next_answers
            .lock()
            .unwrap()
            .extend(answers.iter().copied());
    }

    /// The helpers POSTs went to, in order.
    fn posted_to(&self) -> Vec<String> {
        self.posts().into_iter().map(|(helper, _)| helper).collect()
    }

    /// Every POST body, with the helper it went to.
    fn posts(&self) -> Vec<(String, serde_json::Value)> {
        self.posts.lock().unwrap().clone()
    }
}

impl HelperTransport for Helpers {
    fn get<'a>(&'a self, url: &'a str, _timeout: Duration) -> HelperFuture<'a> {
        let status = if url.contains("/share-status/") {
            if self.revealed.load(Ordering::SeqCst) {
                "confirmed"
            } else {
                "pending"
            }
        } else {
            "ok"
        };
        let delay = if url.contains("/share-status/") {
            Duration::ZERO
        } else {
            *self.status_delay.lock().unwrap()
        };
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(HelperResponse::json(
                200,
                format!(r#"{{"status":"{status}"}}"#).into_bytes(),
            ))
        })
    }

    fn post_json<'a>(
        &'a self,
        url: &'a str,
        body: Vec<u8>,
        _timeout: Duration,
    ) -> HelperFuture<'a> {
        if let Some(hook) = self.before_post.lock().unwrap().as_ref() {
            hook();
        }
        let helper = url
            .strip_suffix("/shielded-vote/v1/shares")
            .expect("a share POST")
            .to_string();
        self.posts
            .lock()
            .unwrap()
            .push((helper.clone(), serde_json::from_slice(&body).unwrap()));
        let next = self.next_answers.lock().unwrap().pop_front();
        let answer = next
            .or_else(|| {
                self.answers
                    .lock()
                    .unwrap()
                    .get_mut(&helper)
                    .and_then(VecDeque::pop_front)
            })
            .unwrap_or(Answer::Queued);
        let delay = *self.post_delay.lock().unwrap();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            match answer {
                Answer::Queued => Ok(HelperResponse::json(
                    200,
                    br#"{"status":"queued"}"#.to_vec(),
                )),
                Answer::Duplicate => Ok(HelperResponse::json(
                    200,
                    br#"{"status":"duplicate"}"#.to_vec(),
                )),
                Answer::Refused => Ok(HelperResponse::json(
                    400,
                    br#"{"status":"error","error":"invalid round choice"}"#.to_vec(),
                )),
                Answer::Lost => Err(HelperTransportError::Timeout),
                Answer::DelegateNotFound => Ok(HelperResponse::json(
                    422,
                    br#"{"status":"error","error":"delegate_not_found"}"#.to_vec(),
                )),
                Answer::ProxyDisabled => Ok(HelperResponse::json(
                    422,
                    br#"{"status":"error","error":"proxy_delegation_disabled"}"#.to_vec(),
                )),
                Answer::LeafNotFound => Ok(HelperResponse::json(
                    503,
                    br#"{"status":"error","error":"leaf_not_found"}"#.to_vec(),
                )),
            }
        })
    }
}

/// A round whose fixture batch landed, with its executor and helpers.
struct LandedRound {
    fixture: ProvedBatchFixture,
    executor: RoundExecutor<Arc<ScriptedChain>>,
    helpers: Arc<Helpers>,
    host: RoundHostContext,
    /// The device clock the executor reads, the Unix epoch until set.
    wall_clock: Arc<Mutex<Duration>>,
}

impl LandedRound {
    fn db(&self) -> &VotingDb {
        &self.fixture.db
    }

    fn steps(&self) -> Vec<NextStep> {
        self.executor.plan().unwrap().next_steps
    }

    async fn run(&self, step: NextStep) -> crate::RoundStepOutcome {
        self.run_in(step, &self.host).await
    }

    /// Runs `step` with `fleet` as the host's configured helpers.
    async fn run_with_fleet(&self, step: NextStep, fleet: &[String]) -> crate::RoundStepOutcome {
        let host = RoundHostContext {
            configured_helper_urls: fleet.to_vec(),
            ..self.host.clone()
        };
        self.run_in(step, &host).await
    }

    /// Runs `step` with the host clock at `now_seconds`.
    async fn run_at(&self, step: NextStep, now_seconds: u64) -> crate::RoundStepOutcome {
        self.run_in(step, &self.host_at(now_seconds)).await
    }

    /// The host context with its clock at `now_seconds`.
    fn host_at(&self, now_seconds: u64) -> RoundHostContext {
        RoundHostContext {
            now_seconds,
            ..self.host.clone()
        }
    }

    /// The host clock at the authenticated vote end, when the round closes.
    fn vote_end(&self) -> u64 {
        self.host.vote_end_time_seconds.unwrap()
    }

    /// Runs `step` under `host`.
    async fn run_in(&self, step: NextStep, host: &RoundHostContext) -> crate::RoundStepOutcome {
        self.executor
            .advance_step_in_epoch(
                step,
                host,
                &ChainSubmissionControl::new(1),
                1,
                &RecordingProgress::default(),
            )
            .await
            .unwrap()
    }

    async fn fail(&self, step: NextStep) -> crate::RoundStepFailure {
        self.executor
            .advance_step_in_epoch(
                step,
                &self.host,
                &ChainSubmissionControl::new(1),
                1,
                &RecordingProgress::default(),
            )
            .await
            .unwrap_err()
    }

    /// The rows of the DC in `dc_slot`, as `(share, accepted, ambiguous)`.
    fn shares(&self, dc_slot: u8) -> Vec<(i64, usize, usize)> {
        let conn = self.db().conn();
        let mut statement = conn
            .prepare(
                "SELECT share_index, sent_to_urls, ambiguous_urls FROM proxy_dc_shares
                  WHERE dc_slot = ?1 ORDER BY share_index",
            )
            .unwrap();
        statement
            .query_map([i64::from(dc_slot)], |row| {
                let urls = |json: String| serde_json::from_str::<Vec<String>>(&json).unwrap().len();
                Ok((row.get(0)?, urls(row.get(1)?), urls(row.get(2)?)))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

/// A step on the DC in `dc_slot`.
fn submit(dc_slot: u8, share_index: u32) -> NextStep {
    NextStep::SubmitShares {
        bundle_index: 0,
        commitment: CommitmentRef::Proxy { dc_slot },
        share_index,
    }
}

/// Every share step of the plan's DCs, as `(submit?, slot, share)`.
fn dc_steps(steps: &[NextStep]) -> Vec<(bool, u8, u32)> {
    steps
        .iter()
        .filter_map(|step| match *step {
            NextStep::SubmitShares {
                commitment: CommitmentRef::Proxy { dc_slot },
                share_index,
                ..
            } => Some((true, dc_slot, share_index)),
            NextStep::ConfirmShare {
                commitment: CommitmentRef::Proxy { dc_slot },
                share_index,
                ..
            } => Some((false, dc_slot, share_index)),
            _ => None,
        })
        .collect()
}

/// The fixture batch under `layout`, chain-anchored, its executor over a
/// scripted chain and three scripted helpers, and the batch not yet landed.
fn proved_round(layout: DcShareLayout) -> (LandedRound, Arc<ScriptedChain>) {
    let fixture = proved_batch_with(FixtureAnchor::Chain, layout);
    let chain = Arc::new(ScriptedChain::default());
    let helpers = Arc::new(Helpers::default());
    let wall_clock = Arc::new(Mutex::new(Duration::ZERO));
    let executor = bound_executor(
        &fixture.db,
        Arc::clone(&chain),
        scripted_chain_config(),
        HelperClient::new(helpers.clone(), HelperHealth::default()),
        &[(1, 2)],
    )
    .with_wall_clock({
        let wall_clock = Arc::clone(&wall_clock);
        Arc::new(move || *wall_clock.lock().unwrap())
    });
    let host = RoundHostContext {
        configured_helper_urls: Helpers::urls(),
        ..host()
    };
    (
        LandedRound {
            fixture,
            executor,
            helpers,
            host,
            wall_clock,
        },
        chain,
    )
}

/// [`proved_round`] with the batch landed through its own step.
async fn landed_round(layout: DcShareLayout) -> LandedRound {
    let (round, chain) = proved_round(layout);
    land(&round, &chain).await;
    round
}

/// Lands `round`'s proved batch through its own step.
async fn land(round: &LandedRound, chain: &ScriptedChain) {
    let digest = batch_digest(&round.fixture);
    chain.expect("GET", "/v1/height", chain_height(1_000));
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", landed(&round.fixture, digest, FINAL_VAN));
    let outcome = round
        .run(NextStep::AdvanceProxyBatch { bundle_index: 0 })
        .await;
    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    assert_eq!(
        round.executor.plan().unwrap().proxy_batches[0].stage,
        ProxyBatchStage::Landed
    );
}

/// The fixture round's complete ballot list, holding `ballots` as
/// `(delegate_index, options)` in ascending delegate order.
fn ballot_list(ballots: &[(u32, &[u32])]) -> DelegateBallotList {
    let round = BASE64_STANDARD.encode(hex::decode(ROUND_ID).unwrap());
    let records: Vec<_> = ballots
        .iter()
        .map(|(delegate_index, options)| {
            json!({ "vote_round_id": round, "delegate_index": delegate_index, "options": options })
        })
        .collect();
    let mut list = DelegateBallotList::new(ROUND_ID).unwrap();
    list.add_page(json!({ "ballots": records }).to_string().as_bytes())
        .unwrap();
    list
}

/// Each delegation of the status view as `(delegate, ballots, stage,
/// ballot)`, in request order.
fn delegations(
    round: &LandedRound,
    ballots: &DelegateBallotList,
    phase: ProxyRoundPhase,
) -> Vec<(u32, u64, ProxyDelegationStage, DelegateBallotView)> {
    status(round.db(), ROUND_ID, ballots, phase)
        .unwrap()
        .expect("the fixture round has an allocation")
        .delegations
        .into_iter()
        .map(|view| (view.delegate_index, view.ballots, view.stage, view.ballot))
        .collect()
}

/// The delegation job's progress in the active round.
fn job(round: &LandedRound) -> ProxyDelegationProgress {
    progress(
        round.db(),
        ROUND_ID,
        &ballot_list(&[]),
        ProxyRoundPhase::Active,
    )
    .unwrap()
    .unwrap()
}

/// One background tracking pass over the round's shares with the host
/// clock at `now_seconds`, holding the round hotkey when `with_hotkey`.
async fn track(round: &LandedRound, now_seconds: u64, with_hotkey: bool) -> ShareTrackingReport {
    track_pending_shares(
        round.db(),
        &ShareTrackingParams {
            round_id: ROUND_ID,
            configured_server_urls: &round.host.configured_helper_urls,
            now_seconds,
            vote_end_time_seconds: round.host.vote_end_time_seconds,
            policy: ShareTimingPolicy::default(),
            round_hotkey_seed: with_hotkey.then_some(HOTKEY_SECRET.as_slice()),
            random_bytes: &os_random_bytes,
        },
        &HelperClient::new(round.helpers.clone(), HelperHealth::default()),
        &|| false,
    )
    .await
    .unwrap()
}

/// The slot of the fixture batch's DC to `delegate_index`.
fn slot_of(round: &LandedRound, delegate_index: u32) -> u8 {
    round
        .fixture
        .batch
        .dcs
        .iter()
        .find(|dc| dc.delegate_index == delegate_index)
        .unwrap()
        .dc_slot
}

#[tokio::test]
async fn shares_are_owed_only_once_the_batch_lands() {
    let (round, _chain) = proved_round(DcShareLayout::Standard);
    assert!(dc_steps(&round.steps()).is_empty());
    let outcome = round.run(submit(0, 0)).await;
    assert_eq!(outcome.disposition, RoundStepDisposition::NoWork);
    assert!(round.helpers.posts().is_empty());

    let round = landed_round(DcShareLayout::Standard).await;

    let mut owed: Vec<_> = (0..16).map(|share| (true, 0, share)).collect();
    owed.extend((0..16).map(|share| (true, 1, share)));
    assert_eq!(dc_steps(&round.steps()), owed);
}

#[tokio::test]
async fn a_dc_is_planned_before_its_first_post_and_delivered_into_its_delegate_s_pool() {
    let round = landed_round(DcShareLayout::Standard).await;
    {
        let db = Arc::clone(&round.fixture.db);
        *round.helpers.before_post.lock().unwrap() = Some(Box::new(move || {
            let planned: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM proxy_dc_shares WHERE dc_slot = 0",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(planned, 16, "every share is planned before the first POST");
        }));
    }

    let outcome = round.run(submit(0, 0)).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    let delegate = round.fixture.batch.dcs[0].delegate_index;
    let posts = round.helpers.posts();
    assert_eq!(posts.len(), 32, "16 shares, each to 2 of 3 helpers");
    for (_, body) in &posts {
        assert_eq!(body["proposal_id"], POOL_PROPOSAL_ID);
        assert_eq!(body["vote_decision"], delegate);
        assert_eq!(body["tree_position"], FINAL_VAN + 1);
        let submit_at = body["submit_at"].as_u64().unwrap();
        assert!(submit_at >= round.host.now_seconds, "the standard schedule");
    }
    assert!(round
        .shares(0)
        .iter()
        .all(|&(_, accepted, _)| accepted == 2));
    let mut owed: Vec<_> = (0..16).map(|share| (true, 1, share)).collect();
    owed.extend((0..16).map(|share| (false, 0, share)));
    assert_eq!(
        dc_steps(&round.steps()),
        owed,
        "DC 1 still owes its shares; DC 0's are handed off to tracking"
    );
}

/// A DC planned at once sends share 0 alone with `submit_at = 0`, and once
/// it is handed off the round leaves its reveal to background tracking,
/// which confirms it.
#[tokio::test]
async fn a_single_share_dc_sends_share_zero_at_once_and_tracking_confirms_its_reveal() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    assert_eq!(dc_steps(&round.steps()), vec![(true, 0, 0), (true, 1, 0)]);

    assert_eq!(
        round.run(submit(1, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );

    let posts = round.helpers.posts();
    assert_eq!(posts.len(), 2, "share 0 alone, to ceil(3 / 2) helpers");
    assert!(posts.iter().all(|(_, body)| body["submit_at"] == 0
        && body["share_index"] == 0
        && body["tree_position"] == FINAL_VAN + 2));

    let host = round.host.clone();
    let report = RoundDriver::new(&round.executor)
        .run(
            &RoundHostSourceBridge::new(move || host.clone()),
            &ChainSubmissionControl::new(1),
            &NoopRoundDriveReporter::default(),
        )
        .await;
    let RoundQuiescence::BackgroundShareWorkOnly { shares } = report.quiescence else {
        panic!("{:?}", report.quiescence);
    };
    let share_zeros = vec![ShareKey::proxy(0, 0, 0), ShareKey::proxy(0, 1, 0)];
    assert_eq!(
        shares, share_zeros,
        "the run delivers DC 0 and polls neither reveal"
    );

    round.helpers.revealed.store(true, Ordering::SeqCst);
    let ready = round.host.now_seconds + ShareTimingPolicy::default().status_check_grace_seconds;
    assert_eq!(track(&round, ready, true).await.confirmed, share_zeros);
    assert!(dc_steps(&round.steps()).is_empty());
}

#[tokio::test]
async fn a_share_short_of_its_target_is_owed_again_until_handed_off() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round.helpers.script(&urls[0], &[Answer::Refused]);
    round.helpers.script(&urls[1], &[Answer::Refused]);
    round.helpers.script(&urls[2], &[Answer::Refused]);

    let failure = round.fail(submit(0, 0)).await;

    assert_eq!(failure.kind, RoundStepFailureKind::HelperDeliveryIncomplete);
    assert_eq!(round.shares(0), vec![(0, 0, 0)]);
    assert!(dc_steps(&round.steps()).contains(&(true, 0, 0)));

    // A definite refusal leaves the helper eligible on a later pass.
    let before = round.helpers.posts().len();
    assert_eq!(
        round.run(submit(0, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    assert_eq!(round.shares(0), vec![(0, 2, 0)]);
    let posts = round.helpers.posts();
    assert_eq!(
        posts[before].1, posts[0].1,
        "the retry posts the share the first pass derived"
    );
}

/// A helper's 422 refusal of a pool share (contract §11.8) is an ordinary
/// definite failure of that helper: with a target of two, the share goes to
/// the fallback helper and is handed off.
#[tokio::test]
async fn a_refusing_helper_is_replaced_and_the_share_handed_off() {
    use Answer::{DelegateNotFound, ProxyDisabled};
    for script in [&[DelegateNotFound][..], &[ProxyDisabled][..]] {
        let round = landed_round(DcShareLayout::SingleShare).await;
        // The first POST goes to one of the share's two planned helpers.
        round.helpers.script_next_posts(script);
        let dc_slot = slot_of(&round, 17);

        assert_eq!(
            round.run(submit(dc_slot, 0)).await.disposition,
            RoundStepDisposition::Advanced,
            "{script:?}"
        );
        let posted_to = round.helpers.posted_to();
        assert_eq!(
            posted_to.len(),
            3,
            "{script:?}: the fallback helper is tried"
        );
        assert_eq!(round.shares(dc_slot), vec![(0, 2, 0)], "{script:?}");
        let steps = dc_steps(&round.steps());
        assert!(
            !steps.contains(&(true, dc_slot, 0)) && steps.contains(&(false, dc_slot, 0)),
            "{script:?}: only share 0's reveal is owed"
        );
        assert_eq!(
            delegations(&round, &ballot_list(&[]), ProxyRoundPhase::Active)[0].2,
            ProxyDelegationStage::HandedOff,
            "{script:?}"
        );
    }
}

/// A DC whose share every helper keeps refusing is never handed off. Once
/// the vote ends its delivery is no longer owed, so a step on it does no
/// work and posts nothing, and once the round closes it is not counted.
#[tokio::test]
async fn a_dc_every_helper_refuses_is_not_counted_after_close() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    for url in Helpers::urls() {
        round.helpers.script(&url, &[Answer::DelegateNotFound]);
    }
    let dc_slot = slot_of(&round, 17);
    assert_eq!(
        round.fail(submit(dc_slot, 0)).await.kind,
        RoundStepFailureKind::HelperDeliveryIncomplete
    );
    assert_eq!(round.helpers.posts().len(), 3, "every helper is tried");

    let ended = round.run_at(submit(dc_slot, 0), round.vote_end()).await;
    assert_eq!(ended.disposition, RoundStepDisposition::NoWork);
    assert!(dc_steps(&ended.plan.next_steps).is_empty());
    assert_eq!(round.helpers.posts().len(), 3, "nothing is posted after it");
    assert_eq!(
        delegations(&round, &ballot_list(&[]), ProxyRoundPhase::Closed)[0].2,
        ProxyDelegationStage::NotCounted
    );
}

/// A driver run at the vote end stops owing a short DC's delivery: the one
/// step planned before it sampled the host clock does no work, and the run
/// ends without posting or retrying.
#[tokio::test]
async fn a_run_at_the_vote_end_stops_delivering_a_short_dc() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    for url in Helpers::urls() {
        round.helpers.script(&url, &[Answer::LeafNotFound]);
    }
    let dc_slot = slot_of(&round, 17);
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    let posts = round.helpers.posts().len();

    let ended = round.host_at(round.vote_end());
    let report = crate::RoundDriver::new(&round.executor)
        .run(
            &crate::RoundHostSourceBridge::new(|| ended.clone()),
            &ChainSubmissionControl::new(1),
            &crate::NoopRoundDriveReporter {},
        )
        .await;

    assert!(
        !matches!(
            report.quiescence,
            crate::RoundQuiescence::PassBudgetExhausted { .. }
        ),
        "{:?}",
        report.quiescence
    );
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(dc_steps(&report.plan.unwrap().next_steps).is_empty());
    assert_eq!(round.helpers.posts().len(), posts);
}

/// A pass whose helper preflight ends at the vote end plans and posts
/// nothing, though the host clock read before it. The pass is `Pending`, and
/// the next plan, past the vote end, owes no delivery.
#[tokio::test(start_paused = true)]
async fn a_pass_whose_preflight_crosses_the_vote_end_posts_nothing() {
    let round = landed_round(DcShareLayout::Standard).await;
    *round.helpers.status_delay.lock().unwrap() = Duration::from_secs(5);

    let outcome = round.run_at(submit(0, 0), round.vote_end() - 2).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Pending);
    assert!(round.helpers.posts().is_empty());
    assert!(round.shares(0).is_empty(), "nothing is planned");
}

/// The host's whole-second sample is read as late as it may be: one a
/// second before the vote end may be 999 ms behind, so a preflight of half
/// a second ends past the vote end and nothing is posted.
#[tokio::test(start_paused = true)]
async fn a_whole_second_host_sample_is_read_as_late_as_it_may_be() {
    let round = landed_round(DcShareLayout::Standard).await;
    *round.helpers.status_delay.lock().unwrap() = Duration::from_millis(500);

    let outcome = round.run_at(submit(0, 0), round.vote_end() - 1).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Pending);
    assert!(round.helpers.posts().is_empty());
}

/// The device clock counts the time between the host's sample and the
/// pass: a host sample two seconds before the vote end, taken before a delay
/// that puts the device clock 100 ms past it, posts nothing.
#[tokio::test(start_paused = true)]
async fn a_delay_before_the_pass_is_counted_by_the_device_clock() {
    let round = landed_round(DcShareLayout::Standard).await;
    *round.wall_clock.lock().unwrap() =
        Duration::from_secs(round.vote_end()) + Duration::from_millis(100);

    let outcome = round.run_at(submit(0, 0), round.vote_end() - 2).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Pending);
    assert!(round.helpers.posts().is_empty());
}

/// A POST in flight at the vote end keeps its outcome, and no new POST
/// starts after it: the helper that refused share 0 is not replaced.
#[tokio::test(start_paused = true)]
async fn a_post_in_flight_at_the_vote_end_keeps_its_outcome_and_starts_no_other() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    round.helpers.script_next_posts(&[Answer::Refused]);
    *round.helpers.post_delay.lock().unwrap() = Duration::from_secs(5);
    let dc_slot = slot_of(&round, 17);

    let outcome = round.run_at(submit(dc_slot, 0), round.vote_end() - 2).await;

    assert_eq!(outcome.disposition, RoundStepDisposition::Pending);
    assert_eq!(round.helpers.posts().len(), 2, "share 0's planned helpers");
    assert_eq!(round.shares(dc_slot), vec![(0, 1, 0)]);
}

/// A helper that refuses a reconciling re-post keeps its earlier unknown
/// outcome, and the next pass re-posts to it again.
#[tokio::test]
async fn a_refused_re_post_keeps_the_earlier_unknown_outcome() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round.helpers.script(
        &urls[0],
        &[Answer::Lost, Answer::DelegateNotFound, Answer::Duplicate],
    );
    round
        .helpers
        .script(&urls[1], &[Answer::Lost, Answer::Refused, Answer::Refused]);
    round
        .helpers
        .script(&urls[2], &[Answer::Lost, Answer::Duplicate]);
    let dc_slot = slot_of(&round, 17);
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    assert_eq!(round.shares(dc_slot), vec![(0, 0, 3)]);

    // Every lost helper is re-posted once: one refuses, one fails and one
    // answers duplicate.
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    assert_eq!(round.shares(dc_slot), vec![(0, 1, 2)]);

    let posts = round.helpers.posts().len();
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    assert!(round.helpers.posted_to()[posts..].contains(&urls[0]));
    assert_eq!(round.shares(dc_slot), vec![(0, 2, 1)]);
}

/// `leaf_not_found` (503) means the DC is not yet in that helper's tree. It
/// is read as any 5xx is: the outcome is unknown, so the step is pending
/// rather than failed, and once the helpers catch up a later pass's re-post
/// hands the share off.
#[tokio::test]
async fn a_dc_not_yet_in_the_helpers_trees_is_retried_later() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    for url in Helpers::urls() {
        round.helpers.script(&url, &[Answer::LeafNotFound]);
    }
    let dc_slot = slot_of(&round, 17);

    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    assert_eq!(round.helpers.posts().len(), 3, "every helper is tried");
    assert_eq!(round.shares(dc_slot), vec![(0, 0, 3)]);
    assert!(dc_steps(&round.steps()).contains(&(true, dc_slot, 0)));

    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    assert_eq!(round.shares(dc_slot), vec![(0, 2, 1)]);
}

/// A configured fleet that shrinks below a plan's target is handled as a
/// vote share's placement is: the pass aims for the target capped by the
/// fleet's size, counts only configured helpers, and records the share
/// handed off once that many hold it, so the DC is not owed shares forever.
/// The record survives the fleet growing back.
#[tokio::test]
async fn a_fleet_that_shrinks_below_the_plan_s_target_hands_the_share_off() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round.helpers.script(&urls[1], &[Answer::Refused]);
    round.helpers.script(&urls[2], &[Answer::Refused]);
    let dc_slot = slot_of(&round, 17);
    let failure = round.fail(submit(dc_slot, 0)).await;
    assert_eq!(failure.kind, RoundStepFailureKind::HelperDeliveryIncomplete);
    assert_eq!(round.shares(dc_slot), vec![(0, 1, 0)], "one of two placed");
    let posts = round.helpers.posts().len();

    let shrunk = [urls[0].clone()];
    assert_eq!(
        round
            .run_with_fleet(submit(dc_slot, 0), &shrunk)
            .await
            .disposition,
        RoundStepDisposition::Advanced
    );
    assert_eq!(round.helpers.posts().len(), posts, "nothing more to post");
    let handed_off = |steps: &[NextStep]| {
        let steps = dc_steps(steps);
        !steps.contains(&(true, dc_slot, 0)) && steps.contains(&(false, dc_slot, 0))
    };
    assert!(handed_off(&round.steps()), "only share 0's reveal is owed");
    assert_eq!(
        delegations(&round, &ballot_list(&[]), ProxyRoundPhase::Active)[0].2,
        ProxyDelegationStage::HandedOff
    );

    // Once recorded, the hand-off stands when the fleet grows back.
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::NoWork
    );
    assert!(handed_off(&round.steps()));
}

/// A helper removed from the fleet never counts toward the effective
/// target: shrunk to a helper that does not hold the share, the pass posts
/// to it and the share stays owed until a configured helper holds it.
#[tokio::test]
async fn a_removed_helper_does_not_count_toward_a_shrunk_fleet_s_target() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round
        .helpers
        .script(&urls[1], &[Answer::Refused, Answer::Refused]);
    round.helpers.script(&urls[2], &[Answer::Refused]);
    let dc_slot = slot_of(&round, 17);
    round.fail(submit(dc_slot, 0)).await;
    assert_eq!(round.shares(dc_slot), vec![(0, 1, 0)]);

    let shrunk = [urls[1].clone()];
    let host = RoundHostContext {
        configured_helper_urls: shrunk.to_vec(),
        ..round.host.clone()
    };
    let failure = round
        .executor
        .advance_step_in_epoch(
            submit(dc_slot, 0),
            &host,
            &ChainSubmissionControl::new(1),
            1,
            &RecordingProgress::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, RoundStepFailureKind::HelperDeliveryIncomplete);
    assert!(dc_steps(&round.steps()).contains(&(true, dc_slot, 0)));

    assert_eq!(
        round
            .run_with_fleet(submit(dc_slot, 0), &shrunk)
            .await
            .disposition,
        RoundStepDisposition::Advanced,
        "the remaining helper accepts on a later pass"
    );
}

/// A share's helpers that left the configured fleet never hand it off. Two
/// helpers' acceptances filled its target of two, but no pass recorded the
/// hand-off (a crash between the acceptances and the record), and the fleet
/// then lost both: the share is still owed, goes to the configured helper,
/// and is handed off once that helper, the shrunk fleet's reachable target,
/// holds it.
#[tokio::test]
async fn helpers_removed_from_the_fleet_do_not_hand_a_share_off() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    for url in &urls {
        round.helpers.script(url, &[Answer::Refused]);
    }
    let dc_slot = slot_of(&round, 17);
    round.fail(submit(dc_slot, 0)).await;
    assert_eq!(
        round.shares(dc_slot),
        vec![(0, 0, 0)],
        "planned, held by none"
    );
    round
        .db()
        .conn()
        .execute(
            "UPDATE proxy_dc_shares SET sent_to_urls = ?1",
            [serde_json::to_string(&urls[..2]).unwrap()],
        )
        .unwrap();

    assert!(
        dc_steps(&round.steps()).contains(&(true, dc_slot, 0)),
        "no pass recorded the two acceptances as a hand-off"
    );
    let posts = round.helpers.posts().len();
    let shrunk = [urls[2].clone()];
    assert_eq!(
        round
            .run_with_fleet(submit(dc_slot, 0), &shrunk)
            .await
            .disposition,
        RoundStepDisposition::Advanced
    );
    assert_eq!(round.helpers.posted_to()[posts..], shrunk[..]);
    assert!(!dc_steps(&round.steps()).contains(&(true, dc_slot, 0)));
}

/// An unknown outcome at a helper that left the fleet is never reconciled,
/// since no pass posts there again, so it doesn't hold the step pending:
/// once every configured helper refuses, the share is short and the step
/// fails.
#[tokio::test]
async fn an_unknown_outcome_at_a_removed_helper_does_not_hide_a_short_share() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round.helpers.script(&urls[0], &[Answer::Lost]);
    for url in &urls[1..] {
        round
            .helpers
            .script(url, &[Answer::Refused, Answer::Refused]);
    }
    let dc_slot = slot_of(&round, 17);
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    assert_eq!(round.shares(dc_slot), vec![(0, 0, 1)]);

    let host = RoundHostContext {
        configured_helper_urls: urls[1..].to_vec(),
        ..round.host.clone()
    };
    let failure = round
        .executor
        .advance_step_in_epoch(
            submit(dc_slot, 0),
            &host,
            &ChainSubmissionControl::new(1),
            1,
            &RecordingProgress::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, RoundStepFailureKind::HelperDeliveryIncomplete);
}

#[tokio::test]
async fn an_ambiguous_helper_is_reconciled_by_a_duplicate_safe_re_post() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let urls = Helpers::urls();
    round
        .helpers
        .script(&urls[0], &[Answer::Lost, Answer::Duplicate]);
    round
        .helpers
        .script(&urls[1], &[Answer::Lost, Answer::Refused]);
    round
        .helpers
        .script(&urls[2], &[Answer::Lost, Answer::Duplicate]);

    // Every helper's answer is lost, so none definitely holds the share and
    // only reconciliation can tell; the pass waits for it.
    assert_eq!(
        round.run(submit(0, 0)).await.disposition,
        RoundStepDisposition::Pending
    );
    assert_eq!(round.shares(0), vec![(0, 0, 3)]);

    assert_eq!(
        round.run(submit(0, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    assert_eq!(
        round.shares(0),
        vec![(0, 2, 1)],
        "two helpers answered duplicate; the refusal says nothing about the original"
    );
}

#[tokio::test]
async fn the_status_view_follows_each_delegation_from_preparing_to_handed_off() {
    use DelegateBallotView::{Counted, NoBallotYet, NotCounted, Submitted};
    use ProxyDelegationStage::{HandedOff, Included, Preparing};
    let (round, chain) = proved_round(DcShareLayout::Standard);
    let none = ballot_list(&[]);
    let active = ProxyRoundPhase::Active;

    // Committed and proved, nothing sent: 23% and 17% of 100 ballots.
    let committed = status(round.db(), ROUND_ID, &none, active)
        .unwrap()
        .unwrap();
    assert_eq!(committed.kept_ballots, 60);
    assert_eq!(
        delegations(&round, &none, active),
        [
            (17, 23, Preparing, NoBallotYet),
            (5, 17, Preparing, NoBallotYet)
        ]
    );
    assert!(!job(&round).broadcast_started);

    land(&round, &chain).await;
    assert_eq!(
        delegations(&round, &none, active),
        [
            (17, 23, Included, NoBallotYet),
            (5, 17, Included, NoBallotYet)
        ]
    );
    let landed = job(&round);
    assert!(landed.broadcast_started);
    assert!(!landed.planned_immediate && !landed.refused_by_chain);

    assert_eq!(
        round.run(submit(slot_of(&round, 17), 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    let published = ballot_list(&[(5, &[1]), (17, &[0])]);
    assert_eq!(
        delegations(&round, &published, active),
        [
            (17, 23, HandedOff, Submitted { options: vec![0] }),
            (5, 17, Included, Submitted { options: vec![1] })
        ]
    );

    // After the tally transition only the handed-off DC counts, and each
    // ballot that landed was counted.
    let closed = ProxyRoundPhase::Closed;
    assert_eq!(
        delegations(&round, &published, closed),
        [
            (17, 23, HandedOff, Counted { options: vec![0] }),
            (
                5,
                17,
                ProxyDelegationStage::NotCounted,
                Counted { options: vec![1] }
            )
        ]
    );
    assert_eq!(
        delegations(&round, &ballot_list(&[(17, &[0])]), closed)[1].3,
        NotCounted,
        "delegate 5 published no ballot before the tally"
    );
}

#[tokio::test]
async fn a_dc_planned_at_once_is_handed_off_before_its_reveal_which_completes_the_job() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let none = ballot_list(&[]);
    assert!(!job(&round).planned_immediate, "nothing is planned yet");
    for dc_slot in [0, 1] {
        assert_eq!(
            round.run(submit(dc_slot, 0)).await.disposition,
            RoundStepDisposition::Advanced
        );
    }

    let handed_off = job(&round);
    assert!(handed_off.planned_immediate);
    assert!(!handed_off.immediate_reveal_confirmed);
    assert!(!handed_off.is_complete());
    assert!(handed_off
        .delegations
        .iter()
        .all(|view| view.stage == ProxyDelegationStage::HandedOff));
    assert!(
        delegations(&round, &none, ProxyRoundPhase::Closed)
            .iter()
            .all(|(_, _, stage, _)| *stage == ProxyDelegationStage::HandedOff),
        "the round closing before the reveal is confirmed still counts the hand-off"
    );

    round.helpers.revealed.store(true, Ordering::SeqCst);
    let ready = round.host.now_seconds + ShareTimingPolicy::default().status_check_grace_seconds;
    assert_eq!(track(&round, ready, true).await.confirmed.len(), 2);
    let revealed = job(&round);
    assert!(revealed.immediate_reveal_confirmed);
    assert!(revealed.is_complete());
}

/// A delayed share every helper holding it drops is not lost: once it is
/// overdue, background tracking derives it again from the round hotkey and
/// sends it at once to a helper that does not hold it, and a later pass
/// confirms its reveal through the helper quorum.
#[tokio::test]
async fn an_overdue_handed_off_dc_share_is_resent_at_once_and_confirmed_later() {
    let round = landed_round(DcShareLayout::Standard).await;
    let dc_slot = slot_of(&round, 17);
    assert_eq!(
        round.run(submit(dc_slot, 0)).await.disposition,
        RoundStepDisposition::Advanced
    );
    let delivered = round.helpers.posts();
    let overdue = round.vote_end() - 20;

    let resent = track(&round, overdue, true).await;

    let keys: Vec<ShareKey> = (0..16)
        .map(|share| ShareKey::proxy(0, dc_slot, share))
        .collect();
    let resent_keys: Vec<ShareKey> = resent.resubmitted.iter().map(|sent| sent.share).collect();
    assert_eq!(resent_keys, keys, "one resend per share");
    let resends = &round.helpers.posts()[delivered.len()..];
    assert_eq!(resends.len(), 16);
    for (helper, body) in resends {
        assert_eq!(body["submit_at"], 0);
        assert_eq!(body["proposal_id"], POOL_PROPOSAL_ID);
        assert_eq!(body["vote_decision"], 17);
        assert!(
            !delivered
                .iter()
                .any(|(holder, first)| holder == helper
                    && first["share_index"] == body["share_index"]),
            "a share goes to a helper that does not hold it yet"
        );
    }
    assert!(resent.confirmed.is_empty() && resent.next_delay_seconds.is_some());

    round.helpers.revealed.store(true, Ordering::SeqCst);
    let confirmed = track(&round, overdue, true).await;
    assert_eq!(confirmed.confirmed, keys);
    assert_eq!(confirmed.next_delay_seconds, None);
    assert_eq!(round.helpers.posts().len(), delivered.len() + 16);
}

/// Without the round hotkey a pass cannot derive a DC share, so it polls
/// and confirms an overdue one but never resends it.
#[tokio::test]
async fn without_the_hotkey_tracking_confirms_dc_shares_but_never_resends_one() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    for dc_slot in [0, 1] {
        round.run(submit(dc_slot, 0)).await;
    }
    let delivered = round.helpers.posts().len();
    let overdue = round.vote_end() - 20;

    let polled = track(&round, overdue, false).await;
    assert!(polled.resubmitted.is_empty() && polled.unrecoverable.is_empty());
    assert_eq!(round.helpers.posts().len(), delivered, "nothing is resent");

    round.helpers.revealed.store(true, Ordering::SeqCst);
    let confirmed = track(&round, overdue, false).await;
    assert_eq!(
        confirmed.confirmed,
        vec![ShareKey::proxy(0, 0, 0), ShareKey::proxy(0, 1, 0)]
    );
    assert_eq!(round.helpers.posts().len(), delivered);
}

/// A round stays pending for background tracking, and each pass asks for
/// another, while a handed-off DC share is unconfirmed.
#[tokio::test]
async fn tracking_continues_while_a_handed_off_dc_share_is_unconfirmed() {
    let round = landed_round(DcShareLayout::SingleShare).await;
    let pending = || pending_rounds_for_accounts(round.db(), &[WALLET_ID]).unwrap();
    assert!(pending().is_empty(), "nothing is handed off yet");

    round.run(submit(slot_of(&round, 17), 0)).await;
    assert_eq!(pending()[0].round_id, ROUND_ID);
    let waiting = track(&round, round.host.now_seconds, true).await;
    assert_eq!(waiting.unconfirmed_at_entry, Some(1));
    assert!(waiting.next_delay_seconds.is_some());

    round.helpers.revealed.store(true, Ordering::SeqCst);
    let ready = round.host.now_seconds + ShareTimingPolicy::default().status_check_grace_seconds;
    assert_eq!(track(&round, ready, true).await.next_delay_seconds, None);
    assert!(pending().is_empty());
}

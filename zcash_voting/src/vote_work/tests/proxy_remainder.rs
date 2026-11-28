//! A round driven after its proxy batch lands with the bundle's whole weight
//! delegated (plan CF-2): the driver never stops for a ballot, casts nothing
//! whatever the intents say, and sends no vote share. The planning
//! rules are in `round_planning/tests/proxy_remainder.rs` and the VAN's
//! arithmetic in `storage/tests/remaining_weight.rs`.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use super::fixtures::{host, RecordingProgress};
use crate::wire::ProxyDelegateChoice;
use crate::{
    chain_submission::tests::scripted_chain::{accepted, chain_height, landed, ScriptedChain},
    helper::transport::{HelperFuture, HelperResponse, HelperTransport},
    proxy::{
        tests::submission_fixtures::{
            batch_digest, bound_executor, delegate_choice, proved_batch_for, scripted_chain_config,
            FixtureAnchor, ProvedBatchFixture,
        },
        DcShareLayout,
    },
    round_drive::{NoopRoundDriveReporter, RoundDriver, RoundHostSourceBridge, RoundQuiescence},
    session::{Decision, NextStep, ProxyBatchStage, RoundPlan},
    BallotIntent, ChainSubmissionControl, HelperClient, HelperHealth, RoundExecutor,
    RoundHostContext, RoundStepDisposition,
};

/// The leaf the fixture batch's final VAN lands at; its DCs follow.
const FINAL_VAN: u64 = 9;

/// A helper fleet that queues every share, reports every share revealed, and
/// records the proposal each share POST names.
#[derive(Default)]
struct Helpers {
    proposals: Mutex<Vec<serde_json::Value>>,
}

impl Helpers {
    /// The fleet's base URLs.
    fn urls() -> Vec<String> {
        (1..=3)
            .map(|index| format!("https://helper-{index}.example"))
            .collect()
    }
}

impl HelperTransport for Helpers {
    fn get<'a>(&'a self, url: &'a str, _timeout: Duration) -> HelperFuture<'a> {
        let status = if url.contains("/share-status/") {
            "confirmed"
        } else {
            "ok"
        };
        Box::pin(async move {
            Ok(HelperResponse::json(
                200,
                format!(r#"{{"status":"{status}"}}"#).into_bytes(),
            ))
        })
    }

    fn post_json<'a>(
        &'a self,
        _url: &'a str,
        body: Vec<u8>,
        _timeout: Duration,
    ) -> HelperFuture<'a> {
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        self.proposals
            .lock()
            .unwrap()
            .push(body["proposal_id"].clone());
        Box::pin(async {
            Ok(HelperResponse::json(
                200,
                br#"{"status":"queued"}"#.to_vec(),
            ))
        })
    }
}

/// A round whose proxy batch landed, with its executor, chain and helpers.
struct LandedRound {
    fixture: ProvedBatchFixture,
    executor: RoundExecutor<Arc<ScriptedChain>>,
    chain: Arc<ScriptedChain>,
    helpers: Arc<Helpers>,
    host: RoundHostContext,
}

impl LandedRound {
    /// The round's plan.
    fn plan(&self) -> RoundPlan {
        self.executor.plan().unwrap()
    }

    /// Records a choice for both roster proposals and returns the plan.
    fn decide(&self) -> RoundPlan {
        self.executor
            .set_ballot_intents(&[
                BallotIntent {
                    proposal_id: 1,
                    decision: Decision::Choice(0),
                },
                BallotIntent {
                    proposal_id: 2,
                    decision: Decision::Choice(1),
                },
            ])
            .unwrap()
    }

    /// Drives the round until it stops, and says why.
    async fn drive(&self) -> RoundQuiescence {
        let host = self.host.clone();
        let source = RoundHostSourceBridge::new(move || host.clone());
        let report = RoundDriver::new(&self.executor)
            .run(
                &source,
                &ChainSubmissionControl::new(1),
                &NoopRoundDriveReporter::default(),
            )
            .await;
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        report.quiescence
    }
}

/// The fixture bundle's 100 ballots allocated by `request`, its batch proved,
/// submitted and landed through its own step.
async fn landed_round(request: Vec<ProxyDelegateChoice>) -> LandedRound {
    let fixture = proved_batch_for(FixtureAnchor::Chain, DcShareLayout::Standard, request);
    let chain = Arc::new(ScriptedChain::default());
    let helpers = Arc::new(Helpers::default());
    let executor = bound_executor(
        &fixture.db,
        Arc::clone(&chain),
        scripted_chain_config(),
        HelperClient::new(helpers.clone(), HelperHealth::default()),
        &[(1, 2), (2, 3)],
    );
    let round = LandedRound {
        fixture,
        executor,
        chain,
        helpers,
        host: RoundHostContext {
            configured_helper_urls: Helpers::urls(),
            ..host()
        },
    };

    let digest = batch_digest(&round.fixture);
    round.chain.expect("GET", "/v1/height", chain_height(1_000));
    round
        .chain
        .expect("POST", "/proxy-delegation-batch", accepted(digest));
    round
        .chain
        .expect("GET", "/tx/", landed(&round.fixture, digest, FINAL_VAN));
    let outcome = round
        .executor
        .advance_step_in_epoch(
            NextStep::AdvanceProxyBatch { bundle_index: 0 },
            &round.host,
            &ChainSubmissionControl::new(1),
            1,
            &RecordingProgress::default(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.disposition, RoundStepDisposition::Advanced);
    assert_eq!(round.plan().proxy_batches[0].stage, ProxyBatchStage::Landed);
    round.chain.assert_drained();
    round
}

/// One delegate for the round's whole weight.
fn delegate_everything() -> Vec<ProxyDelegateChoice> {
    vec![delegate_choice(17, 10_000)]
}

/// Every `(bundle, proposal)` the plan casts.
fn casts(plan: &RoundPlan) -> Vec<(u32, u32)> {
    plan.next_steps
        .iter()
        .filter_map(|step| match *step {
            NextStep::CastVote {
                bundle_index,
                proposal_id,
                ..
            } => Some((bundle_index, proposal_id)),
            _ => None,
        })
        .collect()
}

/// The driver never stops for a ballot, recorded choices plan no cast, and
/// nothing but the batch reaches the chain, and nothing reaches a helper.
#[tokio::test]
async fn delegating_everything_never_asks_for_a_ballot_or_casts() {
    let round = landed_round(delegate_everything()).await;
    assert!(round.plan().nothing_left_to_vote);
    assert!(
        matches!(round.drive().await, RoundQuiescence::NoWorkLeft),
        "the run does not stop for a ballot"
    );

    let plan = round.decide();
    assert_eq!(casts(&plan), Vec::new(), "the intents are ignored");
    assert!(matches!(round.drive().await, RoundQuiescence::NoWorkLeft));

    assert_eq!(round.chain.posts(), 1, "the batch is the round's only POST");
    assert!(
        round.helpers.proposals.lock().unwrap().is_empty(),
        "no share is sent"
    );
}

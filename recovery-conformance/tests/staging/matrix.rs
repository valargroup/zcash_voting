//! Driving every crash stage against staging, in order, and judging the result.

use std::time::{Duration, Instant};

use recovery_conformance::assertions::{
    assert_confirmed_by_a_legal_route, assert_idempotent, assert_matches_control,
    assert_no_second_generation, assert_other_bundles_untouched, assert_plans_precede_broadcast,
    assert_recovered_the_same_transaction, assert_reservations_monotonic, assert_stage_state,
    assert_terminal_rows_unchanged, assert_untouched_bundles_did_not_reserve, confirmation_source,
    confirmed_transaction_hash, deterministic_plan, dispatched_transaction_hash, DurableSnapshot,
};
use recovery_conformance::child::{run_to_quiescence, run_until_crash};
use recovery_conformance::round_run::{default_target, proposal_ids};
use recovery_conformance::run_config::RunMode;
use recovery_conformance::setup_preservation::{
    assert_a_host_reset_preserved_crash_state, assert_delegation_setup_preserved,
};
use recovery_conformance::CrashStage;

#[path = "fixture.rs"]
mod fixture;

use fixture::{
    build_control, config_for, fixture_account, prepare, provision, warm_from, Faults, Fixture,
    ProvisionedRound,
};

/// How long one stage may take before it is abandoned.
///
/// Generous: a full drive to quiescence proves three delegations and nine
/// votes, and a vote proof takes minutes. The budget exists so a wedged run
/// fails the matrix rather than hanging it.
const STAGE_BUDGET: Duration = Duration::from_secs(45 * 60);

/// How long a dispatched transaction is given to reach a block.
///
/// Only the stages whose premise is "the chain already holds this" wait. Long
/// enough for inclusion on `svote-1`, and paid once by two stages rather than
/// by the whole matrix.
const CHAIN_INCLUSION_WAIT: Duration = Duration::from_secs(45);

/// Dispatch ceiling for one drive, so a plan that never shrinks ends the run.
///
/// Sized from the work a resume can actually owe, because the ceiling is a
/// livelock detector and a ceiling below the honest maximum turns every slow
/// convergence into a false positive. The round carries 3 bundles x 3 proposals
/// x 16 shares = 144 shares. A stage crashed at the first share POST resumes
/// owing every one of them: one dispatch to deliver each, then one per
/// confirmation poll, and a helper quorum routinely needs several polls before
/// it answers. At 512 that stage exhausted the budget with all 144 shares still
/// unconfirmed while the round was in fact converging.
///
/// Ten dispatches per share leaves room for delivery plus a long confirmation
/// tail. A plan that genuinely never shrinks still ends the run, just later.
const MAX_DISPATCHES: usize = 144 * 10;

pub enum Run {
    Skipped(String),
    Completed(Report),
}

pub struct Report {
    pub attempted: usize,
    /// Case labels rather than stages: the matrix now runs crash-during-recovery
    /// sequences alongside single stages, and a control failure is its own case
    /// rather than a stage's.
    pub passed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub skipped: Vec<(String, String)>,
}

impl Report {
    pub fn print(&self) {
        eprintln!("\n=== staging conformance ===");
        for stage in &self.passed {
            eprintln!("  PASS  {stage}");
        }
        for (stage, why) in &self.skipped {
            eprintln!("  SKIP  {stage}: {why}");
        }
        for (stage, why) in &self.failed {
            eprintln!("  FAIL  {stage}: {why}");
        }
        eprintln!(
            "  {} passed, {} failed, {} skipped, of {} attempted",
            self.passed.len(),
            self.failed.len(),
            self.skipped.len(),
            self.attempted
        );
    }
}

pub fn run() -> Run {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return Run::Skipped(format!("no tokio runtime: {error}")),
    };
    match runtime.block_on(prepare()) {
        Err(reason) => Run::Skipped(reason),
        Ok(fixture) => Run::Completed(runtime.block_on(drive_matrix(fixture))),
    }
}

async fn drive_matrix(fixture: Fixture) -> Report {
    let mut report = Report {
        attempted: 0,
        passed: Vec::new(),
        failed: Vec::new(),
        skipped: Vec::new(),
    };

    // The control comes first: every terminal comparison is against it, so a
    // matrix without one proves only that crashes converge somewhere.
    let control = match build_control(&fixture, MAX_DISPATCHES, &Faults::none()).await {
        Ok(control) => control,
        Err(error) => {
            report.failed.push((
                "control".to_string(),
                format!("control run failed: {error:#}"),
            ));
            report.attempted = 1;
            return report;
        }
    };
    eprintln!("control terminal snapshot: {:?}", control.states());

    // Each dimension has its own selector, and naming either one narrows the
    // run to just what was named. Without that rule, re-running two stages
    // after a small change would still provision five crash-during-recovery
    // rounds nobody asked for — and a targeted run that quietly costs an extra
    // hour is a targeted run people stop using.
    let selected = selected_stages();
    let recrash = recovery_conformance::recrash::selected();
    let targeted = selected.is_some() || recrash.is_some();

    for stage in CrashStage::ALL {
        let stage = *stage;
        match &selected {
            Some(selected) if !selected.contains(&stage) => continue,
            None if targeted => continue,
            _ => {}
        }
        report.attempted += 1;
        let started = Instant::now();

        // Every exercise resumes to quiescence, so every exercise eventually
        // mutates the chain even when its crash itself occurs before the first
        // POST. A round is one-shot once any bundle delegates; sharing one
        // would make later cases observe effects from an earlier case.
        let round = match provision(&fixture).await {
            Ok(round) => round,
            Err(error) => {
                report
                    .skipped
                    .push((stage.to_string(), format!("no round: {error:#}")));
                continue;
            }
        };

        match exercise(&fixture, stage, &round, &control).await {
            Ok(()) => {
                eprintln!("  PASS {stage} in {:.0}s", started.elapsed().as_secs_f64());
                report.passed.push(stage.to_string());
            }
            // Printed as they happen, not only in the final report. A matrix
            // run takes tens of minutes, and a verdict withheld until the end
            // is indistinguishable from a stage that is still running.
            Err(Outcome::Skipped(why)) => {
                eprintln!(
                    "  SKIP {stage} after {:.0}s: {why}",
                    started.elapsed().as_secs_f64()
                );
                report.skipped.push((stage.to_string(), why));
            }
            Err(Outcome::Failed(why)) => {
                eprintln!(
                    "  FAIL {stage} after {:.0}s: {why}",
                    started.elapsed().as_secs_f64()
                );
                report.failed.push((stage.to_string(), why));
            }
        }
    }

    // Signer-less target recovery. One case rather than a step inside every
    // stage; see `exercise_signerless` for why that had to change.
    if !targeted || std::env::var_os("RECOVERY_CONFORMANCE_SIGNERLESS").is_some() {
        report.attempted += 1;
        let started = Instant::now();
        let label = "signerless-target-recovery".to_string();
        match provision(&fixture).await {
            Err(error) => report
                .skipped
                .push((label, format!("no round: {error:#}"))),
            Ok(round) => match exercise_signerless(&fixture, &round, &control).await {
                Ok(()) => {
                    eprintln!("  PASS {label} in {:.0}s", started.elapsed().as_secs_f64());
                    report.passed.push(label);
                }
                Err(Outcome::Skipped(why)) => {
                    eprintln!(
                        "  SKIP {label} after {:.0}s: {why}",
                        started.elapsed().as_secs_f64()
                    );
                    report.skipped.push((label, why));
                }
                Err(Outcome::Failed(why)) => {
                    eprintln!(
                        "  FAIL {label} after {:.0}s: {why}",
                        started.elapsed().as_secs_f64()
                    );
                    report.failed.push((label, why));
                }
            },
        }
    }

    // Crash-during-recovery. Runs after the single-stage matrix because it is
    // the more expensive dimension — each case provisions a round and kills the
    // child two or three times within it — and because a failure here is far
    // easier to read once the single-fault stages are known green.
    for case in recovery_conformance::recrash::ALL {
        let case = *case;
        match &recrash {
            Some(selected) if !selected.contains(&case) => continue,
            None if targeted => continue,
            _ => {}
        }
        report.attempted += 1;
        let started = Instant::now();
        let round = match provision(&fixture).await {
            Ok(round) => round,
            Err(error) => {
                report
                    .skipped
                    .push((case.name().to_string(), format!("no round: {error:#}")));
                continue;
            }
        };
        match exercise_recrash(&fixture, case, &round, &control).await {
            Ok(()) => {
                eprintln!("  PASS {case} in {:.0}s", started.elapsed().as_secs_f64());
                report.passed.push(case.name().to_string());
            }
            Err(Outcome::Skipped(why)) => {
                eprintln!(
                    "  SKIP {case} after {:.0}s: {why}",
                    started.elapsed().as_secs_f64()
                );
                report.skipped.push((case.name().to_string(), why));
            }
            Err(Outcome::Failed(why)) => {
                eprintln!(
                    "  FAIL {case} after {:.0}s: {why}",
                    started.elapsed().as_secs_f64()
                );
                report.failed.push((case.name().to_string(), why));
            }
        }
    }
    report
}

enum Outcome {
    Skipped(String),
    Failed(String),
}

/// Runs one stage end to end.
async fn exercise(
    fixture: &Fixture,
    stage: CrashStage,
    round: &ProvisionedRound,
    control: &DurableSnapshot,
) -> Result<(), Outcome> {
    let started = Instant::now();
    let sidecar = fixture.workspace.join(format!("{}.db", stage.name()));
    let _ = std::fs::remove_file(&sidecar);

    let armed = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::Armed { stage },
        MAX_DISPATCHES,
        &Faults::none(),
    );

    // (c) spawn the armed child; (d) require SIGABRT and a matching observation
    let crash = run_until_crash(&fixture.worker, &armed);
    warm_from(fixture, &sidecar);
    let crash = match crash {
        Ok(crash) => crash,
        Err(error) => {
            let detail = format!("{error:#}");
            // A stage that stops firing is the way this suite rots: it becomes
            // a skip, skips do not fail the matrix, and the run stays green
            // having proven nothing about that boundary. Only the stages known
            // to be unreachable may skip; for any other, a trigger that never
            // fires is a failure.
            if detail.contains("never reached") && !is_known_unreachable(stage) {
                return Err(Outcome::Failed(format!(
                    "{stage} was never reached, and it is not a stage known to be \
                     unreachable, so its crash seam has stopped firing: {detail}"
                )));
            }
            return Err(Outcome::Skipped(detail));
        }
    };

    // (b) capture the durable state the crash left
    let after_crash = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;

    // The two double-spend-adjacent stages leave exactly the state a host's
    // reset must refuse: an abandoned reservation whose bytes may already be on
    // the wire, over setup that can no longer be rebuilt. The SDK's own tests
    // check that guard against fixture-assembled rows; this checks it against
    // rows a killed process actually produced, which is the combination nothing
    // else exercises.
    if stage.is_sharp() {
        let held = assert_a_host_reset_preserved_crash_state(
            &sidecar,
            &fixture_account(),
            &round.round_id,
        )
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
        eprintln!("  {stage}: a host reset left {held}");
    }

    // (f) plan twice in a fresh process-local database and require agreement
    let plan = deterministic_plan(
        &sidecar,
        &fixture_account(),
        &round.round_id,
        &proposal_ids(),
    )
    .map_err(|error| Outcome::Failed(format!("{error:#}")))?;

    // (g) the stage's own durable expectations
    let bundle = default_target().bundle_index;
    recovery_conformance::combined::assert_combined_stage(
        stage,
        &plan,
        &after_crash,
        &fixture_account(),
        &round.round_id,
        bundle,
        &proposal_ids(),
    )
    .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    assert_stage_state(stage, &plan, &after_crash, bundle)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    assert_plans_precede_broadcast(&after_crash)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    if stage.touches_chain() && crash.dispatched_a_post() {
        // A dispatched POST means a transaction may exist. Its record is the
        // only evidence of that, so this sidecar is never discarded or retried
        // past, whatever happens next.
        eprintln!(
            "  {stage}: a POST reached the wire; sidecar preserved at {}",
            sidecar.display()
        );
    }
    assert_other_bundles_untouched(&plan, bundle, 3)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;

    // Every stage resumes, including the ones that never reached the chain.
    //
    // They used to stop here, and the reason was sound while it held: a
    // pre-chain stage resumed on a *shared* round would delegate and vote on
    // it, and a delegation is consumed on the vote chain, so the next stage's
    // copy of that round would fail with `nullifier already spent` — a
    // statement about round accounting rather than about recovery.
    //
    // That sharing is gone. Every stage now provisions its own round, so a
    // pre-chain resume consumes only its own delegation and cannot reach
    // another stage. Stopping early would leave the whole delegation-side
    // family proving that the crash was real and its durable state correct,
    // while never proving the round recovers — no A2 convergence and no A3
    // equality with the control, for six of twenty stages, including
    // `before-broadcast`, the conservative-by-design case this suite exists
    // for.
    // A dispatched transaction needs to reach a block before the premise of
    // these stages holds. Their whole point is that the chain *has* the
    // transaction while the wallet has no hash for it, and exact-tree recovery
    // resolves the gap. Resuming the instant the bytes leave tests something
    // else: the tree pass runs, correctly finds nothing yet, and a
    // same-generation retry supplies the hash instead — spec-legal, but it
    // means the tree route is never the thing that resolved the round, and
    // whether it was came down to a race with block inclusion.
    if stage.settles_on_chain_before_resume() {
        eprintln!("  {stage}: waiting {CHAIN_INCLUSION_WAIT:?} for the dispatched transaction");
        tokio::time::sleep(CHAIN_INCLUSION_WAIT).await;
    }

    // (h) resume to quiescence in a new process
    let resumed = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::Unarmed,
        MAX_DISPATCHES,
        &Faults::none(),
    );
    if started.elapsed() > STAGE_BUDGET {
        return Err(Outcome::Skipped(
            "stage budget exhausted before resume".to_string(),
        ));
    }
    // A resume that never completes is only a skip when the environment stopped
    // it. Retries that all end on the same non-transport error mean the round
    // does not converge, which is exactly what this matrix exists to catch.
    let outcome = run_to_quiescence(&fixture.worker, &resumed);
    warm_from(fixture, &sidecar);
    let outcome = outcome.map_err(|error| {
        let detail = format!("{error:#}");
        if detail.contains("Transport") || detail.contains("PIR") {
            Outcome::Skipped(format!("resume did not complete: {detail}"))
        } else {
            Outcome::Failed(format!("resume never converged: {detail}"))
        }
    })?;

    // (i) fail on anything that is not a clean ending
    if !outcome.is_terminal_success() {
        return Err(Outcome::Failed(format!(
            "resume ended at {} rather than quiescence; failures: {:?}",
            outcome.quiescence, outcome.failures
        )));
    }

    let terminal = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
    assert_reservations_monotonic(&after_crash, &terminal)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    // The half the count cannot prove: no target gained a second generation,
    // and no bundle the crash left alone reserved another POST.
    assert_no_second_generation(&after_crash, &terminal)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    assert_untouched_bundles_did_not_reserve(&after_crash, &terminal, bundle)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    assert_terminal_rows_unchanged(&after_crash, &terminal)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    // The setup a delegation can never rebuild. Every assertion above asks what
    // the round still owes; this asks what it still has, and a bundle that lost
    // `van_comm_rand` would satisfy all of them while its voting weight was
    // already stranded.
    let setup = assert_delegation_setup_preserved(&after_crash.setup, &terminal.setup)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    eprintln!("  {stage}: delegation setup {setup}");

    // Requirement 8 wants direct evidence that no second transaction was
    // POSTed, not an inference from eventual confirmation. The durable half is
    // the reservation count: every committed POST increments it and a trigger
    // makes it monotonic, so the number of reservations is the number of times
    // the wallet committed to sending. Reported per stage because the correct
    // value differs by boundary — a crash before dispatch legitimately reserves
    // again on resume, one after it must not — and asserting a number before
    // observing it would encode a guess.
    eprintln!(
        "  {stage}: reservations {} -> {} (crash -> terminal), states {:?}",
        after_crash.total_reservations(),
        terminal.total_reservations(),
        terminal.states()
    );

    // A crash after dispatch but before the response was read leaves no
    // candidate hash, so recovery must resolve it either by scanning the tree
    // or by re-POSTing the same generation. The route is reported every run:
    // it is not assertable, but a change in it should not pass unseen.
    if stage == CrashStage::AfterBroadcastUnread {
        let source = confirmation_source(&sidecar, bundle)
            .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
        eprintln!("  {stage}: confirmation source {:?}", source.as_deref());
        assert_confirmed_by_a_legal_route(source.as_deref())
            .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    }

    // Requirement 8's chain-identity half, where the stage captured one. The
    // reservation count above says how many times the wallet committed to
    // sending; this says the thing that actually confirmed is the thing it
    // sent, which counting alone cannot show.
    if let Some(body) = crash.dispatched_response_body() {
        if let Some(dispatched) = dispatched_transaction_hash(body) {
            let confirmed = confirmed_transaction_hash(&sidecar, bundle)
                .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
            let source = confirmation_source(&sidecar, bundle)
                .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
            eprintln!(
                "  {stage}: dispatched {dispatched}, confirmed {} via {}",
                confirmed.as_deref().unwrap_or("<none>"),
                source.as_deref().unwrap_or("<none>")
            );
            assert_recovered_the_same_transaction(
                &dispatched,
                confirmed.as_deref(),
                source.as_deref(),
            )
            .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
        }
    }

    recovery_conformance::combined::assert_preserved_combined(&after_crash, &terminal)
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    recovery_conformance::combined::assert_combined_terminal(&terminal, &proposal_ids())
        .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    // (j) the terminal shape must match the uncrashed control
    if let Err(error) = assert_matches_control(&terminal, control) {
        return Err(Outcome::Failed(format!("{error:#}")));
    }

    // (k) a second resume must find nothing to do
    let settled = deterministic_plan(
        &sidecar,
        &fixture_account(),
        &round.round_id,
        &proposal_ids(),
    )
    .map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    assert_idempotent(&settled).map_err(|error| Outcome::Failed(format!("{error:#}")))?;
    Ok(())
}

/// Stages whose crash seam cannot fire, with the reason.
///
/// Empty, and it should stay that way. `AfterVoteCommit` was listed here on the
/// belief that vote completion offered no seam between persisting the committed
/// vote and writing helper plans. It does: the step probes the helper fleet
/// between those two commits, and that probe is a real network round trip this
/// suite already wraps. Believing a boundary untestable is cheaper than
/// checking, and it cost this stage every run it was ever skipped in.
///
/// Everything must crash where it claims to, or the matrix fails rather than
/// skipping.
fn is_known_unreachable(_stage: CrashStage) -> bool {
    false
}

/// The stages this run exercises, or `None` for the whole matrix.
///
/// Set `RECOVERY_CONFORMANCE_STAGES` to a comma-separated list of stage names
/// to re-run only the stages a change could have affected. The control run is
/// unconditional, because every terminal comparison is against it.
///
/// An unrecognized name is a hard error rather than an empty selection: a typo
/// that silently ran nothing would report a green matrix having tested nothing,
/// which is the failure mode this suite exists to avoid.
fn selected_stages() -> Option<Vec<CrashStage>> {
    let requested = std::env::var("RECOVERY_CONFORMANCE_STAGES").ok()?;
    let stages: Vec<CrashStage> = requested
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            name.parse::<CrashStage>().unwrap_or_else(|_| {
                panic!(
                    "RECOVERY_CONFORMANCE_STAGES names an unknown stage {name:?}; \
                     known stages are {}",
                    CrashStage::ALL
                        .iter()
                        .map(|stage| stage.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect();
    (!stages.is_empty()).then_some(stages)
}

/// Runs one crash-during-recovery sequence end to end.
///
/// The shape is the ordinary exercise with one change that carries the whole
/// point: the resume is itself armed, repeatedly, before any clean run is
/// allowed. `run_until_crash` re-enters an existing sidecar the same way a
/// resume does — `drive_round` skips setup once bundles exist — so a second
/// armed run *is* a recovery run that happens to die partway through.
///
/// Each crash is held to the same anti-rot bar as a first one: `SIGABRT` plus a
/// matching fsynced observation. A second seam that quietly stopped firing
/// would leave this case asserting over an ordinary single-fault round while
/// still reporting green, which is precisely the rot the matrix guards against.
async fn exercise_recrash(
    fixture: &Fixture,
    case: recovery_conformance::RecrashCase,
    round: &ProvisionedRound,
    control: &DurableSnapshot,
) -> Result<(), Outcome> {
    let started = Instant::now();
    let sidecar = fixture.workspace.join(format!("{}.db", case.name()));
    let _ = std::fs::remove_file(&sidecar);

    // One snapshot per open, so setup can be compared across *every* interval
    // rather than only first-to-last. A value that drifted on a middle open and
    // drifted back would otherwise pass.
    let mut opens: Vec<(String, DurableSnapshot)> = Vec::new();

    for (index, stage) in case.stages().enumerate() {
        let mut armed = config_for(
            fixture,
            &sidecar,
            round,
            RunMode::Armed { stage },
            MAX_DISPATCHES,
            &Faults::none(),
        );
        // `CrashLog::create` truncates, and every crash after the first would
        // otherwise erase the observations of the one before it — including the
        // dispatched-response record the identity assertions read.
        if index > 0 {
            armed.crash_log = sidecar.with_extension(format!("recrash{index}.crashlog.jsonl"));
        }

        let crash = run_until_crash(&fixture.worker, &armed);
        warm_from(fixture, &sidecar);
        if let Err(error) = crash {
            let detail = format!("{error:#}");
            // A seam that stopped firing is a defect; staging running out of
            // patience is not. `run_until_crash` already waits out a chain
            // recovery that has not advanced, so anything still reporting
            // "never reached" here really did finish the round.
            if detail.contains("never reached") {
                return Err(Outcome::Failed(format!(
                    "{case}: crash {}/{} at {stage} never fired, so this case ran as an \
                     ordinary single-fault round and proved nothing about recovery \
                     durability: {detail}",
                    index + 1,
                    case.crashes()
                )));
            }
            if detail.contains("before stage") {
                return Err(Outcome::Skipped(format!(
                    "{case}: staging never let crash {}/{} at {stage} fire: {detail}",
                    index + 1,
                    case.crashes()
                )));
            }
            return Err(Outcome::Skipped(detail));
        }

        let snapshot = DurableSnapshot::read(&sidecar)
            .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
        eprintln!(
            "  {case}: crash {}/{} at {stage}; reservations {}, states {:?}",
            index + 1,
            case.crashes(),
            snapshot.total_reservations(),
            snapshot.states()
        );
        // The plan is the oracle after every crash, not only the last. A
        // sequence that produced an unplannable sidecar midway would otherwise
        // surface as a confusing failure at the end.
        deterministic_plan(
            &sidecar,
            &fixture_account(),
            &round.round_id,
            &proposal_ids(),
        )
        .map_err(|error| {
            Outcome::Failed(format!("{case}: after crash {}: {error:#}", index + 1))
        })?;
        opens.push((format!("crash {}", index + 1), snapshot));
    }

    // Now let it finish.
    let resumed = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::Unarmed,
        MAX_DISPATCHES,
        &Faults::none(),
    );
    let outcome = run_to_quiescence(&fixture.worker, &resumed);
    warm_from(fixture, &sidecar);
    let outcome = outcome
        .map_err(|error| Outcome::Failed(format!("{case}: resume never converged: {error:#}")))?;
    if !outcome.is_terminal_success() {
        return Err(Outcome::Failed(format!(
            "{case}: resume ended at {} rather than quiescence; failures: {:?}",
            outcome.quiescence, outcome.failures
        )));
    }

    let terminal = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
    opens.push(("terminal".to_string(), terminal));

    // Every consecutive interval, so a violation names the two opens it
    // happened between rather than only the endpoints.
    let mut compared_columns = 0;
    for window in opens.windows(2) {
        let (from, before) = &window[0];
        let (to, after) = &window[1];
        let setup = assert_delegation_setup_preserved(&before.setup, &after.setup)
            .map_err(|error| Outcome::Failed(format!("{case}: {from} -> {to}: {error:#}")))?;
        compared_columns += setup.compared_columns;
        assert_reservations_monotonic(before, after)
            .map_err(|error| Outcome::Failed(format!("{case}: {from} -> {to}: {error:#}")))?;
        assert_no_second_generation(before, after)
            .map_err(|error| Outcome::Failed(format!("{case}: {from} -> {to}: {error:#}")))?;
        assert_terminal_rows_unchanged(before, after)
            .map_err(|error| Outcome::Failed(format!("{case}: {from} -> {to}: {error:#}")))?;
    }
    // A sequence that compared no frozen setup examined none of the state this
    // case exists to protect, and would hold trivially.
    if compared_columns == 0 {
        return Err(Outcome::Failed(format!(
            "{case}: no frozen delegation setup was compared across {} opens, so the \
             preservation claim held vacuously",
            opens.len()
        )));
    }

    let terminal = &opens.last().expect("opens is never empty").1;
    assert_matches_control(terminal, control)
        .map_err(|error| Outcome::Failed(format!("{case}: {error:#}")))?;
    recovery_conformance::combined::assert_combined_terminal(terminal, &proposal_ids())
        .map_err(|error| Outcome::Failed(format!("{case}: {error:#}")))?;

    let settled = deterministic_plan(
        &sidecar,
        &fixture_account(),
        &round.round_id,
        &proposal_ids(),
    )
    .map_err(|error| Outcome::Failed(format!("{case}: {error:#}")))?;
    assert_idempotent(&settled).map_err(|error| Outcome::Failed(format!("{case}: {error:#}")))?;

    eprintln!(
        "  {case}: {} crashes, {compared_columns} setup columns held across {} opens, \
         in {:.0}s — {}",
        case.crashes(),
        opens.len(),
        started.elapsed().as_secs_f64(),
        case.asks()
    );
    Ok(())
}

/// Advancing a durably authorized combined batch with no signing material.
///
/// The claim is worth stating precisely, because it is the reason
/// `delegate_cast_recovery` is an immutable table rather than a cache: once a
/// combined delegation-and-cast batch is durably authorized, the spend-auth
/// signature it carries is *sufficient*. A wallet that still holds the sidecar
/// but can no longer produce signing material — a hardware signer that is gone,
/// a seed the host cannot re-derive — must still be able to drive that batch to
/// confirmation. If it cannot, the authorization was never self-contained and
/// the round's weight depends on key material the specification says it should
/// not need.
///
/// # Why the crash lands on the last bundle
///
/// This used to be a step inside every stage, and across a full 19-stage run it
/// ran at none of them. The signer-less child is given a single dispatch and no
/// mnemonic, driver, signer or hotkey, and `RoundDriver` executes the plan in
/// order — so the exercise is only possible when the plan owes nothing but the
/// target. The specification orders steps "delegation first, then vote and
/// share submission", and every crash seam fires on the *first* POST of its
/// class, which for a round that drives bundles in order is always bundle 0.
/// With two later bundles still owing `Delegate`, the target sorts third and
/// the child is asked for material it deliberately does not have.
///
/// Letting the earlier bundles' batches through first fixes that at the source.
/// The crash lands on the last bundle, whose siblings are already confirmed, so
/// the only work the round still owes *is* the target's own batch — which is
/// both the shape this claim needs and the shape a real wallet is in when it
/// discovers its signer is gone.
///
/// Nothing is trimmed or reshaped to get there. An earlier attempt built the
/// precondition by deleting the untouched bundles from a copy; it produced the
/// right plan, but it meant testing the claim against a round no wallet would
/// ever hold, and the worker rightly refused the altered layout.
async fn exercise_signerless(
    fixture: &Fixture,
    round: &ProvisionedRound,
    control: &DurableSnapshot,
) -> Result<(), Outcome> {
    let started = Instant::now();
    let label = "signerless-target-recovery";
    let sidecar = fixture.workspace.join("signerless-target-recovery.db");
    let _ = std::fs::remove_file(&sidecar);

    // The last bundle, and the crash that lets its siblings through first.
    let bundle = (recovery_conformance::provisioning::EXPECTED_BUNDLE_COUNT - 1) as u32;
    let stage = CrashStage::BeforeBroadcast;
    let mut armed = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::Armed { stage },
        MAX_DISPATCHES,
        &Faults::none(),
    );
    armed.broadcast_skip = bundle as usize;
    armed.target.bundle_index = bundle;

    let crash = run_until_crash(&fixture.worker, &armed);
    warm_from(fixture, &sidecar);
    if let Err(error) = crash {
        let detail = format!("{error:#}");
        if detail.contains("never reached") {
            return Err(Outcome::Failed(format!(
                "{label}: {stage} never fired on bundle {bundle}, so there was no authorized \
                 target to recover: {detail}"
            )));
        }
        return Err(Outcome::Skipped(detail));
    }

    let after_crash = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
    let authorized = after_crash.combined.iter().any(|b| {
        b.bundle_index == bundle
            && !b.authorizations.is_empty()
            && b.van_position.is_none()
    });
    if !authorized {
        return Err(Outcome::Failed(format!(
            "{label}: the crash left no durable authorization on bundle {bundle}, so the \
             exercise would have proven nothing"
        )));
    }

    // The precondition, checked rather than assumed: the round owes the target
    // and nothing that needs a signer.
    let plan = deterministic_plan(
        &sidecar,
        &fixture_account(),
        &round.round_id,
        &proposal_ids(),
    )
    .map_err(|error| Outcome::Failed(format!("{label}: {error:#}")))?;
    let leads = matches!(
        plan.next_steps.first(),
        Some(zcash_voting::session::NextStep::AdvanceVoteBatch { bundle_index, .. })
            if *bundle_index == bundle
    );
    if !leads {
        return Err(Outcome::Failed(format!(
            "{label}: the round owes {:?} before the target batch, so a child with no signing \
             material could not reach it",
            plan.next_steps.first()
        )));
    }

    let signerless = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::RecoverCombined,
        1,
        &Faults::none(),
    );
    let mut signerless = signerless;
    signerless.target.bundle_index = bundle;
    let recovered = run_to_quiescence(&fixture.worker, &signerless)
        .map_err(|error| Outcome::Failed(format!("{label}: signerless recovery: {error:#}")))?;
    if recovered.quiescence_kind != "TargetRecovered" || !recovered.failures.is_empty() {
        return Err(Outcome::Failed(format!(
            "{label}: a durably authorized batch could not be advanced without signing \
             material; ended at {} with failures {:?}",
            recovered.quiescence, recovered.failures
        )));
    }

    // It got there without rewriting the setup it was handed.
    let recovered_snapshot = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
    let setup = assert_delegation_setup_preserved(&after_crash.setup, &recovered_snapshot.setup)
        .map_err(|error| Outcome::Failed(format!("{label}: {error:#}")))?;
    if setup.is_vacuous() {
        return Err(Outcome::Failed(format!(
            "{label}: no frozen setup was compared, so the preservation half held vacuously"
        )));
    }
    assert_reservations_monotonic(&after_crash, &recovered_snapshot)
        .map_err(|error| Outcome::Failed(format!("{label}: {error:#}")))?;
    assert_no_second_generation(&after_crash, &recovered_snapshot)
        .map_err(|error| Outcome::Failed(format!("{label}: {error:#}")))?;

    eprintln!(
        "  {label}: bundle {bundle}'s batch, authorized before the crash, confirmed with no \
         mnemonic, signer or hotkey; {setup}"
    );

    // And the round still finishes normally afterwards, with a signer present
    // for the work that genuinely needs one.
    let resumed = config_for(
        fixture,
        &sidecar,
        round,
        RunMode::Unarmed,
        MAX_DISPATCHES,
        &Faults::none(),
    );
    let outcome = run_to_quiescence(&fixture.worker, &resumed);
    warm_from(fixture, &sidecar);
    let outcome = outcome
        .map_err(|error| Outcome::Failed(format!("{label}: resume never converged: {error:#}")))?;
    if !outcome.is_terminal_success() {
        return Err(Outcome::Failed(format!(
            "{label}: resume ended at {} rather than quiescence; failures: {:?}",
            outcome.quiescence, outcome.failures
        )));
    }
    let terminal = DurableSnapshot::read(&sidecar)
        .map_err(|error| Outcome::Failed(format!("unreadable sidecar: {error:#}")))?;
    assert_matches_control(&terminal, control)
        .map_err(|error| Outcome::Failed(format!("{label}: {error:#}")))?;

    eprintln!(
        "  {label}: the round converged to the control afterwards, in {:.0}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

//! What one benchmark child run needs to know, as a value rather than an
//! argument list.
//!
//! The parent writes this to a file inside the run directory and passes the
//! child a single path — the same choice `recovery-conformance` makes, for the
//! same two reasons: a dozen positional pairs is a transposition waiting to
//! happen, and argv is world readable through `ps`. Nothing here is secret;
//! the credentials the child needs reach it only through the environment it
//! inherits, so they are never written to disk or exposed in a process listing.
//!
//! The file is also the run's own record of what was measured. `analyze` reads
//! it back out of a finished run directory, so a manifest and its metrics can
//! never describe a different workload than the one that produced them.

use std::path::{Path, PathBuf};

use recovery_conformance::helper_fleet::HelperFleetPlan;
use recovery_conformance::run_config::Endpoints;
use serde::{Deserialize, Serialize};

use crate::ballot::Ballot;

/// Everything one child run needs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchRunConfig {
    /// The voting sidecar this run builds and drives.
    pub sidecar: PathBuf,
    /// The scanned voter wallet note selection reads.
    pub wallet_db: PathBuf,
    /// A previous sidecar whose cached PIR proofs are copied in first.
    ///
    /// Absent means every padded slot and note is fetched from the live PIR
    /// fleet, which staging serves from one synchronous endpoint. Present, the
    /// run measures the phases a warm host actually spends time in.
    pub warm_pir_from: Option<PathBuf>,
    pub round_id: String,
    pub account_uuid: String,
    pub endpoints: Endpoints,
    /// The ballot this round was provisioned with, and votes.
    pub ballot: Ballot,
    /// The synthetic helper fleet, if any. Empty means the real staging primary.
    #[serde(default)]
    pub fleet: HelperFleetPlan,
    /// Unix time when this benchmark began provisioning the ceremony.
    ///
    /// Vizor receives the corresponding authenticated `ceremony_phase_start`
    /// from voting config. The SDK needs both this value and the vote end to
    /// derive the passive helper-submission window.
    pub ceremony_start_time_seconds: u64,
    /// Unix vote-end the round was provisioned with.
    ///
    /// Share timing derives its retry, overdue, and last-moment windows from
    /// the distance to this time, so a run's window is part of what it measured
    /// rather than an incidental setting.
    pub vote_end_time_seconds: u64,
    /// Bundles the driver advances at once.
    ///
    /// The SDK ships three. Lowering it to one is what a cold-PIR run needs,
    /// because staging serves PIR from a single synchronous endpoint.
    pub bundle_concurrency: usize,
    /// Vote-commitment proofs built at once within a bundle.
    ///
    /// The SDK's `DEFAULT_BATCH_PROOF_CONCURRENCY` is three, capped at fifteen.
    /// A 37-proposal bundle builds 37 of these, so this is the second of the
    /// two serializations a wide ballot pays for.
    pub proof_concurrency: usize,
    /// Upper bound on driver dispatches, so a plan that never shrinks ends the
    /// run instead of hanging the benchmark.
    pub max_dispatches: usize,
    /// Milliseconds between polls while a chain submission is still tracking.
    ///
    /// `ChainAdvancePolicy::pending_repoll`, which the SDK defaults to two
    /// seconds. A chain advance is mostly waiting rather than network, so this
    /// separates the host's polling cadence from the chain's own block time —
    /// lowering it cannot make a block arrive sooner, and the difference
    /// between the two is exactly what a run at a shorter cadence measures.
    pub chain_repoll_milliseconds: u64,
    /// Wall-clock ceiling on the confirmation phase, in seconds.
    ///
    /// The benchmark's bound, not the round's: a healthy host confirms across
    /// the whole voting window. When it expires the run reports the tail as
    /// explicitly incomplete rather than pretending the round settled.
    pub tracking_budget_seconds: u64,
    /// Which shares the run confirms after delivery.
    pub confirm_mode: ConfirmMode,
    /// Focused confirmations driven at once, for [`ConfirmMode::Concurrent`].
    pub confirm_concurrency: usize,
    /// Detailed records retained per reported invocation.
    ///
    /// A run whose records are capped cannot support a peak-concurrency claim,
    /// so the derived metrics say so rather than reporting a smaller peak.
    pub max_records: usize,
    /// Where the child writes its snapshots, events, and outcome.
    pub run_dir: PathBuf,
}

impl BenchRunConfig {
    /// Path of the run configuration inside a run directory.
    pub fn path_in(run_dir: &Path) -> PathBuf {
        run_dir.join("run-config.json")
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}

/// What a run confirms once every share has been delivered.
///
/// A round designates one immediate helper share, and confirming it is what
/// decides whether a vote reads as cast. The other two modes chase the whole
/// tail, which no wallet waits on.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmMode {
    /// Confirm only `RoundPlan::immediate_share_key`, as a wallet does.
    #[default]
    Immediate,
    /// Run the shipped background tracker over every unconfirmed share.
    All,
    /// Drive concurrent focused confirmations over every unconfirmed share.
    Concurrent,
}

impl ConfirmMode {
    /// Whether this mode measures shipped wallet behaviour.
    ///
    /// Only [`Immediate`](Self::Immediate) does. The other two are deliberate
    /// experiments about the confirmation tail, and every report carrying their
    /// numbers says so.
    pub fn is_shipped_behaviour(self) -> bool {
        matches!(self, Self::Immediate)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::All => "all",
            Self::Concurrent => "concurrent",
        }
    }
}

/// One failed obligation, flattened so the parent can report it without
/// linking the driver's error types.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FailureRecord {
    pub step: Option<String>,
    pub bundle_index: Option<u32>,
    pub kind: String,
    /// Redacted by construction: the SDK bounds and escapes diagnostics before
    /// they reach here, and no payload or key material is copied in.
    pub message: String,
}

/// One share's public position in a round.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShareIdentity {
    pub bundle_index: u32,
    pub proposal_id: u32,
    pub share_index: u32,
}

/// Distribution of helper submission times planned for one benchmark round.
///
/// Initial wallet-to-helper delivery still happens in the foreground. A
/// passive share is one whose helper was instructed to wait until `submit_at`
/// before revealing it to the chain.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShareScheduleSummary {
    /// Wall clock used to classify future submissions.
    pub observed_at_seconds: u64,
    /// Every durable share row created for the round.
    pub total_shares: usize,
    /// The immutable round designation, not merely a `submit_at == 0` row.
    pub designated_immediate_shares: usize,
    /// Rows whose helper submission time is zero.
    pub submit_at_zero_shares: usize,
    /// Rows carrying a nonzero scheduled submission time.
    pub passive_shares: usize,
    /// Passive rows whose scheduled time had not arrived when inspected.
    pub future_shares: usize,
    /// Passive rows due inside the benchmark's confirmation observation budget.
    pub due_within_tracking_budget: usize,
    /// Earliest nonzero helper submission timestamp.
    pub earliest_submit_at_seconds: Option<u64>,
    /// Median passive delay from durable share creation.
    pub p50_delay_seconds: u64,
    /// 95th-percentile passive delay from durable share creation.
    pub p95_delay_seconds: u64,
    /// Longest passive delay from durable share creation.
    pub max_delay_seconds: u64,
}

/// What one background share-tracking invocation did.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TrackingSummary {
    /// Debug rendering of `ShareTrackingQuiescence`.
    pub quiescence: String,
    pub passes: u32,
    pub confirmed: usize,
    pub resubmitted: usize,
    pub ambiguous: usize,
    pub unrecoverable: usize,
}

/// What one benchmark run ended up doing.
///
/// The authoritative domain result, kept separate from the timing snapshots.
/// A run that measured beautifully and delivered nothing is a failed run, and
/// this is what says so.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchOutcome {
    /// Debug rendering of `RoundQuiescence`.
    pub quiescence: String,
    /// Just the variant name, so the parent can match without parsing.
    pub quiescence_kind: String,
    pub failures: Vec<FailureRecord>,
    /// Notes the wallet selected, and the bundles they packed into.
    ///
    /// Observed, not asserted: the benchmark reports the layout it got, because
    /// a wallet rebalance changes the workload rather than invalidating it.
    pub notes: usize,
    pub bundles: u32,
    pub proposals: usize,
    /// The round's designated immediate share, once the plan has one.
    ///
    /// Recorded so the report can say how much of the round preceded the share a
    /// voter actually waits on. Absent when no vote has been planned yet.
    #[serde(default)]
    pub immediate_share: Option<ShareIdentity>,
    /// Helper submission schedule created during vote planning.
    #[serde(default)]
    pub share_schedule: ShareScheduleSummary,
    /// Proposals the driver reported complete, out of the ballot.
    pub completed_proposals: usize,
    pub tracking: Vec<TrackingSummary>,
    /// Wall-clock seconds the child spent inside the round driver.
    pub round_drive_seconds: f64,
    /// Wall-clock seconds the child spent in background share tracking.
    pub tracking_seconds: f64,
}

impl BenchOutcome {
    /// Path of the outcome inside a run directory.
    pub fn path_in(run_dir: &Path) -> PathBuf {
        run_dir.join("outcome.json")
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }

    /// Whether the round finished everything the foreground owns.
    ///
    /// `BackgroundShareWorkOnly` counts: a share already accepted by a helper
    /// but not yet visible as confirmed is the host's timer to finish, not a
    /// failure of the drive.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
            && matches!(
                self.quiescence_kind.as_str(),
                "NoWorkLeft" | "BackgroundShareWorkOnly"
            )
    }
}

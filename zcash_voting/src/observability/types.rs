use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Per-call collection policy for reported workflows. Absence disables collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityOptions {
    /// Maximum detailed stage/attempt records; zero retains only summaries.
    /// Active timers and summary groups have independent limits.
    pub max_records: usize,
    /// Maximum distinct (stage, attribution, outcome) summary groups.
    pub max_summary_groups: usize,
    /// Maximum concurrently admitted stage timers.
    pub max_active_stages: usize,
}

impl Default for ObservabilityOptions {
    fn default() -> Self {
        Self {
            max_records: 4096,
            max_summary_groups: 4096,
            max_active_stages: 4096,
        }
    }
}

impl ObservabilityOptions {
    /// Collects outcome-specific summaries without retaining detailed records.
    pub fn summaries_only() -> Self {
        Self {
            max_records: 0,
            ..Self::default()
        }
    }
}

/// Existing API result together with optional invocation diagnostics.
///
/// Save `observability` before applying `?` to `result`. Operational errors
/// retain the stages and attempts that preceded them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct OperationReport<T> {
    pub result: T,
    pub observability: Option<OperationObservability>,
}

impl<T> OperationReport<T> {
    /// Separates the authoritative result from diagnostics before propagating errors.
    pub fn into_parts(self) -> (T, Option<OperationObservability>) {
        (self.result, self.observability)
    }
}

/// Public, non-secret location of work within a round. Unknown fields stay absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObservationAttribution {
    pub bundle_index: Option<u32>,
    pub proposal_id: Option<u32>,
    pub share_index: Option<u32>,
}

/// Execution outcome, distinct from whether a Rust call returned `Ok`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ObservationOutcome {
    Succeeded,
    Failed,
    Pending,
    Rejected,
    Cancelled,
    NoWork,
    Reused,
    /// Work was still running when the invocation returned, or its future was dropped.
    Unfinished,
    /// Dispatch may have happened; this is not proof of acceptance.
    PossiblyDispatched,
}

/// Request correlation and transport boundaries, present only for observed direct HTTP.
/// Server timings are untrusted diagnostics and never affect request outcomes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HttpRequestDiagnostics {
    pub request_id: String,
    pub protocol: Option<String>,
    /// Time from handoff until Hyper assigns a connection, including pool wait.
    pub connection_acquired_us: Option<u64>,
    /// DNS, TCP, and TLS combined for the assigned connection; excludes pool wait.
    #[serde(default)]
    pub connection_setup_us: Option<u64>,
    /// True when the assigned connection was established before this request began.
    #[serde(default)]
    pub connection_predates_request: Option<bool>,
    pub response_headers_us: Option<u64>,
    pub server_started_unix_us: Option<u64>,
    pub server_handler_us: Option<u64>,
    /// Client time to headers less reported server time; not a pure network metric.
    pub unattributed_wait_us: Option<u64>,
}

/// One measured stage or network attempt. Durations may overlap with children.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ObservationRecord {
    pub id: u64,
    pub parent_id: Option<u64>,
    /// SDK-authored stage name, never caller data or a request URL.
    /// Labels and nesting may evolve across SDK versions; consumers must
    /// tolerate unknown stages rather than depend on an exact phase sequence.
    pub stage: Arc<str>,
    pub attribution: ObservationAttribution,
    pub started_after_us: u64,
    pub elapsed_us: u64,
    pub outcome: ObservationOutcome,
    /// Stable error category only; never the error's free-form message.
    pub error_kind: Option<Arc<str>>,
    pub http_status: Option<u16>,
    /// Configured endpoint ordinal, when known. URLs and request paths are excluded.
    pub endpoint_index: Option<u32>,
    /// One-based network attempt within its parent operation; absent on ordinary stages.
    pub attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_diagnostics: Option<HttpRequestDiagnostics>,
}

/// Totals for one stage, attribution, and outcome, subject to the summary group cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ObservationSummary {
    pub stage: Arc<str>,
    pub attribution: ObservationAttribution,
    pub calls: u64,
    pub outcome: ObservationOutcome,
    /// Sum of stage durations, not invocation wall time.
    pub cumulative_elapsed_us: u64,
}

/// Frozen snapshot of one invocation. Safe to serialize independently of its result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OperationObservability {
    pub operation: String,
    /// Wall-clock anchor in microseconds since the Unix epoch. Durations stay monotonic.
    pub started_at_unix_us: u64,
    pub round_id: Option<String>,
    pub elapsed_us: u64,
    pub outcome: ObservationOutcome,
    pub records: Vec<ObservationRecord>,
    pub summaries: Vec<ObservationSummary>,
    pub records_dropped: u64,
    /// Measurements omitted because their summary group could not be admitted.
    /// Counts updates, not distinct omitted groups.
    pub summary_updates_dropped: u64,
    /// Stage starts omitted because the concurrent timer limit was reached.
    pub active_stages_dropped: u64,
}

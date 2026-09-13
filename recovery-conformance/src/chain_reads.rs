//! Which step a chain read belonged to.
//!
//! `ChainRecoveryStalled` is not a verdict. The specification separates it from
//! `ChainTerminal` because running again later may resolve it, so the harness
//! waits and re-drives rather than failing — and that is right for a run whose
//! exact-tree scan looked and found nothing, because the transaction may simply
//! not be mined yet.
//!
//! It is not right for a run that returned without asking the chain anything at
//! all. Those two endings are indistinguishable from the outside: both report
//! `ChainRecoveryStalled`, both leave the row in `Recovering`, and the next
//! process resolves both. The difference is whether a request was made, which is
//! why it has to be recorded rather than inferred.
//!
//! # Why the count is per step rather than per run
//!
//! A resumed round drives every bundle, not only the crashed one. The others
//! delegate, broadcast, and poll, so a round-wide total is never zero and would
//! cover for a target that made no request of its own. Attribution is by
//! selected step, which is sound here because every run this suite drives sets
//! `max_bundle_concurrency` to one: exactly one step owns the route while it
//! executes.
//!
//! Two request classes count. `TransactionLookup` is the status poll and
//! `CommitmentTreeRead` is the exact-tree scan; between them they are every read
//! chain recovery can make. A POST is deliberately not a read — a run that
//! re-transmitted did make progress, but it is `dispatches` that records it, and
//! folding the two together would let a retransmission excuse a missing scan.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use zcash_voting::{session::NextStep, RouteFuture, RouteHttp, RouteRequest};

use crate::stall::{RequestClassifier, StallTarget};

/// The step a read is attributed to.
///
/// The step's own `Debug` rendering, so the key a read is filed under and the
/// key the stalled quiescence is looked up by are produced by one impl and
/// cannot drift apart.
fn step_key(step: &NextStep) -> String {
    format!("{step:?}")
}

#[derive(Default)]
struct Ledger {
    /// The step currently executing, or `None` before the first selection.
    selected: Option<String>,
    /// Reads attributed to each step that has held the route.
    by_step: BTreeMap<String, usize>,
    /// Every counted read, whichever step held the route.
    total: usize,
    /// Reads made before any step was selected, or after the last one ended.
    ///
    /// Kept apart rather than dropped: planning and tree sync both read the
    /// chain outside a step, and silently filing that under the step that
    /// happened to run last would inflate exactly the number an assertion
    /// reads.
    unattributed: usize,
}

/// Counts the chain reads a run made, per step.
///
/// Shared between the route that observes the requests and the reporter that
/// knows which step is running; both hold an [`Arc`] of it.
#[derive(Default)]
pub struct ChainReadLedger {
    inner: Mutex<Ledger>,
}

impl ChainReadLedger {
    /// A ledger with nothing recorded and no step selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attributes subsequent reads to `step`.
    ///
    /// Called for every `StepSelected` event, including steps that make no
    /// chain request at all — a step that is selected and reads nothing is
    /// precisely the observation this exists to make.
    pub fn select(&self, step: &NextStep) {
        let key = step_key(step);
        if let Ok(mut ledger) = self.inner.lock() {
            ledger.by_step.entry(key.clone()).or_insert(0);
            ledger.selected = Some(key);
        }
    }

    /// Counts one request if it is a chain read.
    ///
    /// Anything else — a POST, a helper call, PIR, an unclassified endpoint —
    /// is ignored rather than filed under the nearest class, for the same
    /// reason [`RequestClassifier::classify`] returns `None` rather than
    /// guessing.
    pub fn record(&self, class: Option<StallTarget>) {
        if !matches!(
            class,
            Some(StallTarget::TransactionLookup | StallTarget::CommitmentTreeRead)
        ) {
            return;
        }
        let Ok(mut ledger) = self.inner.lock() else {
            return;
        };
        ledger.total += 1;
        match ledger.selected.clone() {
            Some(step) => *ledger.by_step.entry(step).or_insert(0) += 1,
            None => ledger.unattributed += 1,
        }
    }

    /// How many chain reads ran while `step` held the route.
    pub fn reads_for(&self, step: &NextStep) -> usize {
        let key = step_key(step);
        self.inner
            .lock()
            .map(|ledger| ledger.by_step.get(&key).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Every chain read this run made, whichever step held the route.
    pub fn total(&self) -> usize {
        self.inner.lock().map(|ledger| ledger.total).unwrap_or(0)
    }

    /// Reads made while no step held the route.
    pub fn unattributed(&self) -> usize {
        self.inner
            .lock()
            .map(|ledger| ledger.unattributed)
            .unwrap_or(0)
    }
}

/// Wraps a route executor and counts the chain reads passing through it.
///
/// Outside the stall wrapper on purpose: a request that was armed to hang was
/// still *asked*, and the question this ledger answers is whether the run asked.
/// Counting below the stall would report an armed hang as a run that never
/// looked, which is the one false finding this must not produce.
pub struct CountingRoute<R> {
    inner: R,
    classifier: RequestClassifier,
    ledger: Arc<ChainReadLedger>,
}

impl<R> CountingRoute<R> {
    /// Wraps `inner`, filing what it classifies as a chain read into `ledger`.
    pub fn new(inner: R, classifier: RequestClassifier, ledger: Arc<ChainReadLedger>) -> Self {
        Self {
            inner,
            classifier,
            ledger,
        }
    }
}

impl<R: RouteHttp> RouteHttp for CountingRoute<R> {
    fn execute<'a>(
        &'a self,
        request: RouteRequest<'a>,
        on_dispatch: &'a (dyn Fn() + Send + Sync),
    ) -> RouteFuture<'a> {
        self.ledger.record(
            self.classifier
                .classify(request.method.as_str(), request.url),
        );
        self.inner.execute(request, on_dispatch)
    }

    /// Reported from the wrapped executor.
    ///
    /// Counting a request changes nothing about how it is executed, so both of
    /// these must keep describing the real executor rather than this wrapper.
    fn hook_precedes_connection_setup(&self) -> bool {
        self.inner.hook_precedes_connection_setup()
    }

    fn enforces_connect_timeout(&self) -> bool {
        self.inner.enforces_connect_timeout()
    }
}

//! A round-wide barrier that lets the designated immediate share reach a helper
//! before the round's other shares do.
//!
//! # Why a barrier is needed at all
//!
//! A round designates one immediate share — share index 0 of the lowest voted
//! proposal in the *highest* eligible bundle — and it is the share a voter waits
//! on. Two existing choices put it last rather than first. The designation names
//! the highest bundle index, which is the last bundle to reach the chain, and
//! [`submit_votes`](super::queue::submit_votes) is called once per confirmed
//! unit, so the bundles that confirmed earlier have already delivered by the time
//! the designated one exists. Measured against staging, 67% of a 37-proposal
//! round's shares reached the helper before its immediate share on serialized
//! bundles, and 8% with concurrent ones.
//!
//! Ordering inside one call cannot fix that: the designated share is already the
//! first job of its own commitment. The delay is entirely other bundles, so the
//! barrier has to span the calls that deliver them.
//!
//! # Why it stops at ordering inside one call
//!
//! Holding a call's own shares until its designated one is acknowledged was
//! tried and withdrawn. It breaks two properties the delivery queue is built on:
//! a slow share must not block its siblings
//! (`combined_reconciliation_delivers_later_proposals_while_the_first_is_unfinished`),
//! and a full commitment must still reach the 128-POST ceiling
//! (`full_commitment_reaches_but_never_exceeds_128_posts`). Both are deliberate,
//! and the measurement did not justify overriding them: every share ahead of the
//! designated one belonged to *another* bundle, which is exactly what the gate
//! addresses. Inside its own call the designated share is dispatched first and
//! races only its own siblings.
//!
//! # What it guarantees, and what it deliberately does not
//!
//! The call holding the designated share submits it alone and opens the gate as
//! soon as a helper acknowledges it. Calls without it wait, and **always
//! proceed** once [`WAIT_BUDGET`] expires. That bound is the safety property: a
//! round whose designated bundle has not confirmed — every round, while bundles
//! run serially — must never have its ready bundles stalled behind work that
//! does not exist yet. On expiry delivery is exactly what it is today.
//!
//! So this is a best-effort priority, not an ordering invariant. It cannot make
//! the immediate share first when its bundle confirms after the wait budget, and
//! it is not intended to.
//!
//! # What the wait actually turns on
//!
//! Three things release a waiting delivery: the designated share was already
//! accepted before it started waiting, the call holding that share finished with
//! it, or the budget expired. "Finished with it" includes a refusal — a share no
//! helper took will not arrive by being waited for.
//!
//! The already-accepted case is read from durable state once, before waiting,
//! and is what lets a later pass or a restart proceed with no state carried
//! between calls. Within one share's fan-out the boundary is its wave
//! completing: the executor resolves a wave's outcomes only after all of its
//! POSTs return, deliberately and serially, so that a stale generation aborts
//! before a later write can mask it. That ordering is not changed here; what it
//! costs is at most one wave's slowest reply, is bounded by [`WAIT_BUDGET`]
//! regardless, and is zero for a single-target placement.
//!
//! # Why an acknowledgement is the right signal
//!
//! The helper answers a share POST only once the row is durably in its queue.
//! That is a much earlier and more reliable signal than chain confirmation, and
//! it is what actually matters here: once the share is queued the helper will
//! process it, and until then no amount of other traffic should be ahead of it.
//! A duplicate answer counts too — the share is in the queue either way, which is
//! how the delivery executor already treats it.
//!
//! # Why it helps even against a helper that serves its queue arbitrarily
//!
//! A helper picking arbitrarily among ready shares gives no advantage to
//! whichever arrived first. What the gate provides is not position but solitude:
//! while it is closed the immediate share is the round's only in-flight POST, so
//! an arbitrary choice among ready shares is very likely to be it. Ordering on
//! the helper side strengthens this; it is not a precondition for it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use tokio::sync::Notify;

/// How long a call without the designated share waits before delivering anyway.
///
/// Sized for the gap between bundles reaching the chain when they run
/// concurrently, which staging measured at about five seconds. Long enough to
/// cover that skew, short enough that a round whose designated bundle is still
/// being proved is not held up meaningfully. Rounds that run bundles serially
/// always reach this bound; that is the intended, documented outcome rather than
/// a failure.
pub(super) const WAIT_BUDGET: Duration = Duration::from_secs(10);

/// How often a waiter observes host cancellation while the gate is closed.
///
/// Matches the delivery admission tick in [`super::capacity`], for the same
/// reason: a caller that cancels must not wait out the whole budget first.
const CANCEL_CHECK: Duration = Duration::from_millis(50);

/// `(sidecar connection, wallet id, round id)`. The connection id keeps two
/// independently opened sidecars sharing a wallet from gating each other.
type GateKey = (u64, String, String);

/// Live gates, dropped once no delivery holds one.
static GATES: LazyLock<Mutex<HashMap<GateKey, Weak<ImmediateGate>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Rounds already released, so a later call does not wait again.
///
/// A gate lives only as long as the deliveries holding it, which is right for
/// the gate itself but loses the one fact a later pass needs: this round has
/// been released. Durable state answers that when the designated share was
/// *accepted*, but not when it was **refused** — a definite failure records no
/// acceptance, so an acceptance-only predicate would send every later pass back
/// into the full budget waiting for a share no helper took.
///
/// Bounded rather than unbounded: a process drives few rounds at once, and the
/// oldest entry is evicted past [`RELEASED_ROUNDS`]. Losing an entry costs one
/// avoidable wait, never correctness. The set does not survive a restart, where
/// the durable acceptance check covers the accepted case and a refused round
/// pays one wait once.
static RELEASED: LazyLock<Mutex<(HashMap<GateKey, ()>, VecDeque<GateKey>)>> =
    LazyLock::new(|| Mutex::new((HashMap::new(), VecDeque::new())));

/// Released rounds remembered before the oldest is evicted.
const RELEASED_ROUNDS: usize = 64;

/// Whether this round has already been released.
///
/// Read while the caller holds the gate registry, so that observing "not
/// released" and creating a closed gate cannot straddle a release.
fn is_released(key: &GateKey) -> bool {
    RELEASED
        .lock()
        .map(|released| released.0.contains_key(key))
        .unwrap_or(false)
}

/// One round's barrier.
pub(super) struct ImmediateGate {
    opened: Mutex<bool>,
    woken: Notify,
    /// Recorded on open, so a later delivery for this round starts released.
    key: Option<GateKey>,
}

/// Why a waiter stopped waiting. Recorded so a report can distinguish a gate
/// that did its job from one that timed out, which are very different runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GateWait {
    /// The designated share was already accepted, so nothing waited.
    AlreadyAccepted,
    /// A helper acknowledged the immediate share while this caller waited.
    Accepted,
    /// [`WAIT_BUDGET`] expired first; delivery proceeds unprioritised.
    Expired,
    /// The host cancelled while waiting.
    Cancelled,
}

impl ImmediateGate {
    /// The gate for one round, creating it if this is the first delivery.
    ///
    /// A round already released comes back open, so a later pass does not wait
    /// again for a designated share that has been dealt with — accepted or
    /// refused.
    pub(super) fn for_round(sidecar_id: u64, wallet_id: &str, round_id: &str) -> Arc<Self> {
        let key = (sidecar_id, wallet_id.to_string(), round_id.to_string());
        let mut gates = match GATES.lock() {
            Ok(gates) => gates,
            // A poisoned registry must not stop delivery: an unshared gate makes
            // every caller behave as though it holds the designated share, which
            // is exactly today's unprioritised behaviour.
            Err(_) => return Arc::new(Self::new()),
        };
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        // Checked under the registry lock, not before it. A holder that opens,
        // records its release, and drops its last reference between an earlier
        // check and this one would otherwise leave this caller waiting on a
        // freshly created closed gate for a round already released — and a
        // refusal records no acceptance to fall back on.
        if is_released(&key) {
            let gate = Self::new();
            gate.open();
            return Arc::new(gate);
        }
        let gate = Arc::new(Self::keyed(key.clone()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }

    fn new() -> Self {
        Self {
            opened: Mutex::new(false),
            woken: Notify::new(),
            key: None,
        }
    }

    fn keyed(key: GateKey) -> Self {
        Self {
            opened: Mutex::new(false),
            woken: Notify::new(),
            key: Some(key),
        }
    }

    /// Releases every waiter, permanently.
    ///
    /// Called once a helper has the immediate share, and also whenever the
    /// holding call cannot deliver it at all — a share that will not be posted
    /// must not keep the rest of the round waiting for its budget.
    pub(super) fn open(&self) {
        if let Ok(mut opened) = self.opened.lock() {
            *opened = true;
        }
        if let (Some(key), Ok(mut released)) = (self.key.clone(), RELEASED.lock()) {
            let (seen, order) = &mut *released;
            if seen.insert(key.clone(), ()).is_none() {
                order.push_back(key);
                while order.len() > RELEASED_ROUNDS {
                    if let Some(evicted) = order.pop_front() {
                        seen.remove(&evicted);
                    }
                }
            }
        }
        self.woken.notify_waiters();
    }

    fn is_open(&self) -> bool {
        self.opened.lock().map(|opened| *opened).unwrap_or(true)
    }

    /// Waits for the designated share to reach a helper, bounded by
    /// [`WAIT_BUDGET`].
    ///
    /// The caller establishes whether the share is *already* accepted before
    /// calling; this only waits. Nothing inside the loop touches storage, so the
    /// budget and the cancellation tick are the only things that end it and both
    /// are genuinely bounded — a durable read takes the sidecar connection, which
    /// delivery is using continuously, and re-reading it on every tick could
    /// block the task past its own deadline.
    ///
    /// That costs nothing in practice. The durable read answers "was this
    /// accepted by an earlier pass, or before a restart", which cannot change
    /// while this call waits. A share being accepted *now* is being accepted by a
    /// sibling call in this process, which signals through the notification.
    ///
    /// Registers for notification before testing, so an `open` racing this call
    /// is observed rather than missed.
    pub(super) async fn wait(&self, cancel: &(dyn Fn() -> bool + Send + Sync)) -> GateWait {
        let deadline = tokio::time::Instant::now() + WAIT_BUDGET;
        loop {
            let woken = self.woken.notified();
            tokio::pin!(woken);
            if self.is_open() {
                return GateWait::Accepted;
            }
            if cancel() {
                return GateWait::Cancelled;
            }
            if tokio::time::Instant::now() >= deadline {
                return GateWait::Expired;
            }
            tokio::select! {
                biased;
                _ = &mut woken => {
                    if self.is_open() {
                        return GateWait::Accepted;
                    }
                }
                _ = tokio::time::sleep_until(deadline) => return GateWait::Expired,
                _ = tokio::time::sleep(CANCEL_CHECK) => {}
            }
        }
    }
}

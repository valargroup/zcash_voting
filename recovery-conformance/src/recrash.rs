//! Crashing again while recovering.
//!
//! Every other exercise in this suite proves its claim over exactly one fault.
//! The armed run kills the child once, and everything after that is a clean
//! resume — so the recovery path's own durability is the one thing the crash
//! matrix cannot ask about. Dying midway through resolving a hashless dispatch,
//! or after normalization has written `Recovering` but before the retry
//! reserves, leaves durable state no single-fault run ever produces.
//!
//! It is also the shape a real user hits. An app that crashes once usually
//! crashes again for the same reason — a device under memory pressure kills the
//! same process at the same point on every launch — so "the user can always
//! recover" is a claim about repeated faults, not one.
//!
//! # Why every second stage is guaranteed to fire
//!
//! `run_until_crash` treats a round that finished without the seam firing as a
//! failure, because a completed round satisfies every assertion about "the
//! state a crash left". That rule is what keeps the matrix from rotting, and it
//! applies just as much to a second crash as to a first.
//!
//! So the pairs below are chosen so the resumed run *must* pass through the
//! second seam, never merely *may*. That rules out the obvious pairing of
//! `after-broadcast-unread` with itself: a hashless dispatch may be resolved by
//! scanning the tree instead of by re-POSTing, and which route wins is the
//! SDK's choice rather than something a test may require. Pairing it with a
//! later, unavoidable seam asks the same question — can recovery be interrupted
//! again and still converge — without encoding a guess about the route.

use crate::stages::CrashStage;

/// One ordered sequence of crashes over a single round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecrashCase {
    name: &'static str,
    first: CrashStage,
    again: &'static [CrashStage],
    asks: &'static str,
}

impl RecrashCase {
    /// The selector name, as `RECOVERY_CONFORMANCE_RECRASH` accepts it.
    pub fn name(self) -> &'static str {
        self.name
    }

    /// The stage the first, fresh run is armed at.
    pub fn first(self) -> CrashStage {
        self.first
    }

    /// The stages each subsequent recovery run is armed at, in order. Never
    /// empty: a case with nothing here is an ordinary crash stage.
    pub fn again(self) -> &'static [CrashStage] {
        self.again
    }

    /// What this sequence can ask that no single-fault run can.
    pub fn asks(self) -> &'static str {
        self.asks
    }

    /// Total crashes, first and subsequent.
    pub fn crashes(self) -> usize {
        1 + self.again.len()
    }

    /// Every stage this case arms, in the order it arms them.
    pub fn stages(self) -> impl Iterator<Item = CrashStage> {
        std::iter::once(self.first).chain(self.again.iter().copied())
    }
}

impl std::fmt::Display for RecrashCase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name)
    }
}

impl std::str::FromStr for RecrashCase {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ALL.iter()
            .find(|case| case.name == value)
            .copied()
            .ok_or_else(|| format!("unknown recrash case {value:?}"))
    }
}

/// Every crash-during-recovery case.
///
/// Five rather than the nineteen-squared grid, and chosen for where a bug costs
/// a voter a note rather than for coverage. Each costs a provisioned round and
/// the crashes within it, so a case earns its place by asking something the
/// others cannot.
pub const ALL: &[RecrashCase] = &[
    // The reservation that survived one crash must survive the next. A row that
    // vanished on the second open would let the following pass reserve a fresh
    // first attempt and build a second transaction spending the same notes —
    // the `B1` claim, asked of a round that has already been interrupted once.
    // The resume must dispatch the recovering reservation, so the seam is
    // unavoidable.
    RecrashCase {
        name: "before-broadcast-twice",
        first: CrashStage::BeforeBroadcast,
        again: &[CrashStage::BeforeBroadcast],
        asks: "does a reservation survive a second crash without licensing a new generation",
    },
    // A hashless dispatch resolving while a later crash lands in the same
    // recovery. Deliberately not paired with itself: resolution may go through
    // the tree rather than a retry, and requiring the POST seam would encode a
    // guess about which route wins. Shares are posted after confirmation either
    // way, so this seam is reached however the hash was resolved.
    RecrashCase {
        name: "hashless-then-shares",
        first: CrashStage::AfterBroadcastUnread,
        again: &[CrashStage::AfterShareAccepted],
        asks: "can a hashless dispatch resolve across a second, later crash",
    },
    // The narrow window between a committed vote and its durable helper plans,
    // interrupted twice in a row. Nine votes in this round, so a crash on one
    // vote's preflight leaves later votes to reach the plan boundary.
    RecrashCase {
        name: "commit-then-plans",
        first: CrashStage::AfterVoteCommit,
        again: &[CrashStage::AfterHelperPlans],
        asks: "does exactly one plan per vote survive two crashes around the commit",
    },
    // Acceptances recorded by two separately killed processes must both
    // survive, with the second kill landing during a resumed round's *initial*
    // helper delivery.
    //
    // This was first written as `after-share-accepted` twice, on the reasoning
    // that a round places 144 shares so a second run has plenty left to accept.
    // Two live runs showed that reasoning to be wrong, and the distinction it
    // missed is the useful part: once a bundle's initial delivery has begun,
    // what a resume owes is *recovery* of those shares, and the resumed run
    // reached `BackgroundShareWorkOnly` — terminal success — without the seam
    // firing at all. Crashing at `after-helper-plans` instead leaves bundle 0's
    // initial delivery entirely unstarted, so the resume must still perform it.
    RecrashCase {
        name: "plans-then-shares",
        first: CrashStage::AfterHelperPlans,
        again: &[CrashStage::AfterShareAccepted],
        asks: "does a kill during a resumed round's initial helper delivery keep its acceptances",
    },
    // The crash loop. Three kills at the same boundary before any clean run, so
    // the round is opened four times and must still converge. This is the case
    // that most directly answers the question the suite exists for: a user
    // whose app dies at the same point every launch can still finish voting.
    RecrashCase {
        name: "before-broadcast-thrice",
        first: CrashStage::BeforeBroadcast,
        again: &[CrashStage::BeforeBroadcast, CrashStage::BeforeBroadcast],
        asks: "does a round survive a crash loop at one boundary and still converge",
    },
];

/// The cases this run exercises, or `None` for all of them.
///
/// Set `RECOVERY_CONFORMANCE_RECRASH` to a comma-separated list of case names.
/// An unrecognized name panics rather than selecting nothing, for the same
/// reason the stage selector does: a typo that quietly ran nothing would report
/// a green matrix having tested nothing.
pub fn selected() -> Option<Vec<RecrashCase>> {
    let requested = std::env::var("RECOVERY_CONFORMANCE_RECRASH").ok()?;
    let cases: Vec<RecrashCase> = requested
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            name.parse::<RecrashCase>().unwrap_or_else(|_| {
                panic!(
                    "RECOVERY_CONFORMANCE_RECRASH names an unknown case {name:?}; \
                     known cases are {}",
                    ALL.iter()
                        .map(|case| case.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect();
    (!cases.is_empty()).then_some(cases)
}

//! Streaming exact commitment-tree recovery for sticky bound generations.

use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use incrementalmerkletree::frontier::Frontier;
use serde::Deserialize;
use vote_commitment_tree::{MerkleHashVote, TREE_CAPACITY, TREE_DEPTH};

use super::{
    coordination::{CapturedSubmissionOperation, SubmissionOperationLease},
    generation::DerivedChainSubmission,
    protocol::ChainProtocolClient,
    CandidateTransactionHash, ChainHttpRequest, ChainSubmissionDiagnostic,
    ChainSubmissionDiagnosticKind, ChainTransport, ChainTransportError,
};

const VOTE_SDK_PAGE_LEAF_TARGET: u64 = 5_000;
const MAX_RECOVERY_LEAF_REQUESTS: usize =
    maximum_whole_block_page_count(TREE_CAPACITY, VOTE_SDK_PAGE_LEAF_TARGET) as usize;
const MAX_RECOVERY_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Independent transfer budget for one continuously locked recovery pass.
///
/// This is deliberately not derived from the request ceiling: multiplying two
/// individually generous limits would permit a hostile endpoint to retain the
/// lifecycle lease while transferring tens of GiB of irrelevant JSON.
const MAX_RECOVERY_TOTAL_BYTES: u64 = 1_000_000_000;
const RECOVERY_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const RECOVERY_PASS_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Maximum pages produced by vote-sdk's greedy, whole-block pagination.
///
/// Two consecutive non-final pages contain at least `target + 1` leaves:
/// otherwise the second page's first indivisible block would have fit on the
/// first page.
const fn maximum_whole_block_page_count(leaves: u64, target: u64) -> u64 {
    let paired_leaf_count = target + 1;
    let complete_page_pairs = leaves / paired_leaf_count;
    let trailing_page = if leaves.is_multiple_of(paired_leaf_count) {
        0
    } else {
        1
    };
    complete_page_pairs * 2 + trailing_page
}

#[derive(Debug)]
pub(super) enum RecoveryScanFailure {
    Transport(ChainTransportError),
    Invalid(ChainSubmissionDiagnostic),
    AmbiguousLayout(ChainSubmissionDiagnostic),
    Interrupted,
}

pub(super) enum RecoveryScanOutcome<'a> {
    Match {
        final_van_position: u64,
        vote_commitment_positions: Vec<u64>,
    },
    NoMatch(RecoveryRetryAuthorization<'a>),
}

/// Single-use proof that one continuously locked pass scanned a complete fixed
/// snapshot without finding the generation's exact output layout.
pub(super) struct RecoveryRetryAuthorization<'a> {
    operation: &'a CapturedSubmissionOperation,
    _lease: &'a SubmissionOperationLease,
    generation_digest: super::ChainSubmissionGenerationDigest,
    candidate: Option<CandidateTransactionHash>,
}

impl RecoveryRetryAuthorization<'_> {
    pub(super) fn operation(&self) -> &CapturedSubmissionOperation {
        self.operation
    }

    pub(super) fn generation_digest(&self) -> super::ChainSubmissionGenerationDigest {
        self.generation_digest
    }

    pub(super) fn candidate(&self) -> Option<CandidateTransactionHash> {
        self.candidate
    }
}

#[derive(Deserialize)]
struct LatestResponse {
    tree: Option<TreeState>,
}

#[derive(Deserialize)]
struct TreeState {
    #[serde(default)]
    next_index: u64,
    root: Option<String>,
    #[serde(default)]
    height: u64,
}

#[derive(Deserialize)]
struct LeavesResponse {
    #[serde(default)]
    blocks: Vec<LeafBlock>,
    #[serde(default)]
    next_from_height: u64,
}

#[derive(Deserialize)]
struct LeafBlock {
    #[serde(default)]
    height: u64,
    #[serde(default)]
    start_index: u64,
    #[serde(default)]
    leaves: Vec<String>,
    root: Option<String>,
}

struct RecoveryPassBudget {
    deadline: tokio::time::Instant,
    leaf_request_count: usize,
    response_bytes: u64,
}

impl RecoveryPassBudget {
    fn new() -> Self {
        Self {
            deadline: tokio::time::Instant::now() + RECOVERY_PASS_TIMEOUT,
            leaf_request_count: 0,
            response_bytes: 0,
        }
    }

    fn begin_leaf_request(&mut self) -> Result<(), RecoveryScanFailure> {
        if self.leaf_request_count >= MAX_RECOVERY_LEAF_REQUESTS
            || tokio::time::Instant::now() >= self.deadline
        {
            return Err(bounded_pass_exhausted());
        }
        self.leaf_request_count += 1;
        Ok(())
    }

    fn charge_response(&mut self, response_bytes: usize) -> Result<(), RecoveryScanFailure> {
        self.response_bytes = self.response_bytes.saturating_add(response_bytes as u64);
        if self.response_bytes > MAX_RECOVERY_TOTAL_BYTES {
            return Err(RecoveryScanFailure::Invalid(invalid(
                "tree recovery responses exceed the total byte limit",
            )));
        }
        Ok(())
    }

    fn ensure_time_remaining(&self) -> Result<(), RecoveryScanFailure> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(bounded_pass_exhausted());
        }
        Ok(())
    }

    fn cannot_fail_over(&self) -> bool {
        self.leaf_request_count >= MAX_RECOVERY_LEAF_REQUESTS
            || self.response_bytes > MAX_RECOVERY_TOTAL_BYTES
            || tokio::time::Instant::now() >= self.deadline
    }
}

enum ReplicaScanOutcome {
    Match {
        final_van_position: u64,
        vote_commitment_positions: Vec<u64>,
    },
    NoMatch,
}

#[derive(Clone, Copy)]
struct RecoveryReplica<'a> {
    endpoint: &'a str,
    endpoint_index: usize,
}

pub(super) async fn scan_exact_layout<'a, T: ChainTransport>(
    protocol: &ChainProtocolClient<T>,
    derived: &DerivedChainSubmission,
    candidate: Option<CandidateTransactionHash>,
    operation: &'a CapturedSubmissionOperation,
    lease: &'a SubmissionOperationLease,
    interrupted: impl Fn() -> bool,
    observations: &crate::ObservationScope,
) -> Result<RecoveryScanOutcome<'a>, RecoveryScanFailure> {
    let round = hex::encode(derived.generation().identity().vote_round_id());
    let expected = derived.expected_layout().leaves();
    let mut budget = RecoveryPassBudget::new();
    let mut last_failure = None;

    for (endpoint_index, endpoint) in protocol.endpoints().iter().enumerate() {
        if interrupted() {
            return Err(RecoveryScanFailure::Interrupted);
        }
        match scan_replica(
            protocol.transport(),
            RecoveryReplica {
                endpoint,
                endpoint_index,
            },
            &round,
            &expected,
            &mut budget,
            &interrupted,
            observations,
        )
        .await
        {
            Ok(ReplicaScanOutcome::Match {
                final_van_position,
                vote_commitment_positions,
            }) => {
                return Ok(RecoveryScanOutcome::Match {
                    final_van_position,
                    vote_commitment_positions,
                })
            }
            Ok(ReplicaScanOutcome::NoMatch) => {
                return Ok(RecoveryScanOutcome::NoMatch(RecoveryRetryAuthorization {
                    operation,
                    _lease: lease,
                    generation_digest: derived.generation().digest(),
                    candidate,
                }))
            }
            Err(RecoveryScanFailure::Interrupted) => return Err(RecoveryScanFailure::Interrupted),
            Err(failure @ RecoveryScanFailure::AmbiguousLayout(_)) => return Err(failure),
            Err(failure) => {
                if budget.cannot_fail_over() {
                    return Err(failure);
                }
                last_failure = Some(failure);
            }
        }
    }

    Err(last_failure.unwrap_or_else(|| {
        RecoveryScanFailure::Invalid(invalid("tree recovery has no configured endpoint"))
    }))
}

/// Scans one replica from its own metadata response and never carries partial
/// snapshot state to another replica.
async fn scan_replica<T: ChainTransport>(
    transport: &T,
    replica: RecoveryReplica<'_>,
    round: &str,
    expected: &[[u8; 32]],
    budget: &mut RecoveryPassBudget,
    interrupted: &impl Fn() -> bool,
    observations: &crate::ObservationScope,
) -> Result<ReplicaScanOutcome, RecoveryScanFailure> {
    let latest_url = format!(
        "{}/shielded-vote/v1/commitment-tree/{round}/latest",
        replica.endpoint
    );
    let latest: LatestResponse = get_json(
        transport,
        latest_url,
        replica.endpoint_index,
        budget,
        interrupted,
        observations,
    )
    .await?;
    let snapshot = latest.tree.ok_or_else(|| {
        RecoveryScanFailure::Invalid(invalid("tree recovery latest response omitted tree state"))
    })?;
    if snapshot.next_index > TREE_CAPACITY || snapshot.height > u32::MAX as u64 {
        return Err(RecoveryScanFailure::Invalid(invalid(
            "tree recovery snapshot exceeds protocol bounds",
        )));
    }
    let snapshot_root = match snapshot.root.as_deref() {
        Some(value) if !value.is_empty() => Some(decode_leaf(value, "snapshot root")?),
        None | Some("") if snapshot.next_index == 0 => None,
        _ => {
            return Err(RecoveryScanFailure::Invalid(invalid(
                "nonempty tree recovery snapshot omitted its root",
            )))
        }
    };
    let mut frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
    let mut window: std::collections::VecDeque<[u8; 32]> =
        std::collections::VecDeque::with_capacity(expected.len());
    let mut next_index = 0_u64;
    let mut from_height = 0_u64;
    let mut previous_height = None;
    let mut unpaired_nonfinal_page_leaves = None;
    let mut match_start = None;

    while next_index < snapshot.next_index {
        if interrupted() {
            return Err(RecoveryScanFailure::Interrupted);
        }
        budget.begin_leaf_request()?;
        let url = format!(
            "{}/shielded-vote/v1/commitment-tree/{round}/leaves?from_height={from_height}&to_height={}",
            replica.endpoint,
            snapshot.height
        );
        let page: LeavesResponse = get_json(
            transport,
            url,
            replica.endpoint_index,
            budget,
            interrupted,
            observations,
        )
        .await?;
        let page_leaf_count: usize = page.blocks.iter().map(|block| block.leaves.len()).sum();
        if page_leaf_count as u64 > snapshot.next_index.saturating_sub(next_index) {
            return Err(RecoveryScanFailure::Invalid(invalid(
                "tree recovery page exceeds the fixed snapshot",
            )));
        }
        for block in page.blocks {
            if block.height > snapshot.height
                || previous_height.is_some_and(|height| block.height <= height)
                || (!block.leaves.is_empty() && block.start_index != next_index)
            {
                return Err(RecoveryScanFailure::Invalid(invalid(
                    "tree recovery block sequence is discontinuous",
                )));
            }
            previous_height = Some(block.height);
            for encoded in block.leaves {
                if next_index >= snapshot.next_index
                    || !frontier.append(decode_leaf(&encoded, "tree leaf")?)
                {
                    return Err(RecoveryScanFailure::Invalid(invalid(
                        "tree recovery leaf sequence exceeds the fixed snapshot",
                    )));
                }
                let bytes = BASE64_STANDARD.decode(encoded).map_err(|_| {
                    RecoveryScanFailure::Invalid(invalid("tree recovery leaf is invalid base64"))
                })?;
                window.push_back(bytes.try_into().map_err(|_| {
                    RecoveryScanFailure::Invalid(invalid("tree recovery leaf has invalid length"))
                })?);
                if window.len() > expected.len() {
                    window.pop_front();
                }
                next_index += 1;
                if window.len() == expected.len()
                    && window.iter().copied().eq(expected.iter().copied())
                {
                    let start = next_index - expected.len() as u64;
                    if match_start.replace(start).is_some() {
                        return Err(RecoveryScanFailure::AmbiguousLayout(invalid(
                            "tree recovery found multiple exact generation layouts",
                        )));
                    }
                }
            }
            let block_root = decode_leaf(
                block.root.as_deref().ok_or_else(|| {
                    RecoveryScanFailure::Invalid(invalid("tree recovery block omitted its root"))
                })?,
                "block root",
            )?;
            if frontier.root() != block_root {
                return Err(RecoveryScanFailure::Invalid(invalid(
                    "tree recovery block root contradicts its leaves",
                )));
            }
        }
        if next_index < snapshot.next_index {
            if page_leaf_count == 0 {
                return Err(RecoveryScanFailure::Invalid(invalid(
                    "tree recovery page made no leaf progress",
                )));
            }
            if let Some(previous_page_leaves) = unpaired_nonfinal_page_leaves.take() {
                if previous_page_leaves + (page_leaf_count as u64) < VOTE_SDK_PAGE_LEAF_TARGET + 1 {
                    return Err(RecoveryScanFailure::Invalid(invalid(
                        "tree recovery pages violate the pagination progress bound",
                    )));
                }
            } else {
                unpaired_nonfinal_page_leaves = Some(page_leaf_count as u64);
            }
            if page.next_from_height <= from_height || page.next_from_height > snapshot.height {
                return Err(RecoveryScanFailure::Invalid(invalid(
                    "tree recovery pagination cursor is invalid",
                )));
            }
            from_height = page.next_from_height;
        } else if page.next_from_height != 0 {
            return Err(RecoveryScanFailure::Invalid(invalid(
                "tree recovery pagination continued beyond the fixed snapshot",
            )));
        }
    }
    budget.ensure_time_remaining()?;
    if snapshot_root.is_some_and(|root| frontier.root() != root) {
        return Err(RecoveryScanFailure::Invalid(invalid(
            "tree recovery final root contradicts the fixed snapshot",
        )));
    }
    if let Some(start) = match_start {
        return Ok(ReplicaScanOutcome::Match {
            final_van_position: start,
            vote_commitment_positions: (1..expected.len())
                .map(|offset| start + offset as u64)
                .collect(),
        });
    }
    Ok(ReplicaScanOutcome::NoMatch)
}

async fn get_json<T: ChainTransport, R: for<'de> Deserialize<'de>>(
    transport: &T,
    url: String,
    endpoint_index: usize,
    budget: &mut RecoveryPassBudget,
    interrupted: &impl Fn() -> bool,
    observations: &crate::ObservationScope,
) -> Result<R, RecoveryScanFailure> {
    let timer = observations.stage("chain.recovery_get");
    let mut http_status = None;
    let result = async {
        if interrupted() {
            return Err(RecoveryScanFailure::Interrupted);
        }
        let request = ChainHttpRequest::new(
            url,
            vec![("accept".to_string(), "application/json".to_string())],
            RECOVERY_REQUEST_TIMEOUT,
            MAX_RECOVERY_RESPONSE_BYTES,
        );
        let request_deadline =
            (tokio::time::Instant::now() + RECOVERY_REQUEST_TIMEOUT).min(budget.deadline);
        let response = tokio::time::timeout_at(request_deadline, transport.chain_get(request))
            .await
            .map_err(|_| {
                if tokio::time::Instant::now() >= budget.deadline {
                    return bounded_pass_exhausted();
                }
                RecoveryScanFailure::Transport(ChainTransportError::possibly_dispatched(
                    "tree recovery request timed out",
                ))
            })?
            .map_err(RecoveryScanFailure::Transport)?;
        http_status = Some(response.status());
        budget.charge_response(response.body().len())?;
        let has_json_content_type = response.content_type().is_some_and(|content_type| {
            content_type.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("application/json")
            })
        });
        if response.status() != 200
            || response.body().len() > MAX_RECOVERY_RESPONSE_BYTES
            || !has_json_content_type
        {
            return Err(RecoveryScanFailure::Invalid(invalid(
                "tree recovery response has invalid HTTP metadata",
            )));
        }
        serde_json::from_slice(response.body()).map_err(|_| {
            RecoveryScanFailure::Invalid(invalid("tree recovery response is malformed"))
        })
    }
    .await;
    let (outcome, error_kind) = match &result {
        Ok(_) => (crate::ObservationOutcome::Succeeded, None),
        Err(RecoveryScanFailure::Interrupted) => {
            (crate::ObservationOutcome::Cancelled, Some("Interrupted"))
        }
        Err(RecoveryScanFailure::Transport(_)) => {
            (crate::ObservationOutcome::Failed, Some("Transport"))
        }
        Err(_) => (crate::ObservationOutcome::Failed, Some("Protocol")),
    };
    timer.finish_http(
        outcome,
        error_kind,
        http_status,
        u32::try_from(endpoint_index).ok(),
    );
    result
}

fn bounded_pass_exhausted() -> RecoveryScanFailure {
    RecoveryScanFailure::Invalid(invalid("tree recovery exhausted its bounded pass"))
}

fn decode_leaf(encoded: &str, label: &str) -> Result<MerkleHashVote, RecoveryScanFailure> {
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| RecoveryScanFailure::Invalid(invalid(format!("{label} is invalid base64"))))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        RecoveryScanFailure::Invalid(invalid(format!("{label} has invalid length")))
    })?;
    MerkleHashVote::from_bytes(&bytes)
        .ok_or_else(|| RecoveryScanFailure::Invalid(invalid(format!("{label} is noncanonical"))))
}

fn invalid(message: impl AsRef<str>) -> ChainSubmissionDiagnostic {
    ChainSubmissionDiagnostic::from_redacted_message(
        ChainSubmissionDiagnosticKind::InvalidProtocolResponse,
        message,
    )
}

#[cfg(test)]
mod tests;

//! Vote-chain endpoint construction and wire-protocol interpretation.

use std::{str::FromStr, time::Duration};

use serde::Deserialize;
use url::{Host, Url};

use crate::{
    confirmation::TxEvent,
    delegation_key_fingerprint::{DelegationKeyFingerprint, DELEGATION_KEY_FINGERPRINT_HEADER},
    proxy::batch::ProxyDelegationBatchWire,
    types::Network,
    wire::{DelegationSubmissionWire, VoteCommitmentWire},
};

use super::{
    CandidateTransactionHash, ChainHttpRequest, ChainHttpResponse, ChainPostDispatch,
    ChainSubmissionDiagnostic, ChainSubmissionDiagnosticKind, ChainTransport, ChainTransportError,
    ChainTransportFailureKind, MAX_CHAIN_HTTP_RESPONSE_BYTES,
};

use crate::ingress_timeout;

const API_PREFIX: [&str; 2] = ["shielded-vote", "v1"];
const DELEGATION_ENDPOINT: &str = "delegate-vote";
const VOTE_ENDPOINT: &str = "cast-vote";
const VOTE_BATCH_ENDPOINT: &str = "cast-vote-batch";
const PROXY_DELEGATION_BATCH_ENDPOINT: &str = "proxy-delegation-batch";
/// Opts every submission and `/tx/{hash}` lookup into the `codespace` field
/// (contracts-v1 §0). A node that ignores it answers as before.
const CODESPACE_HEADER: &str = "x-svote-codespace";
/// The vote chain's height route (contracts-v1 §8). A dormant node answers 404.
const CHAIN_HEIGHT_PATH: &str = "shielded-vote/v1/height";
/// The Cosmos SDK validator-set route, the height fallback for a node without
/// [`CHAIN_HEIGHT_PATH`].
pub(super) const LATEST_VALIDATOR_SET_PATH: &str =
    "cosmos/base/tendermint/v1beta1/validatorsets/latest?pagination.limit=1";
// CheckTx may use its complete 120-second server budget to verify a proof. The
// client default leaves time for connection setup and response delivery.
const MIN_CHAIN_POST_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_CHAIN_POST_TIMEOUT: Duration = Duration::from_secs(150);
const DEFAULT_CHAIN_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CHAIN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_CHAIN_HTTP_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_CHAIN_ENDPOINTS: usize = 100;
/// Code 2 as a node without codespaces reports a spent nullifier.
const NULLIFIER_ALREADY_SPENT_CODE: u32 = 2;

/// How a chain rejection steers the lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainRejectionKind {
    /// A nullifier is already spent: code 2, in codespace `vote` for a 0x09
    /// batch.
    NullifierAlreadySpent,
    /// `vote` 51: coordinators paused DC batches. A 0x09 batch only.
    ProxyBatchPaused,
    /// `vote` 52: the attempt expired, which proves no attempt of the batch
    /// landed or can. A 0x09 batch only.
    ProxyBatchExpired,
    /// Any other rejection.
    Other,
}

/// Classifies a delegation or cast (0x02 to 0x07) rejection on (codespace,
/// code) (contracts-v1 §0, §6). Only `vote` 2 is a spent nullifier. Without a
/// codespace the code alone is used.
fn cast_rejection_kind(codespace: Option<&str>, code: u32) -> ChainRejectionKind {
    use zcash_vote_delegate::ProxyChainError;
    let spent = match codespace {
        Some(codespace) => matches!(
            ProxyChainError::from_codespace_and_code(codespace, code),
            Some(ProxyChainError::DuplicateNullifier)
        ),
        None => code == NULLIFIER_ALREADY_SPENT_CODE,
    };
    if spent {
        ChainRejectionKind::NullifierAlreadySpent
    } else {
        ChainRejectionKind::Other
    }
}

/// Classifies a 0x09 rejection on (codespace, code) (contract §6). Only
/// codespace `vote` names a registered error.
pub(super) fn proxy_batch_rejection_kind(codespace: &str, code: u32) -> ChainRejectionKind {
    use zcash_vote_delegate::ProxyChainError;
    match ProxyChainError::from_codespace_and_code(codespace, code) {
        Some(ProxyChainError::DuplicateNullifier) => ChainRejectionKind::NullifierAlreadySpent,
        Some(ProxyChainError::ProxyDelegationPaused) => ChainRejectionKind::ProxyBatchPaused,
        Some(ProxyChainError::ProxyBatchExpired) => ChainRejectionKind::ProxyBatchExpired,
        _ => ChainRejectionKind::Other,
    }
}

/// Which response contract a mutation endpoint follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseShape {
    /// The deployed `BroadcastResult`, with or without a batch digest and
    /// with an optional `codespace` (contracts-v1 §0).
    Legacy {
        expected_batch_digest: Option<[u8; 32]>,
    },
    /// The 0x09 endpoint's `BroadcastResult` (contracts-v1 §0, §8).
    ProxyBatch { expected_batch_digest: [u8; 32] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChainProtocolTiming {
    post_timeout: Duration,
    lookup_timeout: Duration,
}

impl ChainProtocolTiming {
    pub(super) fn new(
        post_timeout: Duration,
        lookup_timeout: Duration,
    ) -> Result<Self, ChainSubmissionDiagnostic> {
        if !(MIN_CHAIN_POST_TIMEOUT..=MAX_CHAIN_REQUEST_TIMEOUT).contains(&post_timeout) {
            return Err(invalid_protocol(
                "vote-chain POST timeout must be between 120 and 600 seconds",
            ));
        }
        if lookup_timeout.is_zero() || lookup_timeout > MAX_CHAIN_REQUEST_TIMEOUT {
            return Err(invalid_protocol(
                "vote-chain lookup timeout must be between 1 nanosecond and 600 seconds",
            ));
        }
        Ok(Self {
            post_timeout,
            lookup_timeout,
        })
    }
}

impl Default for ChainProtocolTiming {
    fn default() -> Self {
        Self {
            post_timeout: DEFAULT_CHAIN_POST_TIMEOUT,
            lookup_timeout: DEFAULT_CHAIN_LOOKUP_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PostAttemptOutcome {
    Accepted(CandidateTransactionHash),
    Rejected {
        code: u32,
        kind: ChainRejectionKind,
        diagnostic: ChainSubmissionDiagnostic,
        candidate_transaction_hash: Option<CandidateTransactionHash>,
    },
    LocalFailure(ChainSubmissionDiagnostic),
    DefinitelyUnsent(ChainTransportError),
    /// The trusted REST ingress rejected this attempt before broadcast.
    NotDispatchedByServer,
    PossiblyDispatched(ChainSubmissionDiagnostic),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedTransaction {
    pub(super) height: u64,
    pub(super) code: u32,
    /// The result's ABCI codespace, absent from a node that ignores the header.
    pub(super) codespace: Option<String>,
    pub(super) events: Vec<TxEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TransactionStatusObservation {
    Pending,
    CommittedSuccess(CommittedTransaction),
    CommittedFailure(CommittedTransaction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LookupFailure {
    Transport(ChainTransportError),
    Protocol(ChainSubmissionDiagnostic),
}

pub(super) struct ChainProtocolClient<T> {
    transport: T,
    endpoints: Vec<String>,
    timing: ChainProtocolTiming,
}

impl<T: ChainTransport> ChainProtocolClient<T> {
    pub(super) fn transport(&self) -> &T {
        &self.transport
    }

    pub(super) fn endpoints(&self) -> &[String] {
        &self.endpoints
    }
    pub(super) fn new(
        transport: T,
        network: Network,
        endpoints: &[String],
    ) -> Result<Self, ChainSubmissionDiagnostic> {
        Self::with_timing(
            transport,
            network,
            endpoints,
            ChainProtocolTiming::default(),
        )
    }

    pub(super) fn with_timing(
        transport: T,
        network: Network,
        endpoints: &[String],
        timing: ChainProtocolTiming,
    ) -> Result<Self, ChainSubmissionDiagnostic> {
        ChainProtocolTiming::new(timing.post_timeout, timing.lookup_timeout)?;
        if endpoints.is_empty() {
            return Err(invalid_protocol(
                "vote-chain endpoint set must not be empty",
            ));
        }
        if endpoints.len() > MAX_CHAIN_ENDPOINTS {
            return Err(invalid_protocol(format!(
                "vote-chain endpoint set exceeds {MAX_CHAIN_ENDPOINTS} endpoint limit"
            )));
        }

        let mut canonical = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            let endpoint = canonicalize_chain_base_url(endpoint, network)?;
            if canonical.contains(&endpoint) {
                return Err(invalid_protocol(
                    "vote-chain endpoint set contains a duplicate canonical endpoint",
                ));
            }
            canonical.push(endpoint);
        }

        Ok(Self {
            transport,
            endpoints: canonical,
            timing,
        })
    }

    /// Number of distinct canonical mutation endpoints available for failover.
    pub(super) fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }

    #[cfg(test)]
    pub(super) async fn submit_delegation(
        &self,
        endpoint_index: usize,
        submission: &DelegationSubmissionWire,
    ) -> PostAttemptOutcome {
        self.submit_delegation_with_dispatch(
            endpoint_index,
            submission,
            None,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await
    }

    /// Posts a standalone delegation.
    ///
    /// `proof_key_fingerprint` names the circuit key that made the proof in
    /// `submission`; when present it is sent in the
    /// [`DELEGATION_KEY_FINGERPRINT_HEADER`] header beside the unchanged body.
    pub(super) async fn submit_delegation_with_dispatch(
        &self,
        endpoint_index: usize,
        submission: &DelegationSubmissionWire,
        proof_key_fingerprint: Option<DelegationKeyFingerprint>,
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let body = match submission.to_json() {
            Ok(json) => json.into_bytes(),
            Err(error) => {
                return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                    "serialize delegation request failed: {error}"
                )))
            }
        };
        self.post(
            endpoint_index,
            ChainPost {
                endpoint: DELEGATION_ENDPOINT,
                body,
                shape: ResponseShape::Legacy {
                    expected_batch_digest: None,
                },
                delegation_key_fingerprint: proof_key_fingerprint,
            },
            dispatch,
            observations,
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn submit_vote(
        &self,
        endpoint_index: usize,
        submission: &VoteCommitmentWire,
    ) -> PostAttemptOutcome {
        self.submit_vote_with_dispatch(
            endpoint_index,
            submission,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await
    }

    pub(super) async fn submit_vote_with_dispatch(
        &self,
        endpoint_index: usize,
        submission: &VoteCommitmentWire,
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let body = match submission.to_json() {
            Ok(json) => json.into_bytes(),
            Err(error) => {
                return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                    "serialize vote request failed: {error}"
                )))
            }
        };
        self.post(
            endpoint_index,
            ChainPost {
                endpoint: VOTE_ENDPOINT,
                body,
                shape: ResponseShape::Legacy {
                    expected_batch_digest: None,
                },
                delegation_key_fingerprint: None,
            },
            dispatch,
            observations,
        )
        .await
    }

    pub(super) async fn submit_vote_batch_with_dispatch(
        &self,
        endpoint_index: usize,
        submission: &crate::wire::VoteCommitmentBatchWire,
        expected_batch_digest: [u8; 32],
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let body = match submission.to_json() {
            Ok(json) => json.into_bytes(),
            Err(error) => {
                return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                    "serialize vote-batch request failed: {error}"
                )))
            }
        };
        self.post(
            endpoint_index,
            ChainPost {
                endpoint: VOTE_BATCH_ENDPOINT,
                body,
                shape: ResponseShape::Legacy {
                    expected_batch_digest: Some(expected_batch_digest),
                },
                delegation_key_fingerprint: None,
            },
            dispatch,
            observations,
        )
        .await
    }

    /// Posts a delegation together with its first vote batch.
    ///
    /// `proof_key_fingerprint` names the circuit key that made the delegation
    /// proof in `submission`; when present it is sent in the
    /// [`DELEGATION_KEY_FINGERPRINT_HEADER`] header beside the unchanged body.
    pub(super) async fn submit_delegate_and_vote_batch_with_dispatch(
        &self,
        endpoint_index: usize,
        submission: &crate::wire::DelegateAndVoteBatchWire,
        expected_batch_digest: [u8; 32],
        proof_key_fingerprint: Option<DelegationKeyFingerprint>,
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let body = match submission.to_json() {
            Ok(json) => json.into_bytes(),
            Err(error) => {
                return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                    "serialize combined request failed: {error}"
                )))
            }
        };
        self.post(
            endpoint_index,
            ChainPost {
                endpoint: "delegate-and-cast-vote-batch",
                body,
                shape: ResponseShape::Legacy {
                    expected_batch_digest: Some(expected_batch_digest),
                },
                delegation_key_fingerprint: proof_key_fingerprint,
            },
            dispatch,
            observations,
        )
        .await
    }

    /// POSTs one signed attempt of a 0x09 batch. See
    /// [`parse_proxy_batch_response`] for how the answer is read.
    pub(super) async fn submit_proxy_delegation_batch_with_dispatch(
        &self,
        endpoint_index: usize,
        submission: &ProxyDelegationBatchWire,
        expected_batch_digest: [u8; 32],
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let body = match submission.to_json() {
            Ok(json) => json.into_bytes(),
            Err(error) => {
                return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                    "serialize proxy delegation batch failed: {error}"
                )))
            }
        };
        self.post(
            endpoint_index,
            ChainPost {
                endpoint: PROXY_DELEGATION_BATCH_ENDPOINT,
                body,
                shape: ResponseShape::ProxyBatch {
                    expected_batch_digest,
                },
                delegation_key_fingerprint: None,
            },
            dispatch,
            observations,
        )
        .await
    }

    async fn post(
        &self,
        endpoint_index: usize,
        post: ChainPost<'_>,
        dispatch: ChainPostDispatch,
        observations: &crate::ObservationScope,
    ) -> PostAttemptOutcome {
        let ChainPost {
            endpoint,
            body,
            shape,
            delegation_key_fingerprint,
        } = post;
        if body.len() > MAX_CHAIN_HTTP_REQUEST_BYTES {
            return PostAttemptOutcome::LocalFailure(invalid_protocol(format!(
                "vote-chain request exceeds {MAX_CHAIN_HTTP_REQUEST_BYTES} byte limit"
            )));
        }
        let Some(base_url) = self.endpoints.get(endpoint_index) else {
            return PostAttemptOutcome::LocalFailure(invalid_protocol(
                "vote-chain endpoint index is out of range",
            ));
        };
        let attempt_token = ingress_timeout::attempt_token();
        let mut request = chain_request(
            join_chain_url(base_url, &[endpoint]),
            true,
            self.timing.post_timeout,
        );

        if let Some(token) = &attempt_token {
            request.add_header(ingress_timeout::REQUEST_HEADER, token.clone());
        }
        if let Some(fingerprint) = delegation_key_fingerprint {
            request.add_header(DELEGATION_KEY_FINGERPRINT_HEADER, fingerprint.to_hex());
        }
        request.add_header(CODESPACE_HEADER, "1".to_string());

        let stage = observations.stage("chain::post_attempt");
        let dispatch_marker = dispatch.clone();
        let response = tokio::time::timeout(
            self.timing.post_timeout,
            self.transport
                .chain_post_json_with_dispatch(request, body, dispatch),
        )
        .await;
        let http_status = response
            .as_ref()
            .ok()
            .and_then(|response| response.as_ref().ok())
            .map(|response| response.status());
        let outcome = match response {
            // This deadline is enforced here, not by the transport. While the
            // dispatch marker is clear no request bytes reached a network
            // stack, so the attempt is definitely unsent rather than ambiguous.
            Err(_) if !dispatch_marker.is_possible() => {
                PostAttemptOutcome::DefinitelyUnsent(ChainTransportError::definitely_unsent(
                    "vote-chain submission timed out before the transport released the request",
                ))
            }
            Err(_) => PostAttemptOutcome::PossiblyDispatched(
                ChainSubmissionDiagnostic::from_redacted_message(
                    ChainSubmissionDiagnosticKind::AmbiguousDispatch,
                    "vote-chain submission timed out",
                ),
            ),
            Ok(Ok(response)) => {
                if validate_json_response(&response).is_ok()
                    && ingress_timeout::is_not_dispatched(
                        response.status(),
                        response.body(),
                        attempt_token.as_deref(),
                    )
                {
                    PostAttemptOutcome::NotDispatchedByServer
                } else {
                    parse_post_response(response, endpoint, shape)
                }
            }
            Ok(Err(error)) if error.kind() == ChainTransportFailureKind::DefinitelyUnsent => {
                PostAttemptOutcome::DefinitelyUnsent(error)
            }
            Ok(Err(error)) => PostAttemptOutcome::PossiblyDispatched(
                ChainSubmissionDiagnostic::from_redacted_message(
                    ChainSubmissionDiagnosticKind::AmbiguousDispatch,
                    error.message(),
                ),
            ),
        };
        let (classification, error_kind) = match &outcome {
            PostAttemptOutcome::Accepted(_) => (crate::ObservationOutcome::Succeeded, None),
            PostAttemptOutcome::Rejected { .. } => {
                (crate::ObservationOutcome::Rejected, Some("ChainRejected"))
            }
            PostAttemptOutcome::PossiblyDispatched(_) => (
                crate::ObservationOutcome::PossiblyDispatched,
                Some("PossiblyDispatched"),
            ),
            PostAttemptOutcome::NotDispatchedByServer => (
                crate::ObservationOutcome::Failed,
                Some("NotDispatchedByServer"),
            ),
            PostAttemptOutcome::DefinitelyUnsent(_) => {
                (crate::ObservationOutcome::Failed, Some("DefinitelyUnsent"))
            }
            PostAttemptOutcome::LocalFailure(_) => {
                (crate::ObservationOutcome::Failed, Some("Protocol"))
            }
        };
        stage.finish_http(
            classification,
            error_kind,
            http_status,
            u32::try_from(endpoint_index).ok(),
        );
        outcome
    }

    /// The highest latest block height any endpoint reports, failing only when
    /// none answers. Each endpoint is read from [`CHAIN_HEIGHT_PATH`], then
    /// from [`LATEST_VALIDATOR_SET_PATH`] when that gives no height.
    pub(super) async fn latest_block_height(
        &self,
        observations: &crate::ObservationScope,
    ) -> Result<u64, LookupFailure> {
        let operation = observations.stage("chain::latest_block_height");
        let mut highest = None;
        let mut last_failure = None;
        for base_url in &self.endpoints {
            let observed = match self
                .endpoint_height(base_url, CHAIN_HEIGHT_PATH, parse_chain_height_response)
                .await
            {
                Err(LookupFailure::Protocol(_)) => {
                    self.endpoint_height(
                        base_url,
                        LATEST_VALIDATOR_SET_PATH,
                        parse_block_height_response,
                    )
                    .await
                }
                observed => observed,
            };
            match observed {
                Ok(height) => highest = Some(highest.map_or(height, |seen: u64| seen.max(height))),
                Err(failure) => last_failure = Some(failure),
            }
        }
        let result = highest.ok_or_else(|| {
            last_failure.unwrap_or_else(|| {
                LookupFailure::Protocol(invalid_protocol(
                    "vote-chain block height lookup produced no endpoint result",
                ))
            })
        });
        operation.finish(
            if result.is_ok() {
                crate::ObservationOutcome::Succeeded
            } else {
                crate::ObservationOutcome::Failed
            },
            result.as_ref().err().map(|_| "LookupFailure"),
        );
        result
    }

    /// One endpoint's height from `path`, read by `parse`.
    async fn endpoint_height(
        &self,
        base_url: &str,
        path: &str,
        parse: fn(&ChainHttpResponse) -> Result<u64, ChainSubmissionDiagnostic>,
    ) -> Result<u64, LookupFailure> {
        let request = chain_request(
            format!("{base_url}/{path}"),
            false,
            self.timing.lookup_timeout,
        );
        match tokio::time::timeout(
            self.timing.lookup_timeout,
            self.transport.chain_get(request),
        )
        .await
        {
            Ok(Ok(response)) => parse(&response).map_err(LookupFailure::Protocol),
            Ok(Err(error)) => Err(LookupFailure::Transport(error)),
            Err(_) => Err(LookupFailure::Transport(
                ChainTransportError::possibly_dispatched(
                    "vote-chain block height lookup timed out",
                ),
            )),
        }
    }

    /// Polls every endpoint for `transaction_hash`, preferring committed
    /// evidence over pending.
    pub(super) async fn transaction_status(
        &self,
        transaction_hash: CandidateTransactionHash,
        observations: &crate::ObservationScope,
    ) -> Result<TransactionStatusObservation, LookupFailure> {
        let operation = observations.stage("chain::status_lookup");
        let observations = operation.scope();
        let result = async {
            let hash = transaction_hash.to_hex();
            let mut saw_pending = false;
            let mut last_failure = None;

            for (endpoint_index, base_url) in self.endpoints.iter().enumerate() {
                let mut request = chain_request(
                    join_chain_url(base_url, &["tx", &hash]),
                    false,
                    self.timing.lookup_timeout,
                );
                request.add_header(CODESPACE_HEADER, "1".to_string());
                let attempt_context = observations
                    .attempt(u32::try_from(endpoint_index.saturating_add(1)).unwrap_or(u32::MAX));
                let stage = attempt_context.stage("chain::status_attempt");
                let response = tokio::time::timeout(
                    self.timing.lookup_timeout,
                    self.transport.chain_get(request),
                )
                .await;
                let status = response
                    .as_ref()
                    .ok()
                    .and_then(|response| response.as_ref().ok())
                    .map(|response| response.status());
                let observed = match response {
                    Ok(Ok(response)) => {
                        parse_status_response(response).map_err(LookupFailure::Protocol)
                    }
                    Ok(Err(error)) => Err(LookupFailure::Transport(error)),
                    Err(_) => Err(LookupFailure::Transport(
                        ChainTransportError::possibly_dispatched(
                            "vote-chain transaction lookup timed out",
                        ),
                    )),
                };
                let (outcome, error) = match &observed {
                    Ok(TransactionStatusObservation::Pending) => {
                        (crate::ObservationOutcome::Pending, None)
                    }
                    Ok(TransactionStatusObservation::CommittedFailure(_)) => {
                        (crate::ObservationOutcome::Rejected, Some("ChainRejected"))
                    }
                    Ok(_) => (crate::ObservationOutcome::Succeeded, None),
                    Err(LookupFailure::Protocol(_)) => {
                        (crate::ObservationOutcome::Failed, Some("Protocol"))
                    }
                    Err(LookupFailure::Transport(_)) => {
                        (crate::ObservationOutcome::Failed, Some("Transport"))
                    }
                };
                stage.finish_http(outcome, error, status, u32::try_from(endpoint_index).ok());
                match observed {
                    Ok(TransactionStatusObservation::Pending) => saw_pending = true,
                    Ok(committed) => return Ok(committed),
                    Err(error) => last_failure = Some(error),
                }
            }

            if saw_pending {
                Ok(TransactionStatusObservation::Pending)
            } else {
                Err(last_failure.unwrap_or_else(|| {
                    LookupFailure::Protocol(invalid_protocol(
                        "vote-chain lookup produced no endpoint result",
                    ))
                }))
            }
        }
        .await;
        let outcome = match &result {
            Ok(TransactionStatusObservation::Pending) => crate::ObservationOutcome::Pending,
            Ok(TransactionStatusObservation::CommittedFailure(_)) => {
                crate::ObservationOutcome::Rejected
            }
            Ok(_) => crate::ObservationOutcome::Succeeded,
            Err(_) => crate::ObservationOutcome::Failed,
        };
        operation.finish(outcome, result.as_ref().err().map(|_| "LookupFailure"));
        result
    }
}

/// One mutation request as [`ChainProtocolClient::post`] sends it: where it goes,
/// what it carries, and what its answer must echo.
struct ChainPost<'request> {
    /// Route name under the versioned vote-chain API prefix.
    endpoint: &'request str,
    /// Serialized JSON request body.
    body: Vec<u8>,
    /// Response contract the answer follows, with the digest an atomic-batch
    /// answer must echo before it counts as evidence.
    shape: ResponseShape,
    /// Circuit key that made the delegation proof in `body`, sent as a
    /// request header. `None` for vote routes and for a proof whose key was
    /// not recorded.
    delegation_key_fingerprint: Option<DelegationKeyFingerprint>,
}

fn chain_request(url: String, has_json_body: bool, timeout: Duration) -> ChainHttpRequest {
    let mut headers = vec![("accept".to_string(), "application/json".to_string())];
    if has_json_body {
        headers.push(("content-type".to_string(), "application/json".to_string()));
    }
    ChainHttpRequest::new(url, headers, timeout, MAX_CHAIN_HTTP_RESPONSE_BYTES)
}

fn parse_post_response(
    response: ChainHttpResponse,
    endpoint: &str,
    shape: ResponseShape,
) -> PostAttemptOutcome {
    if let Some(diagnostic) = gateway_shaped_route_answer(&response, endpoint) {
        return PostAttemptOutcome::PossiblyDispatched(diagnostic);
    }
    if let Some(diagnostic) = replaced_route_answer(&response, endpoint) {
        return PostAttemptOutcome::PossiblyDispatched(diagnostic);
    }
    if let Err(diagnostic) = validate_json_response(&response) {
        return PostAttemptOutcome::PossiblyDispatched(diagnostic);
    }
    let parsed: TransactionResultJson = match serde_json::from_slice(response.body()) {
        Ok(parsed) => parsed,
        Err(_) => {
            return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(match shape {
                ResponseShape::Legacy { .. } => "vote-chain transaction response is malformed JSON",
                ResponseShape::ProxyBatch { .. } => PROXY_BATCH_NOT_BROADCAST_RESULT,
            }))
        }
    };
    let expected_batch_digest = match shape {
        ResponseShape::Legacy {
            expected_batch_digest,
        } => expected_batch_digest,
        ResponseShape::ProxyBatch {
            expected_batch_digest,
        } => return parse_proxy_batch_response(response.status(), parsed, expected_batch_digest),
    };
    match (expected_batch_digest, &parsed.batch_digest) {
        (Some(expected), BatchDigestJson::Present(observed))
            if observed == &hex::encode(expected) => {}
        (Some(_), BatchDigestJson::Present(_)) => {
            return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
                "vote-batch response contained an invalid batch digest",
            ))
        }
        (Some(_), BatchDigestJson::Absent) => {
            return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
                "vote-batch response omitted its batch digest",
            ))
        }
        (None, BatchDigestJson::Present(_)) => {
            return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
                "non-batch vote-chain response contained a batch digest",
            ))
        }
        (None, BatchDigestJson::Absent) => {}
    }

    classify_broadcast_result(
        response.status(),
        parsed,
        "vote-chain",
        "vote-chain mutation",
        |code, codespace| {
            (
                cast_rejection_kind(codespace, code),
                format!("vote chain rejected transaction with code {code}"),
            )
        },
    )
}

/// Why a 0x09 answer that is not its `BroadcastResult`, with a required
/// `codespace` and `batch_digest`, stays possibly dispatched.
const PROXY_BATCH_NOT_BROADCAST_RESULT: &str =
    "proxy batch response is not a BroadcastResult with a codespace";

/// Interprets the 0x09 endpoint's answer (contracts-v1 §0, §8).
///
/// The answer must carry a `codespace`, and its digest must equal
/// `expected_batch_digest` for acceptance and rejection alike. A missing
/// field, a mismatched digest or any other shape is not evidence and stays
/// possibly dispatched.
fn parse_proxy_batch_response(
    status: u16,
    parsed: TransactionResultJson,
    expected_batch_digest: [u8; 32],
) -> PostAttemptOutcome {
    let (Some(codespace), BatchDigestJson::Present(batch_digest)) =
        (parsed.codespace.clone(), &parsed.batch_digest)
    else {
        return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
            PROXY_BATCH_NOT_BROADCAST_RESULT,
        ));
    };
    if batch_digest != &hex::encode(expected_batch_digest) {
        return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
            "proxy batch response omitted or changed its batch digest",
        ));
    }
    classify_broadcast_result(status, parsed, "proxy batch", "proxy batch", |code, _| {
        (
            proxy_batch_rejection_kind(&codespace, code),
            format!("vote chain rejected the proxy batch with {codespace} code {code}"),
        )
    })
}

/// Maps a `BroadcastResult` whose digest already checked out to an attempt
/// outcome. A 200 with code 0 and a canonical hash is accepted, and a 422
/// with a nonzero code is rejected. Anything else stays possibly
/// dispatched.
///
/// `answer` and `mutation` name the answer in diagnostics. `rejection`
/// gives a rejected code's kind and the start of its diagnostic, from the
/// code and the answer's codespace.
fn classify_broadcast_result(
    status: u16,
    parsed: TransactionResultJson,
    answer: &str,
    mutation: &str,
    rejection: impl FnOnce(u32, Option<&str>) -> (ChainRejectionKind, String),
) -> PostAttemptOutcome {
    match (status, parsed.code) {
        (200, 0) => match parsed.tx_hash.as_deref().and_then(normalize_candidate_hash) {
            Some(candidate) => PostAttemptOutcome::Accepted(candidate),
            None => PostAttemptOutcome::PossiblyDispatched(invalid_protocol(format!(
                "accepted {answer} response omitted a canonical transaction hash"
            ))),
        },
        (422, code) if code != 0 => {
            let candidate_transaction_hash = match parsed.tx_hash.as_deref() {
                None | Some("") => None,
                Some(hash) => match normalize_candidate_hash(hash) {
                    Some(hash) => Some(hash),
                    None => {
                        return PostAttemptOutcome::PossiblyDispatched(invalid_protocol(format!(
                            "rejected {answer} response contained an invalid transaction hash"
                        )))
                    }
                },
            };
            let (kind, message) = rejection(code, parsed.codespace.as_deref());
            PostAttemptOutcome::Rejected {
                code,
                kind,
                // The response log is the only thing that says *why* the chain
                // refused, and a bare numeric code leaves an operator with
                // nothing to act on. It is server-controlled text, so it is
                // carried as data and never as instruction:
                // `from_redacted_message` escapes it and bounds it to
                // `MAX_CHAIN_SUBMISSION_DIAGNOSTIC_BYTES` before it reaches the
                // durable row. The wallet's own submission is not a secret from
                // the wallet, and the row never leaves this device, so a log
                // that echoes the request lands here escaped and truncated
                // rather than dropped or picked over.
                diagnostic: ChainSubmissionDiagnostic::from_redacted_message(
                    ChainSubmissionDiagnosticKind::ChainRejected,
                    if parsed.log.trim().is_empty() {
                        message
                    } else {
                        format!("{message}: {}", parsed.log.trim())
                    },
                ),
                candidate_transaction_hash,
            }
        }
        (300..=399, _) => PostAttemptOutcome::PossiblyDispatched(invalid_protocol(
            "vote-chain mutation redirect was rejected",
        )),
        (status, code) => PostAttemptOutcome::PossiblyDispatched(invalid_protocol(format!(
            "unsupported {mutation} response status={status} code={code}"
        ))),
    }
}

/// Reads `{"height": "<decimal>"}` from the height route (contracts-v1 §8).
fn parse_chain_height_response(
    response: &ChainHttpResponse,
) -> Result<u64, ChainSubmissionDiagnostic> {
    #[derive(Deserialize)]
    struct ChainHeightJson {
        height: String,
    }
    parse_height_response(response, "height", |parsed: ChainHeightJson| parsed.height)
}

/// The height of a [`LATEST_VALIDATOR_SET_PATH`] answer.
#[derive(Deserialize)]
pub(super) struct ValidatorSetJson {
    pub(super) block_height: String,
}

/// Reads the decimal `block_height` from a validator-set answer.
fn parse_block_height_response(
    response: &ChainHttpResponse,
) -> Result<u64, ChainSubmissionDiagnostic> {
    parse_height_response(response, "block height", |parsed: ValidatorSetJson| {
        parsed.block_height
    })
}

/// Reads the decimal height string `height` picks out of a 200 JSON answer;
/// `what` names the height in diagnostics.
fn parse_height_response<T: serde::de::DeserializeOwned>(
    response: &ChainHttpResponse,
    what: &str,
    height: impl FnOnce(T) -> String,
) -> Result<u64, ChainSubmissionDiagnostic> {
    validate_json_response(response)?;
    if response.status() != 200 {
        return Err(invalid_protocol(format!(
            "vote-chain {what} lookup returned status {}",
            response.status()
        )));
    }
    let parsed: T = serde_json::from_slice(response.body())
        .map_err(|_| invalid_protocol(format!("vote-chain {what} answer is malformed")))?;
    height(parsed)
        .parse()
        .map_err(|_| invalid_protocol(format!("vote-chain {what} is not a decimal height")))
}

fn parse_status_response(
    response: ChainHttpResponse,
) -> Result<TransactionStatusObservation, ChainSubmissionDiagnostic> {
    validate_json_response(&response)?;
    if response.status() == 404 {
        let pending: GatewayErrorJson = serde_json::from_slice(response.body())
            .map_err(|_| invalid_protocol("vote-chain pending response is malformed JSON"))?;
        if pending.error != "tx not found" {
            return Err(invalid_protocol(
                "vote-chain pending response has an unsupported error",
            ));
        }
        return Ok(TransactionStatusObservation::Pending);
    }
    if (300..=399).contains(&response.status()) {
        return Err(invalid_protocol(
            "vote-chain transaction lookup redirect was rejected",
        ));
    }
    if !matches!(response.status(), 200 | 422) {
        return Err(invalid_protocol(format!(
            "unsupported vote-chain lookup HTTP status {}",
            response.status()
        )));
    }

    let parsed: TransactionStatusJson = serde_json::from_slice(response.body())
        .map_err(|_| invalid_protocol("vote-chain transaction status is malformed JSON"))?;
    let height = parsed.height.parse::<u64>().map_err(|_| {
        invalid_protocol("vote-chain transaction height is not a u64 decimal string")
    })?;
    let committed = CommittedTransaction {
        height,
        code: parsed.code,
        codespace: parsed.codespace,
        events: parsed.events,
    };
    match (response.status(), committed.code) {
        (200, 0) => Ok(TransactionStatusObservation::CommittedSuccess(committed)),
        (422, code) if code != 0 => Ok(TransactionStatusObservation::CommittedFailure(committed)),
        (status, code) => Err(invalid_protocol(format!(
            "contradictory vote-chain lookup response status={status} code={code}"
        ))),
    }
}

/// Recognizes a mutation answer shaped like the vote-chain gateway's error.
///
/// The gateway uses this shape for an unmounted route, but the shape does not
/// authenticate its writer: a forwarding proxy can reproduce it after the
/// upstream chain accepted the POST. The answer therefore identifies a likely
/// unsupported endpoint for diagnostics while preserving dispatch ambiguity.
///
/// Oversized bodies return `None` so the ordinary response-limit diagnostic
/// still owns them.
fn gateway_shaped_route_answer(
    response: &ChainHttpResponse,
    endpoint: &str,
) -> Option<ChainSubmissionDiagnostic> {
    if response.body().len() > MAX_CHAIN_HTTP_RESPONSE_BYTES {
        return None;
    }
    let status = response.status();
    if !matches!(status, 404 | 405) || !is_json_content_type(response) {
        return None;
    }
    serde_json::from_slice::<GatewayErrorJson>(response.body()).ok()?;
    Some(ChainSubmissionDiagnostic::from_redacted_message(
        ChainSubmissionDiagnosticKind::EndpointUnsupported,
        format!(
            "vote-chain endpoint may not serve /{}/{endpoint} \
             (HTTP {status}, gateway-shaped error envelope); the POST may have reached the \
             chain; check the gateway route and vote-sdk release",
            API_PREFIX.join("/")
        ),
    ))
}

/// Explains a possible route mismatch that cannot be attributed to the router.
///
/// An HTML body with status 200 is a front end's fallback page, and a 404 or
/// 405 without the gateway's error envelope came from something that is not the
/// gateway. Either can be a proxy replacing an answer after it already
/// forwarded the POST upstream, so these preserve dispatch ambiguity. Every
/// other non-JSON or unexpected-status answer reaches the ordinary ambiguous
/// diagnostics unchanged.
///
/// Oversized bodies return `None` so the ordinary response-limit diagnostic
/// still owns them.
fn replaced_route_answer(
    response: &ChainHttpResponse,
    endpoint: &str,
) -> Option<ChainSubmissionDiagnostic> {
    if response.body().len() > MAX_CHAIN_HTTP_RESPONSE_BYTES {
        return None;
    }
    let status = response.status();
    let possible_route_mismatch = match status {
        404 | 405 => true,
        200 => has_media_type(response, "text/html"),
        _ => false,
    };
    if !possible_route_mismatch {
        return None;
    }
    let observed = response.content_type().unwrap_or("(absent)");
    Some(ChainSubmissionDiagnostic::from_redacted_message(
        ChainSubmissionDiagnosticKind::RouteAnswerReplaced,
        format!(
            "vote-chain endpoint may not serve /{}/{endpoint} \
             (HTTP {status}, Content-Type {observed}); the POST may have reached the chain; \
             check the gateway route and vote-sdk release",
            API_PREFIX.join("/")
        ),
    ))
}

fn is_json_content_type(response: &ChainHttpResponse) -> bool {
    has_media_type(response, "application/json")
}

fn has_media_type(response: &ChainHttpResponse, media_type: &str) -> bool {
    response.content_type().is_some_and(|content_type| {
        content_type
            .split(';')
            .next()
            .is_some_and(|observed| observed.trim().eq_ignore_ascii_case(media_type))
    })
}

fn validate_json_response(response: &ChainHttpResponse) -> Result<(), ChainSubmissionDiagnostic> {
    if response.body().len() > MAX_CHAIN_HTTP_RESPONSE_BYTES {
        return Err(invalid_protocol(format!(
            "vote-chain response exceeds {MAX_CHAIN_HTTP_RESPONSE_BYTES} byte limit"
        )));
    }
    if !is_json_content_type(response) {
        // Report what actually arrived. The gateway sets `application/json` on
        // every response it writes, including its errors, so a different type
        // means the answer came from somewhere else — a proxy error page, a
        // router 404, a panic — and the body is the only thing that says
        // which. The text is server-controlled, so it rides along as data:
        // `invalid_protocol` escapes it and bounds it like any other
        // diagnostic.
        let observed = response.content_type().unwrap_or("(absent)");
        let body = String::from_utf8_lossy(response.body());
        let excerpt = body.trim();
        return Err(invalid_protocol(if excerpt.is_empty() {
            format!(
                "vote-chain response Content-Type must be application/json, got {observed} \
                 with an empty body"
            )
        } else {
            format!(
                "vote-chain response Content-Type must be application/json, got {observed}: \
                 {excerpt}"
            )
        }));
    }
    Ok(())
}

fn normalize_candidate_hash(value: &str) -> Option<CandidateTransactionHash> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    CandidateTransactionHash::from_str(&value.to_ascii_lowercase()).ok()
}

/// A deployed endpoint's `BroadcastResult`. Unknown fields are refused, so a
/// reshaped answer stays possibly dispatched.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionResultJson {
    #[serde(default)]
    tx_hash: Option<String>,
    code: u32,
    #[serde(default)]
    codespace: Option<String>,
    #[serde(default)]
    log: String,
    #[serde(default)]
    batch_digest: BatchDigestJson,
}

#[derive(Default)]
enum BatchDigestJson {
    #[default]
    Absent,
    Present(String),
}

impl<'de> Deserialize<'de> for BatchDigestJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Deserialize)]
struct TransactionStatusJson {
    height: String,
    code: u32,
    /// Present when the node honors the codespace header.
    #[serde(default)]
    codespace: Option<String>,
    #[serde(rename = "log")]
    _log: String,
    events: Vec<TxEvent>,
}

/// The gateway's error envelope: a lone `error` field and nothing else.
///
/// `deny_unknown_fields` identifies the expected shape but does not authenticate
/// which network component wrote it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayErrorJson {
    error: String,
}

fn canonicalize_chain_base_url(
    value: &str,
    network: Network,
) -> Result<String, ChainSubmissionDiagnostic> {
    let trimmed = value.trim();
    let mut url = Url::parse(trimmed)
        .map_err(|_| invalid_protocol("vote-chain endpoint is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid_protocol(
            "vote-chain endpoint must use HTTP or HTTPS",
        ));
    }
    if network == Network::Mainnet && url.scheme() != "https" {
        return Err(invalid_protocol(
            "production vote-chain endpoint must use HTTPS",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_protocol(
            "vote-chain endpoint must not contain credentials",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_protocol(
            "vote-chain endpoint must not contain a query or fragment",
        ));
    }
    if let Some(Host::Domain(domain)) = url.host() {
        if let Some(domain) = domain.strip_suffix('.') {
            let domain = domain.to_string();
            url.set_host(Some(&domain))
                .map_err(|_| invalid_protocol("vote-chain endpoint host is invalid"))?;
        }
    }
    let default_port = match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    if url.port() == default_port {
        url.set_port(None)
            .map_err(|_| invalid_protocol("vote-chain endpoint port is invalid"))?;
    }
    let path = normalize_percent_escapes(url.path())
        .map_err(|_| invalid_protocol("vote-chain endpoint path has an invalid percent escape"))?;
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn normalize_percent_escapes(path: &str) -> Result<String, ()> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut normalized = String::with_capacity(path.len());
    let mut remaining = path;
    while let Some(index) = remaining.find('%') {
        normalized.push_str(&remaining[..index]);
        let bytes = remaining.as_bytes();
        let high = bytes
            .get(index + 1)
            .copied()
            .and_then(hex_value)
            .ok_or(())?;
        let low = bytes
            .get(index + 2)
            .copied()
            .and_then(hex_value)
            .ok_or(())?;
        let value = (high << 4) | low;
        if value.is_ascii_alphanumeric() || matches!(value, b'-' | b'.' | b'_' | b'~') {
            normalized.push(char::from(value));
        } else {
            normalized.push('%');
            normalized.push(char::from(HEX[usize::from(high)]));
            normalized.push(char::from(HEX[usize::from(low)]));
        }
        remaining = &remaining[index + 3..];
    }
    normalized.push_str(remaining);
    Ok(normalized)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn join_chain_url(base_url: &str, segments: &[&str]) -> String {
    let mut url = base_url.to_string();
    for segment in API_PREFIX.iter().chain(segments) {
        url.push('/');
        url.push_str(segment);
    }
    url
}

fn invalid_protocol(message: impl AsRef<str>) -> ChainSubmissionDiagnostic {
    ChainSubmissionDiagnostic::from_redacted_message(
        ChainSubmissionDiagnosticKind::InvalidProtocolResponse,
        message,
    )
}

#[cfg(test)]
mod tests {
    mod batch_request;
    mod codespace;
    mod delegation_key_header;
    mod ingress_timeout;
    mod proxy_batch;
    mod rejection_diagnostics;

    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::chain_submission::{ChainTransportFuture, MAX_CHAIN_HTTP_RESPONSE_BYTES};

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[derive(Default)]
    struct ScriptedTransport {
        responses: Mutex<VecDeque<Result<ChainHttpResponse, ChainTransportError>>>,
        calls: Mutex<Vec<(bool, ChainHttpRequest, Vec<u8>)>>,
    }

    struct NeverTransport;

    impl ChainTransport for NeverTransport {
        fn chain_get<'a>(&'a self, _request: ChainHttpRequest) -> ChainTransportFuture<'a> {
            Box::pin(std::future::pending())
        }

        fn chain_post_json<'a>(
            &'a self,
            _request: ChainHttpRequest,
            _json: Vec<u8>,
        ) -> ChainTransportFuture<'a> {
            Box::pin(std::future::pending())
        }
    }

    impl ScriptedTransport {
        fn queue(&self, response: Result<ChainHttpResponse, ChainTransportError>) {
            self.responses.lock().unwrap().push_back(response);
        }
    }

    impl ChainTransport for ScriptedTransport {
        fn chain_get<'a>(&'a self, request: ChainHttpRequest) -> ChainTransportFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((false, request, Vec::new()));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted GET response")
            })
        }

        fn chain_post_json<'a>(
            &'a self,
            request: ChainHttpRequest,
            json: Vec<u8>,
        ) -> ChainTransportFuture<'a> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((true, request, json));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("scripted POST response")
            })
        }
    }

    fn protocol_client<T: ChainTransport>(
        transport: T,
        network: Network,
        endpoints: &[&str],
    ) -> ChainProtocolClient<T> {
        ChainProtocolClient::new(
            transport,
            network,
            &endpoints
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn delegation() -> DelegationSubmissionWire {
        DelegationSubmissionWire {
            rk: "rk".to_string(),
            spend_auth_sig: "signature".to_string(),
            tx1_effects: "effects".to_string(),
            nf_signed: "nullifier".to_string(),
            cmx_new: "cmx".to_string(),
            gov_comm: "van".to_string(),
            gov_nullifiers: vec!["governance-nullifier".to_string()],
            proof: "proof".to_string(),
            vote_round_id: "round".to_string(),
        }
    }

    fn vote() -> VoteCommitmentWire {
        VoteCommitmentWire {
            van_nullifier: "nullifier".to_string(),
            vote_authority_note_new: "successor".to_string(),
            vote_commitment: "commitment".to_string(),
            proposal_id: 3,
            proof: "proof".to_string(),
            vote_round_id: "round".to_string(),
            anchor_height: 42,
            r_vpk: "verification-key".to_string(),
            vote_auth_sig: "signature".to_string(),
        }
    }

    fn json(status: u16, body: impl AsRef<[u8]>) -> ChainHttpResponse {
        ChainHttpResponse::json(status, body.as_ref().to_vec())
    }

    #[tokio::test]
    async fn constructs_exact_mounted_url_headers_timeout_and_json() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(
            200,
            format!(r#"{{"tx_hash":"{}","code":0}}"#, HASH.to_ascii_uppercase()),
        )));
        let client = protocol_client(
            transport.clone(),
            Network::Mainnet,
            &[" HTTPS://Vote.Example:443/mount/// "],
        );
        let wire = delegation();

        assert_eq!(
            client.submit_delegation(0, &wire).await,
            PostAttemptOutcome::Accepted(CandidateTransactionHash::from_str(HASH).unwrap())
        );
        let calls = transport.calls.lock().unwrap();
        let (is_post, request, body) = &calls[0];
        assert!(*is_post);
        assert_eq!(
            request.url(),
            "https://vote.example/mount/shielded-vote/v1/delegate-vote"
        );
        assert_eq!(
            &request.headers()[..2],
            &[
                ("accept".to_string(), "application/json".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]
        );
        assert_eq!(request.headers().len(), 4);
        assert_eq!(request.headers()[2].0, "x-vote-ingress-attempt-v1");
        assert_eq!(hex::decode(&request.headers()[2].1).unwrap().len(), 32);
        assert_eq!(
            request.headers()[3],
            ("x-svote-codespace".to_string(), "1".to_string())
        );
        assert_eq!(request.timeout(), Duration::from_secs(150));
        assert_eq!(request.max_response_bytes(), MAX_CHAIN_HTTP_RESPONSE_BYTES);
        assert_eq!(body, wire.to_json().unwrap().as_bytes());
    }

    #[tokio::test]
    async fn constructs_exact_singleton_vote_url_and_json() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(200, format!(r#"{{"tx_hash":"{HASH}","code":0}}"#))));
        let client = protocol_client(
            transport.clone(),
            Network::Testnet,
            &["https://vote.example"],
        );
        let wire = vote();

        assert!(matches!(
            client.submit_vote(0, &wire).await,
            PostAttemptOutcome::Accepted(_)
        ));
        let calls = transport.calls.lock().unwrap();
        assert_eq!(
            calls[0].1.url(),
            "https://vote.example/shielded-vote/v1/cast-vote"
        );
        assert_eq!(calls[0].2, wire.to_json().unwrap().as_bytes());
    }

    #[tokio::test]
    async fn lookup_uses_its_shorter_bounded_deadline() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(404, r#"{"error":"tx not found"}"#)));
        let client = protocol_client(
            transport.clone(),
            Network::Testnet,
            &["https://vote.example"],
        );

        assert_eq!(
            client
                .transaction_status(
                    CandidateTransactionHash::from_str(HASH).unwrap(),
                    &crate::ObservationScope::disabled()
                )
                .await
                .unwrap(),
            TransactionStatusObservation::Pending
        );
        assert_eq!(
            transport.calls.lock().unwrap()[0].1.timeout(),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn protocol_timing_rejects_unbounded_or_inadequate_deadlines() {
        assert!(
            ChainProtocolTiming::new(Duration::from_secs(120), Duration::from_nanos(1)).is_ok()
        );
        assert!(
            ChainProtocolTiming::new(Duration::from_secs(600), Duration::from_secs(600)).is_ok()
        );

        for (post_timeout, lookup_timeout) in [
            (Duration::from_secs(119), Duration::from_secs(10)),
            (Duration::from_secs(601), Duration::from_secs(10)),
            (Duration::from_secs(150), Duration::ZERO),
            (Duration::from_secs(150), Duration::from_secs(601)),
        ] {
            assert!(ChainProtocolTiming::new(post_timeout, lookup_timeout).is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sdk_deadline_bounds_a_post_transport_that_ignores_metadata() {
        let client = protocol_client(
            Arc::new(NeverTransport),
            Network::Testnet,
            &["https://vote.example"],
        );
        let task = tokio::spawn(async move { client.submit_delegation(0, &delegation()).await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(151)).await;

        assert!(matches!(
            task.await.unwrap(),
            PostAttemptOutcome::PossiblyDispatched(diagnostic)
                if diagnostic.kind() == ChainSubmissionDiagnosticKind::AmbiguousDispatch
                    && diagnostic.message() == "vote-chain submission timed out"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn sdk_deadline_bounds_a_lookup_transport_that_ignores_metadata() {
        let client = protocol_client(
            Arc::new(NeverTransport),
            Network::Testnet,
            &["https://vote.example"],
        );
        let task = tokio::spawn(async move {
            client
                .transaction_status(
                    CandidateTransactionHash::from_str(HASH).unwrap(),
                    &crate::ObservationScope::disabled(),
                )
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        assert!(matches!(
            task.await.unwrap(),
            Err(LookupFailure::Transport(error))
                if error.kind() == ChainTransportFailureKind::PossiblyDispatched
                    && error.message() == "vote-chain transaction lookup timed out"
        ));
    }

    #[test]
    fn endpoint_validation_enforces_identity_and_production_https() {
        for endpoint in [
            "http://vote.example",
            "https://user@vote.example",
            "https://vote.example?x=1",
            "https://vote.example#fragment",
            "file:///tmp/vote",
        ] {
            assert!(
                ChainProtocolClient::new(
                    Arc::new(ScriptedTransport::default()),
                    Network::Mainnet,
                    &[endpoint.to_string()],
                )
                .is_err(),
                "{endpoint}"
            );
        }
        assert!(ChainProtocolClient::new(
            Arc::new(ScriptedTransport::default()),
            Network::Testnet,
            &[
                "HTTPS://vote.example:443/mount/".to_string(),
                "https://vote.example/mount".to_string(),
            ],
        )
        .is_err());
        assert!(ChainProtocolClient::new(
            Arc::new(ScriptedTransport::default()),
            Network::Regtest,
            &["http://127.0.0.1:8080".to_string()],
        )
        .is_ok());
        let maximum_endpoint_set = (0..MAX_CHAIN_ENDPOINTS)
            .map(|index| format!("https://vote-{index}.example"))
            .collect::<Vec<_>>();
        assert!(ChainProtocolClient::new(
            Arc::new(ScriptedTransport::default()),
            Network::Testnet,
            &maximum_endpoint_set,
        )
        .is_ok());
        assert!(ChainProtocolClient::new(
            Arc::new(ScriptedTransport::default()),
            Network::Testnet,
            &(0..=MAX_CHAIN_ENDPOINTS)
                .map(|index| format!("https://vote-{index}.example"))
                .collect::<Vec<_>>(),
        )
        .is_err());
    }

    #[tokio::test]
    async fn every_uncertain_post_result_stops_after_one_request() {
        let cases = [
            Ok(json(200, r#"{"code":0}"#)),
            Ok(json(422, r#"{"code":0}"#)),
            Ok(json(302, r#"{"location":"elsewhere"}"#)),
            Ok(json(200, b"not-json")),
            Ok(ChainHttpResponse::json(
                200,
                vec![b'x'; MAX_CHAIN_HTTP_RESPONSE_BYTES + 1],
            )),
            Ok(ChainHttpResponse::new(
                200,
                br#"{"tx_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","code":0}"#.to_vec(),
                Some("text/plain".to_string()),
                Vec::new(),
            )),
            Err(ChainTransportError::possibly_dispatched("timeout")),
        ];

        for response in cases {
            let transport = Arc::new(ScriptedTransport::default());
            transport.queue(response);
            let client = protocol_client(
                transport.clone(),
                Network::Testnet,
                &["https://one.example", "https://two.example"],
            );
            assert!(matches!(
                client.submit_delegation(0, &delegation()).await,
                PostAttemptOutcome::PossiblyDispatched(_)
            ));
            assert_eq!(transport.calls.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn oversized_request_is_rejected_before_transport() {
        let transport = Arc::new(ScriptedTransport::default());
        let client = protocol_client(
            transport.clone(),
            Network::Testnet,
            &["https://vote.example"],
        );
        let mut wire = delegation();
        wire.proof = "x".repeat(MAX_CHAIN_HTTP_REQUEST_BYTES + 1);

        assert!(matches!(
            client.submit_delegation(0, &wire).await,
            PostAttemptOutcome::LocalFailure(_)
        ));
        assert!(transport.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_transport_can_report_definitely_unsent() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Err(ChainTransportError::definitely_unsent(
            "privacy route unavailable",
        )));
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);

        assert!(matches!(
            client.submit_delegation(0, &delegation()).await,
            PostAttemptOutcome::DefinitelyUnsent(error) if error.is_definitely_unsent()
        ));
    }

    #[tokio::test]
    async fn lookup_prefers_later_committed_evidence_over_pending() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(404, r#"{"error":"tx not found"}"#)));
        transport.queue(Ok(json(
            200,
            r#"{"height":"42","code":0,"log":"","events":[{"type":"delegate_vote","attributes":[{"key":"leaf_index","value":"7","index":true}]}]}"#,
        )));
        let client = protocol_client(
            transport.clone(),
            Network::Testnet,
            &["https://one.example", "https://two.example"],
        );
        let hash = CandidateTransactionHash::from_str(HASH).unwrap();

        let result = client
            .transaction_status(hash, &crate::ObservationScope::disabled())
            .await
            .unwrap();
        let TransactionStatusObservation::CommittedSuccess(committed) = result else {
            panic!("expected committed success");
        };
        assert_eq!(committed.height, 42);
        assert_eq!(committed.events[0].event_type, "delegate_vote");
        assert_eq!(committed.events[0].attributes[0].value, "7");
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].1.url(),
            format!("https://one.example/shielded-vote/v1/tx/{HASH}")
        );
    }

    #[tokio::test]
    async fn lookup_accepts_committed_failure_and_rejects_contradictions() {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(
            422,
            r#"{"height":"43","code":9,"log":"execution failed","events":[]}"#,
        )));
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
        let hash = CandidateTransactionHash::from_str(HASH).unwrap();
        assert!(matches!(
            client
                .transaction_status(hash, &crate::ObservationScope::disabled())
                .await
                .unwrap(),
            TransactionStatusObservation::CommittedFailure(CommittedTransaction {
                height: 43,
                code: 9,
                ..
            })
        ));

        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(
            200,
            r#"{"height":"44","code":8,"log":"contradiction","events":[]}"#,
        )));
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
        assert!(matches!(
            client
                .transaction_status(hash, &crate::ObservationScope::disabled())
                .await,
            Err(LookupFailure::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn malformed_or_oversized_lookup_response_is_not_evidence() {
        let oversized = vec![b'x'; MAX_CHAIN_HTTP_RESPONSE_BYTES + 1];
        for response in [
            json(200, r#"{"height":42,"code":0,"log":"","events":[]}"#),
            json(200, r#"{"height":"42","code":0,"log":""}"#),
            json(404, "null"),
            json(404, r#"{"error":"not found"}"#),
            json(404, r#"{"error":"tx not found","extra":true}"#),
            ChainHttpResponse::json(200, oversized.clone()),
            ChainHttpResponse::new(200, b"not json".to_vec(), None, Vec::new()),
            json(307, r#"{"redirect":true}"#),
        ] {
            let transport = Arc::new(ScriptedTransport::default());
            transport.queue(Ok(response));
            let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
            assert!(matches!(
                client
                    .transaction_status(
                        CandidateTransactionHash::from_str(HASH).unwrap(),
                        &crate::ObservationScope::disabled()
                    )
                    .await,
                Err(LookupFailure::Protocol(_))
            ));
        }
    }
}

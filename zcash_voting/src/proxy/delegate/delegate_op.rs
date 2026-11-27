//! The 0x0B `MsgDelegateOp` body for `POST /shielded-vote/v1/delegate-op`
//! (contract §2.2, §8), and the node's answer.
//!
//! Bodies follow §0 as Go's encoder writes them and are a pure function of
//! the op's fields and signature, so a rebuilt op has the same bytes.

use std::fmt;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zcash_vote_delegate::{AttestationWindow, DelegateKey, DelegateRegisterFields};

use crate::chain_submission::{ChainSubmissionDiagnostic, ChainSubmissionDiagnosticKind};

/// The delegate op endpoint, under a vote server's base URL.
pub const DELEGATE_OP_PATH: &str = "/shielded-vote/v1/delegate-op";

/// Which delegate op a [`SignedDelegateOp`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DelegateOpKind {
    /// Registers a new entry.
    Register,
    /// Retires an entry.
    Retire,
    /// One round's complete ballot.
    Ballot,
}

/// One signed delegate op, for the host to `POST` to [`DELEGATE_OP_PATH`]
/// as `application/json`. Resending the same body is always safe. Once the
/// op landed, a resend gets code 61, 55 or 57, or code 0 with the original
/// hash from a node's transaction cache (contracts-v1 §8), which the
/// classifiers read as an acceptance.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedDelegateOp {
    kind: DelegateOpKind,
    delegate_index: u32,
    dk: DelegateKey,
    body: Vec<u8>,
}

impl SignedDelegateOp {
    /// Which op this is.
    pub fn kind(&self) -> DelegateOpKind {
        self.kind
    }

    /// The entry the op names, or 0 for a register op, which creates one.
    pub fn delegate_index(&self) -> u32 {
        self.delegate_index
    }

    /// The key that signed the op.
    pub fn dk(&self) -> DelegateKey {
        self.dk
    }

    /// The JSON body to post.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl fmt::Debug for SignedDelegateOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedDelegateOp")
            .field("kind", &self.kind)
            .field("delegate_index", &self.delegate_index)
            .field("dk", &self.dk)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// One verifier attestation as a register op carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OpAttestation {
    pub(super) verifier_id: u32,
    pub(super) window: AttestationWindow,
    /// The verifier's Ed25519 signature over ATTEST.
    pub(super) sig: [u8; 64],
}

/// The register op for `fields`, with `dk_sig` over REGISTER.
pub(super) fn register_op(
    fields: &DelegateRegisterFields<'_>,
    dk: &DelegateKey,
    attestations: &[OpAttestation],
    dk_sig: &[u8; 64],
) -> SignedDelegateOp {
    debug_assert_eq!(fields.dk, dk.to_bytes());
    let register = RegisterJson {
        dk: BASE64.encode(fields.dk),
        provider: fields.provider.code(),
        handle: fields.handle,
        proof_id: fields.proof_id,
        attestations: attestations
            .iter()
            .map(|attestation| AttestationJson {
                verifier_id: attestation.verifier_id,
                not_before: attestation.window.not_before,
                expires_at: attestation.window.expires_at,
                sig: BASE64.encode(attestation.sig),
            })
            .collect(),
        dk_sig: BASE64.encode(dk_sig),
    };
    signed_op(
        DelegateOpKind::Register,
        0,
        dk,
        &DelegateOpJson {
            register: Some(register),
            retire: None,
            ballot: None,
        },
    )
}

/// The register op of `dk` stored as `body` by
/// [`prepare_register_op`](super::prepare_register_op). The body is returned
/// as stored, never rebuilt.
pub(super) fn stored_register_op(
    dk: &DelegateKey,
    body: Vec<u8>,
) -> Result<SignedDelegateOp, &'static str> {
    #[derive(Deserialize)]
    struct StoredOp {
        register: StoredRegister,
    }
    #[derive(Deserialize)]
    struct StoredRegister {
        dk: String,
    }
    let stored: StoredOp =
        serde_json::from_slice(&body).map_err(|_| "a stored register op does not parse")?;
    if BASE64.decode(&stored.register.dk).ok().as_deref() != Some(dk.to_bytes().as_slice()) {
        return Err("a stored register op registers another key");
    }
    Ok(SignedDelegateOp {
        kind: DelegateOpKind::Register,
        delegate_index: 0,
        dk: *dk,
        body,
    })
}

/// The retire op of entry `delegate_index`, with `dk_sig` over RETIRE by its
/// key `dk`.
pub(super) fn retire_op(
    delegate_index: u32,
    dk: &DelegateKey,
    dk_sig: &[u8; 64],
) -> SignedDelegateOp {
    let retire = RetireJson {
        delegate_index,
        dk_sig: BASE64.encode(dk_sig),
    };
    signed_op(
        DelegateOpKind::Retire,
        delegate_index,
        dk,
        &DelegateOpJson {
            register: None,
            retire: Some(retire),
            ballot: None,
        },
    )
}

/// The ballot op of entry `delegate_index`, with `dk_sig` over BALLOT.
pub(super) fn ballot_op(
    vote_round_id: &[u8; 32],
    delegate_index: u32,
    dk: &DelegateKey,
    options: &[u32],
    dk_sig: &[u8; 64],
) -> SignedDelegateOp {
    let ballot = BallotJson {
        vote_round_id: BASE64.encode(vote_round_id),
        delegate_index,
        options: options.to_vec(),
        dk_sig: BASE64.encode(dk_sig),
    };
    signed_op(
        DelegateOpKind::Ballot,
        delegate_index,
        dk,
        &DelegateOpJson {
            register: None,
            retire: None,
            ballot: Some(ballot),
        },
    )
}

/// Serializes `op` into a [`SignedDelegateOp`].
fn signed_op(
    kind: DelegateOpKind,
    delegate_index: u32,
    dk: &DelegateKey,
    op: &DelegateOpJson<'_>,
) -> SignedDelegateOp {
    SignedDelegateOp {
        kind,
        delegate_index,
        dk: *dk,
        body: serde_json::to_vec(op).expect("a delegate op always serializes"),
    }
}

/// `MsgDelegateOp`: exactly one field is set.
#[derive(Serialize)]
struct DelegateOpJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    register: Option<RegisterJson<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retire: Option<RetireJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ballot: Option<BallotJson>,
}

/// `DelegateRegister`, fields in proto order.
#[derive(Serialize)]
struct RegisterJson<'a> {
    dk: String,
    provider: u32,
    handle: &'a str,
    proof_id: &'a str,
    attestations: Vec<AttestationJson>,
    dk_sig: String,
}

/// `DelegateAttestation`, fields in proto order.
#[derive(Serialize)]
struct AttestationJson {
    #[serde(skip_serializing_if = "is_zero_u32")]
    verifier_id: u32,
    #[serde(skip_serializing_if = "is_zero_u64")]
    not_before: u64,
    expires_at: u64,
    sig: String,
}

/// `DelegateRetire`, fields in proto order.
#[derive(Serialize)]
struct RetireJson {
    delegate_index: u32,
    dk_sig: String,
}

/// `DelegateBallot`, fields in proto order.
#[derive(Serialize)]
struct BallotJson {
    vote_round_id: String,
    delegate_index: u32,
    options: Vec<u32>,
    dk_sig: String,
}

/// Whether a `u32` is zero, which Go's encoder omits.
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// Whether a `u64` is zero, which Go's encoder omits.
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// CheckTx's verdict on a posted op (HTTP 200 or 422).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DelegateOpAnswer {
    /// CheckTx accepted the op (code 0). It is in the mempool, not a block.
    Accepted {
        /// The transaction hash, uppercase hex.
        tx_hash: String,
    },
    /// CheckTx rejected the op. Rules key on (`codespace`, `code`).
    Rejected {
        /// ABCI codespace. Anything but `vote` is a terminal rejection.
        codespace: String,
        code: u32,
        /// The node's log line, escaped and bounded. Diagnostic only.
        log: String,
    },
}

/// A post that got no CheckTx verdict.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DelegateOpAnswerError {
    /// The node refused the body before CheckTx (HTTP 400). Resending it
    /// cannot help.
    #[error("the node refused the delegate op with HTTP {status}: {message}")]
    RequestRefused {
        status: u16,
        /// The node's error message, escaped and bounded.
        message: String,
    },
    /// Any other answer. The op may have reached a mempool, so the host
    /// resends the same body later.
    #[error("no verdict on the delegate op (HTTP {status}): {detail}")]
    NoVerdict {
        status: u16,
        /// What was wrong, escaped and bounded.
        detail: String,
    },
}

/// Reads the node's answer to a delegate op post (contracts-v1 §0). HTTP 200
/// must carry code 0 and a transaction hash, and HTTP 422 a nonzero code.
///
/// # Errors
///
/// [`DelegateOpAnswerError::RequestRefused`] for HTTP 400, and
/// [`DelegateOpAnswerError::NoVerdict`] for any other answer.
pub fn parse_delegate_op_answer(
    status: u16,
    body: &[u8],
) -> Result<DelegateOpAnswer, DelegateOpAnswerError> {
    let no_verdict = |detail: &str| DelegateOpAnswerError::NoVerdict {
        status,
        detail: bounded_server_text(detail),
    };
    if status == 400 {
        let message = serde_json::from_slice::<ErrorJson>(body)
            .map(|error| error.error)
            .unwrap_or_else(|_| String::from_utf8_lossy(body).into_owned());
        return Err(DelegateOpAnswerError::RequestRefused {
            status,
            message: bounded_server_text(&message),
        });
    }
    if status != 200 && status != 422 {
        return Err(no_verdict(&String::from_utf8_lossy(body)));
    }
    let result: BroadcastResultJson = serde_json::from_slice(body)
        .map_err(|_| no_verdict("the answer is not a BroadcastResult with a codespace"))?;
    match (status, result.code) {
        (200, 0) => {
            let tx_hash = result.tx_hash;
            if tx_hash.len() != 64 || !tx_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(no_verdict("an accepted answer has no transaction hash"));
            }
            Ok(DelegateOpAnswer::Accepted {
                tx_hash: tx_hash.to_ascii_uppercase(),
            })
        }
        (422, code) if code != 0 => Ok(DelegateOpAnswer::Rejected {
            codespace: result.codespace,
            code,
            log: bounded_server_text(&result.log),
        }),
        (status, code) => Err(no_verdict(&format!(
            "HTTP {status} does not go with code {code}"
        ))),
    }
}

/// The endpoint's `BroadcastResult`.
#[derive(Deserialize)]
struct BroadcastResultJson {
    #[serde(default)]
    tx_hash: String,
    code: u32,
    codespace: String,
    #[serde(default)]
    log: String,
}

/// A REST error body, `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorJson {
    error: String,
}

/// Server text, escaped and bounded as chain diagnostics are.
pub(super) fn bounded_server_text(text: &str) -> String {
    ChainSubmissionDiagnostic::from_redacted_message(
        ChainSubmissionDiagnosticKind::ChainRejected,
        text.trim(),
    )
    .message()
    .to_owned()
}

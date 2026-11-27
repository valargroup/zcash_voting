//! Every chain answer to a delegate op, keyed on (codespace, code): code 61
//! for register, 55 for retire and 57 for a ballot are the only rejections
//! that can mean success, and only in codespace `vote` (contract §6, §8).

use zcash_vote_delegate::{ProxyChainError, VOTE_CODESPACE};

use super::fixtures::broadcast_result;
use crate::{
    proxy::delegate::{
        classify_ballot_answer, classify_register_answer, classify_retire_answer,
        parse_delegate_op_answer, BallotAnswer, DelegateOpAnswer, DelegateOpAnswerError,
        RegisterAnswer, RetireAnswer,
    },
    wire::{BallotSubmitResult, DelegateRegisterResult, DelegateRetireResult},
    MAX_CHAIN_SUBMISSION_DIAGNOSTIC_BYTES,
};

/// A rejection with `code` in `codespace`.
fn rejected(codespace: &str, code: u32) -> DelegateOpAnswer {
    DelegateOpAnswer::Rejected {
        codespace: codespace.to_owned(),
        code,
        log: format!("rejected with {code}"),
    }
}

/// Every registered code in `vote`, and the same numbers in another
/// codespace, classify for each op.
#[test]
fn every_chain_code_classifies_for_each_op() {
    for error in ProxyChainError::ALL {
        let code = error.code();
        let answer = rejected(VOTE_CODESPACE, code);
        let register = classify_register_answer(&answer);
        let retire = classify_retire_answer(&answer);
        let ballot = classify_ballot_answer(&answer);
        let reason = format!("rejected with {code}");
        let register_rejection = RegisterAnswer::Rejected(DelegateRegisterResult::Rejected {
            codespace: VOTE_CODESPACE.to_owned(),
            code,
            reason: reason.clone(),
        });
        let retire_rejection = RetireAnswer::Settled(DelegateRetireResult::Rejected {
            codespace: VOTE_CODESPACE.to_owned(),
            code,
            reason: reason.clone(),
        });
        let ballot_rejection = BallotAnswer::Settled(BallotSubmitResult::Rejected {
            codespace: VOTE_CODESPACE.to_owned(),
            code,
            reason,
        });
        let success = (
            error == ProxyChainError::DelegateKeyReused,
            error == ProxyChainError::DelegateRetired,
            error == ProxyChainError::DelegateBallotExists,
        );
        if success.0 {
            assert_eq!(register, RegisterAnswer::AlreadyRegistered);
        } else {
            assert_eq!(register, register_rejection, "{error:?}");
        }
        if success.1 {
            assert_eq!(retire, RetireAnswer::Settled(DelegateRetireResult::Retired));
        } else {
            assert_eq!(retire, retire_rejection, "{error:?}");
        }
        if success.2 {
            assert_eq!(ballot, BallotAnswer::BallotExists);
        } else {
            assert_eq!(ballot, ballot_rejection, "{error:?}");
        }

        let other = rejected("sdk", code);
        assert!(matches!(
            classify_register_answer(&other),
            RegisterAnswer::Rejected(DelegateRegisterResult::Rejected { .. })
        ));
        assert!(matches!(
            classify_retire_answer(&other),
            RetireAnswer::Settled(DelegateRetireResult::Rejected { .. })
        ));
        assert!(matches!(
            classify_ballot_answer(&other),
            BallotAnswer::Settled(BallotSubmitResult::Rejected { .. })
        ));
    }
    let accepted = DelegateOpAnswer::Accepted {
        tx_hash: "AB".repeat(32),
    };
    assert_eq!(
        classify_ballot_answer(&accepted),
        BallotAnswer::Settled(BallotSubmitResult::Accepted)
    );
}

/// HTTP 200 carries code 0 and a hash, HTTP 422 a nonzero code with its
/// codespace; HTTP 400 is a refused body, and anything else no verdict.
#[test]
fn delegate_op_answers_are_read_by_status() {
    let mut accepted = broadcast_result(0, "", "");
    assert_eq!(
        parse_delegate_op_answer(200, &accepted),
        Ok(DelegateOpAnswer::Accepted {
            tx_hash: "AB".repeat(32)
        })
    );
    accepted = br#"{"tx_hash":"abababababababababababababababababababababababababababababababab","code":0,"codespace":"","new_field":true}"#.to_vec();
    assert_eq!(
        parse_delegate_op_answer(200, &accepted),
        Ok(DelegateOpAnswer::Accepted {
            tx_hash: "AB".repeat(32)
        }),
        "the hash is uppercase and unknown fields are ignored"
    );
    assert_eq!(
        parse_delegate_op_answer(422, &broadcast_result(57, "vote", "ballot exists")),
        Ok(DelegateOpAnswer::Rejected {
            codespace: "vote".to_owned(),
            code: 57,
            log: "ballot exists".to_owned(),
        })
    );
    assert_eq!(
        parse_delegate_op_answer(400, br#"{"error":"validation failed: dk_sig"}"#),
        Err(DelegateOpAnswerError::RequestRefused {
            status: 400,
            message: "validation failed: dk_sig".to_owned(),
        })
    );
    for (status, body) in [
        (200, br#"{"tx_hash":"","code":0,"codespace":""}"#.to_vec()),
        (200, broadcast_result(6, "vote", "bad signature")),
        (422, broadcast_result(0, "", "")),
        (
            422,
            br#"{"tx_hash":"","code":57,"log":"no codespace"}"#.to_vec(),
        ),
        (
            404,
            br#"{"error":"proxy delegation is not enabled"}"#.to_vec(),
        ),
        (502, b"broadcast failed".to_vec()),
        (503, br#"{"ready":false}"#.to_vec()),
    ] {
        assert!(
            matches!(
                parse_delegate_op_answer(status, &body),
                Err(DelegateOpAnswerError::NoVerdict { status: answered, .. }) if answered == status
            ),
            "HTTP {status}"
        );
    }
    let long_log = "x".repeat(4 * MAX_CHAIN_SUBMISSION_DIAGNOSTIC_BYTES);
    let Ok(DelegateOpAnswer::Rejected { log, .. }) =
        parse_delegate_op_answer(422, &broadcast_result(58, "vote", &long_log))
    else {
        panic!("a rejection");
    };
    assert!(log.len() <= MAX_CHAIN_SUBMISSION_DIAGNOSTIC_BYTES);
}

/// A byte-identical resend the node's mempool cache still holds is answered
/// with the earlier outcome: code 0 while pending or once landed, which
/// settles as success for every op, or the code its block gave it.
#[test]
fn resends_answered_from_the_mempool_cache_keep_the_earlier_outcome() {
    let pending = serde_json::json!({
        "tx_hash": "AB".repeat(32),
        "code": 0,
        "codespace": "",
        "log": "tx pending in mempool (duplicate submission)",
    })
    .to_string();
    let answer = parse_delegate_op_answer(200, pending.as_bytes()).unwrap();
    assert!(matches!(
        classify_register_answer(&answer),
        RegisterAnswer::Submitted { .. }
    ));
    assert!(matches!(
        classify_retire_answer(&answer),
        RetireAnswer::Submitted { .. }
    ));
    assert_eq!(
        classify_ballot_answer(&answer),
        BallotAnswer::Settled(BallotSubmitResult::Accepted)
    );
    let failed_in_block = parse_delegate_op_answer(
        422,
        &broadcast_result(62, "vote", "handle has a live delegate entry"),
    )
    .unwrap();
    assert_eq!(
        classify_register_answer(&failed_in_block),
        RegisterAnswer::Rejected(DelegateRegisterResult::Rejected {
            codespace: "vote".to_owned(),
            code: 62,
            reason: "handle has a live delegate entry".to_owned(),
        })
    );
}

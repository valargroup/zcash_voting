//! contracts-v1 §0: every submission and lookup asks for the answer's
//! codespace, every rejection is classified on (codespace, code), and a
//! deployed endpoint that omits the codespace is still read as before.

use super::*;

/// Whether `request` carries `X-Svote-Codespace: 1`.
fn asks_for_codespace(request: &ChainHttpRequest) -> bool {
    request
        .headers()
        .contains(&("x-svote-codespace".to_string(), "1".to_string()))
}

#[tokio::test]
async fn every_submission_and_lookup_asks_for_the_codespace() {
    let transport = Arc::new(ScriptedTransport::default());
    let accepted = || Ok(json(200, format!(r#"{{"tx_hash":"{HASH}","code":0}}"#)));
    let batch_digest = [9; 32];
    let accepted_batch = || {
        Ok(json(
            200,
            format!(
                r#"{{"tx_hash":"{HASH}","code":0,"batch_digest":"{}"}}"#,
                hex::encode(batch_digest)
            ),
        ))
    };
    transport.queue(accepted());
    transport.queue(accepted());
    transport.queue(accepted_batch());
    transport.queue(Ok(json(404, r#"{"error":"tx not found"}"#)));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );

    client.submit_delegation(0, &delegation()).await;
    client.submit_vote(0, &vote()).await;
    client
        .submit_vote_batch_with_dispatch(
            0,
            &crate::wire::VoteCommitmentBatchWire {
                votes: vec![vote(), vote()],
            },
            batch_digest,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;
    client
        .transaction_status(
            CandidateTransactionHash::from_str(HASH).unwrap(),
            &crate::ObservationScope::disabled(),
        )
        .await
        .unwrap();

    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for (_, request, _) in calls.iter() {
        assert!(asks_for_codespace(request), "{}", request.url());
    }
}

/// A deployed endpoint's answer that honors the header carries a codespace
/// beside its exact fields, and its rejection is classified on the pair:
/// only `vote` 2 is a spent nullifier, so the SDK's own code 2 is not. One
/// that omits it is keyed on the code alone, as before.
#[tokio::test]
async fn a_legacy_answer_classifies_on_its_codespace_when_it_has_one() {
    for (body, codespace, kind) in [
        (
            r#"{"code":2,"codespace":"vote","log":"spent"}"#,
            Some("vote"),
            ChainRejectionKind::NullifierAlreadySpent,
        ),
        (
            r#"{"code":2,"codespace":"sdk","log":"tx parse error"}"#,
            Some("sdk"),
            ChainRejectionKind::Other,
        ),
        (
            r#"{"code":7,"codespace":"vote","log":"round closed"}"#,
            Some("vote"),
            ChainRejectionKind::Other,
        ),
        (
            r#"{"code":2,"log":"spent"}"#,
            None,
            ChainRejectionKind::NullifierAlreadySpent,
        ),
    ] {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(422, body)));
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);

        let outcome = client.submit_vote(0, &vote()).await;

        let PostAttemptOutcome::Rejected {
            codespace: observed,
            kind: classified,
            ..
        } = outcome
        else {
            panic!("{body}: {outcome:?}");
        };
        assert_eq!(observed.as_deref(), codespace, "{body}");
        assert_eq!(classified, kind, "{body}");
    }

    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(json(
        200,
        format!(r#"{{"tx_hash":"{HASH}","code":0,"codespace":"","log":""}}"#),
    )));
    let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
    assert!(matches!(
        client.submit_delegation(0, &delegation()).await,
        PostAttemptOutcome::Accepted(_)
    ));
}

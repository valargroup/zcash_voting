use super::*;

#[test]
fn only_a_complete_matching_ingress_timeout_is_non_dispatch_evidence() {
    let token = "a".repeat(64);
    let valid = serde_json::json!({"error":{"version":1,"code":"request_body_timeout","dispatch":"not_started","attempt":token}});
    let accepts = |status, value: &serde_json::Value, expected: Option<&str>| {
        super::super::ingress_timeout::is_not_dispatched(
            &ChainHttpResponse::json(status, serde_json::to_vec(value).unwrap()),
            expected,
        )
    };
    assert!(accepts(408, &valid, Some(&token)));
    assert!(!accepts(400, &valid, Some(&token)));
    assert!(!accepts(408, &valid, None));
    assert!(!accepts(408, &valid, Some(&"b".repeat(64))));
    for field in ["version", "code", "dispatch", "attempt"] {
        let mut malformed = valid.clone();
        malformed["error"].as_object_mut().unwrap().remove(field);
        assert!(!accepts(408, &malformed, Some(&token)));
    }
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("dispatch", serde_json::json!("started")),
        ("code", serde_json::json!("gateway_timeout")),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut malformed = valid.clone();
        malformed["error"][field] = value;
        assert!(!accepts(408, &malformed, Some(&token)));
    }
    let duplicate = format!(
        r#"{{"error":{{"version":1,"version":1,"code":"request_body_timeout","dispatch":"not_started","attempt":"{token}"}}}}"#
    );
    assert!(!super::super::ingress_timeout::is_not_dispatched(
        &ChainHttpResponse::json(408, duplicate.into_bytes()),
        Some(&token)
    ));
}

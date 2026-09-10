use super::{observations::observe_helper_http, DirectRoute};
use crate::{ObservabilityOptions, ObservationOutcome, ObservationScope};
use std::{
    io::{Read, Write},
    time::Duration,
};

#[tokio::test]
async fn correlates_server_timing_without_changing_acceptance() {
    for matching in [true, false] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream.read_exact(&mut [0; 2]).unwrap();
            let headers = String::from_utf8(request).unwrap().to_ascii_lowercase();
            let id = headers
                .lines()
                .find_map(|line| line.strip_prefix("x-vote-request-id: "))
                .unwrap();
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
            std::thread::sleep(Duration::from_millis(20));
            let echoed = if matching {
                id
            } else {
                "00000000000000000000000000000000"
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 19\r\nConnection: close\r\nX-Vote-Request-ID: {echoed}\r\nX-Vote-Handler-Started-Us: 1000000\r\nX-Vote-Handler-Duration-Us: 10000\r\n\r\n{{\"status\":\"queued\"}}").unwrap();
        });
        let owner = ObservationScope::new(Some(ObservabilityOptions::default())).invocation();
        let transport = DirectRoute::new();
        let url = format!("http://{address}/shielded-vote/v1/shares");
        let response = observe_helper_http(
            owner.scope().clone(),
            super::diagnostics::observed_request(
                transport.client.as_ref(),
                http::Request::post(&url)
                    .body(http_body_util::Full::new(bytes::Bytes::from_static(b"{}")))
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), 200);
        server.join().unwrap();
        let report = owner
            .finish("test", None, ObservationOutcome::Succeeded)
            .unwrap();
        let diagnostics = report
            .records
            .iter()
            .find_map(|r| r.http_diagnostics.as_ref())
            .unwrap();
        assert_eq!(diagnostics.protocol.as_deref(), Some("http/1.1"));
        // The localhost fixture bypasses the request gate to test correlation,
        // but the connector correctly excludes this non-staging destination.
        assert_eq!(diagnostics.connection_predates_request, None);
        assert_eq!(diagnostics.connection_setup_us, None);
        assert_eq!(diagnostics.server_handler_us, matching.then_some(10000));
        assert!(diagnostics.response_headers_us.unwrap() >= 20000);
        assert_eq!(
            diagnostics.unattributed_wait_us,
            if matching {
                diagnostics.response_headers_us.map(|v| v - 10000)
            } else {
                None
            }
        );
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("127.0.0.1"));
        assert!(!json.contains("x-vote-request-id"));
        let mut legacy = serde_json::to_value(&report).unwrap();
        for record in legacy["records"].as_array_mut().unwrap() {
            record.as_object_mut().unwrap().remove("http_diagnostics");
        }
        assert!(serde_json::from_value::<crate::OperationObservability>(legacy).is_ok());
    }
}

#[test]
fn diagnostics_require_staging_opt_in_and_exact_destination() {
    for environment in ["", "production", "staging"] {
        for flag in ["", "0", "1"] {
            for url in [
                "https://stage.vote-chain-primary.valargroup.org/shielded-vote/v1/shares",
                "https://stage.vote-chain-secondary.valargroup.org/shielded-vote/v1/shares",
                "https://vote-chain-primary.valargroup.org/shielded-vote/v1/shares",
                "https://stage.vote-chain-primary.valargroup.org.example.com/shielded-vote/v1/shares",
                "http://stage.vote-chain-primary.valargroup.org/shielded-vote/v1/shares",
                "https://stage.vote-chain-primary.valargroup.org/status",
            ] {
                let expected = environment == "staging" && flag == "1" && matches!(url,
                    "https://stage.vote-chain-primary.valargroup.org/shielded-vote/v1/shares" |
                    "https://stage.vote-chain-secondary.valargroup.org/shielded-vote/v1/shares");
                assert_eq!(super::diagnostics::enabled_for(&url.parse().unwrap(), environment, flag), expected);
            }
        }
    }
}

//! Correlate an observed POST without changing delivery or timeout semantics.
use super::{
    observations, HyperClientError, HyperRequestClient, Incoming, Request, RequestBody, Response,
};
use crate::observability::HttpRequestDiagnostics;
use crate::ObservationOutcome;
use hyper_util::client::legacy::connect::capture_connection;
use rand::RngCore;
use std::time::Instant;

fn micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Capture connection assignment separately from the remaining header wait.
/// Assignment includes pool wait and any new connection setup; Hyper does not
/// expose DNS/TCP/TLS subdivision or an on-wire send timestamp at this boundary.
pub(super) async fn request(
    client: &dyn HyperRequestClient,
    request: Request<RequestBody>,
) -> Result<Response<Incoming>, HyperClientError> {
    if request.method() != http::Method::POST
        || !enabled_for(
            request.uri(),
            &std::env::var("SENTRY_ENVIRONMENT").unwrap_or_default(),
            &std::env::var("SVOTE_HELPER_DIAGNOSTICS").unwrap_or_default(),
        )
    {
        return client.request(request).await;
    }
    observed_request(client, request).await
}

// Exact destinations prevent a staging-configured process from instrumenting
// production or an arbitrary helper. No suffix matching or URL text logging.
pub(super) fn enabled_for(uri: &http::Uri, environment: &str, flag: &str) -> bool {
    environment == "staging"
        && flag == "1"
        && uri.scheme_str() == Some("https")
        && matches!(
            uri.host(),
            Some(
                "stage.vote-chain-primary.valargroup.org"
                    | "stage.vote-chain-secondary.valargroup.org"
            )
        )
        && uri.path() == "/shielded-vote/v1/shares"
}

pub(super) async fn observed_request(
    client: &dyn HyperRequestClient,
    mut request: Request<RequestBody>,
) -> Result<Response<Incoming>, HyperClientError> {
    let scope = observations::scope();
    if !scope.is_enabled() {
        return client.request(request).await;
    }
    let mut random = [0; 16];
    // Failure to obtain diagnostic randomness must not fail a vote request.
    if rand::rngs::OsRng.try_fill_bytes(&mut random).is_err() {
        return client.request(request).await;
    }
    let request_id = hex::encode(random);
    request.headers_mut().insert(
        "x-vote-request-id",
        request_id.parse().expect("hex is a valid header"),
    );
    let mut captured = capture_connection(&mut request);
    let timer = scope.stage("helper.http.transport");
    let mut diagnostics = HttpRequestDiagnostics {
        request_id,
        ..Default::default()
    };
    timer.http_diagnostics(diagnostics.clone());
    let started = Instant::now();
    let response_future = client.request(request);
    tokio::pin!(response_future);
    let response = tokio::select! {
        response = &mut response_future => response,
        has_connection = async { captured.wait_for_connection_metadata().await.is_some() } => {
            if has_connection {
                diagnostics.connection_acquired_us = Some(micros(started));
                timer.http_diagnostics(diagnostics.clone());
            }
            response_future.await
        }
    };
    {
        let metadata = captured.connection_metadata();
        if let Some(connection) = metadata.as_ref() {
            let mut extensions = http::Extensions::new();
            connection.get_extras(&mut extensions);
            if let Some(timing) =
                extensions.get::<super::connection_diagnostics::ConnectionTiming>()
            {
                diagnostics.connection_setup_us = Some(timing.setup_us);
                diagnostics.connection_predates_request = Some(timing.established < started);
            }
        }
    }
    let elapsed = micros(started);
    if let Ok(response) = &response {
        diagnostics.response_headers_us = Some(elapsed);
        diagnostics.protocol = Some(
            match response.version() {
                http::Version::HTTP_2 => "h2",
                http::Version::HTTP_11 => "http/1.1",
                http::Version::HTTP_10 => "http/1.0",
                _ => "other",
            }
            .to_owned(),
        );
        // Require the echoed token before trusting that timing belongs to this request.
        if response
            .headers()
            .get("x-vote-request-id")
            .and_then(|v| v.to_str().ok())
            == Some(&diagnostics.request_id)
        {
            let number = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
            };
            diagnostics.server_started_unix_us = number("x-vote-handler-started-us");
            diagnostics.server_handler_us = number("x-vote-handler-duration-us");
            diagnostics.unattributed_wait_us = diagnostics
                .server_handler_us
                .and_then(|server| elapsed.checked_sub(server));
        }
    }
    timer.http_diagnostics(diagnostics);
    timer.finish(
        if response.is_ok() {
            ObservationOutcome::Succeeded
        } else {
            ObservationOutcome::Failed
        },
        None,
    );
    response
}

/// Connection metadata is captured only for opted-in staging destinations.
/// The connector sees an origin URI, so request path/method filtering happens above.
pub(super) fn connection_enabled(uri: &http::Uri) -> bool {
    std::env::var("SENTRY_ENVIRONMENT").as_deref() == Ok("staging")
        && std::env::var("SVOTE_HELPER_DIAGNOSTICS").as_deref() == Ok("1")
        && uri.scheme_str() == Some("https")
        && matches!(
            uri.host(),
            Some(
                "stage.vote-chain-primary.valargroup.org"
                    | "stage.vote-chain-secondary.valargroup.org"
            )
        )
}

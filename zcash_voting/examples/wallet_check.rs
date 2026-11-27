//! `wallet-check`: reads a dev stack round the way a wallet does, through
//! zcash_voting's config and registry client, and prints what it verified as
//! JSON for the dev stack's end-to-end test.
//!
//! ```text
//! cargo run -p zcash_voting --example wallet_check -- \
//!     --static-source <url?checksum=sha256:…> --ca <pem> --round <hex>
//! ```
//!
//! Over HTTPS, trusting only the CA in `--ca`, it:
//!
//! 1. fetches the static config and authenticates it against its pin
//!    (`resolve_static_voting_config`);
//! 2. fetches the dynamic config from its first mirror, resolves it
//!    (`resolve_dynamic_voting_config`) and reports whether the round's
//!    ordinary entry is authenticated;
//! 3. reads the unsigned URL lists of `extensions.proxy_delegation_v1`
//!    (`proxy_service_urls`).
//!
//! Any failure exits 1 with the reason on stderr.

use std::{process::ExitCode, sync::Arc, time::Duration};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde_json::{json, Value};
use zcash_voting::{
    config::{
        resolve_dynamic_voting_config, resolve_static_voting_config, ResolveVotingConfigOptions,
    },
    proxy::registry::proxy_service_urls,
};

/// An HTTP(S) client trusting only the `--ca` roots.
type HttpClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

/// How long one GET may take, from connecting to the last body byte.
const GET_TIMEOUT: Duration = Duration::from_secs(30);

/// The command line.
const USAGE: &str = "usage: wallet-check --static-source <url> --ca <pem> --round <hex>";

/// The arguments.
struct Args {
    static_source: String,
    ca: String,
    round: String,
}

/// Runs the check and prints its report.
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&args) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match check(&args).await {
        Ok(report) => {
            println!("{}", serde_json::to_string_pretty(&report).expect("JSON"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("wallet-check: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses `--name value` pairs; every one is required.
fn parse(args: &[String]) -> Result<Args, String> {
    let mut values = [None, None, None];
    let names = ["--static-source", "--ca", "--round"];
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let position = names
            .iter()
            .position(|name| name == flag)
            .ok_or_else(|| format!("unknown argument {flag}"))?;
        values[position] = Some(
            rest.next()
                .ok_or_else(|| format!("{flag} needs a value"))?
                .clone(),
        );
    }
    let [Some(static_source), Some(ca), Some(round)] = values else {
        return Err("every argument is required".into());
    };
    Ok(Args {
        static_source,
        ca,
        round,
    })
}

/// The whole check; see the module docs.
async fn check(args: &Args) -> Result<Value, String> {
    let pem = std::fs::read(&args.ca).map_err(|error| format!("{}: {error}", args.ca))?;
    let http = http_client(&pem).map_err(|error| format!("{}: {error}", args.ca))?;

    // 1 and 2: the static config's pin, the dynamic config's round entries.
    let static_url = args
        .static_source
        .split_once('?')
        .map_or(args.static_source.as_str(), |(url, _)| url);
    let static_bytes = get(&http, static_url).await?;
    let resolved_static = resolve_static_voting_config(&args.static_source, &static_bytes)
        .map_err(|error| format!("static config: {error}"))?;
    let config_url = resolved_static.dynamic_config_url.clone();
    let config_bytes = get(&http, &config_url).await?;
    let resolved = resolve_dynamic_voting_config(
        resolved_static.clone(),
        &config_bytes,
        ResolveVotingConfigOptions::default(),
    )
    .map_err(|error| format!("dynamic config: {error}"))?;
    let round_authenticated = resolved
        .authenticated_rounds
        .iter()
        .any(|round| round.round_id == args.round);

    // 3: the URL lists.
    let urls = proxy_service_urls(&config_bytes)
        .map_err(|error| format!("URL lists: {error}"))?
        .ok_or("the dynamic config has no extensions.proxy_delegation_v1")?;

    Ok(json!({
        "static": {
            "dynamic_config_urls": resolved_static.dynamic_config_urls,
            "hash_pinned": resolved_static.source.sha256.is_some(),
        },
        "round_authenticated": round_authenticated,
        "urls": {
            "verifier_api_urls": urls.verifier_api_urls,
            "proof_rpc_urls": urls.proof_rpc_urls,
        },
    }))
}

/// A client that trusts only the certificates in `pem`, over HTTP/1.1.
fn http_client(pem: &[u8]) -> Result<HttpClient, String> {
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(pem) {
        roots
            .add(certificate.map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    }
    if roots.is_empty() {
        return Err("no certificate".into());
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| error.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .wrap_connector(connector);
    Ok(Client::builder(TokioExecutor::new()).build(https))
}

/// GETs `url`, which must answer with a success status within
/// [`GET_TIMEOUT`].
async fn get(http: &HttpClient, url: &str) -> Result<Vec<u8>, String> {
    let fail = |error: &dyn std::fmt::Display| format!("GET {url}: {error}");
    let uri: http::Uri = url.parse().map_err(|error| fail(&error))?;
    let fetch = async {
        let response = http.get(uri).await.map_err(|error| fail(&error))?;
        if !response.status().is_success() {
            return Err(fail(&response.status()));
        }
        Ok(response
            .into_body()
            .collect()
            .await
            .map_err(|error| fail(&error))?
            .to_bytes()
            .to_vec())
    };
    tokio::time::timeout(GET_TIMEOUT, fetch)
        .await
        .map_err(|error| fail(&error))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every argument is required, and an unknown one is refused.
    #[test]
    fn arguments_parse() {
        let args =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_owned).collect() };
        let parsed = parse(&args(
            "--static-source https://a/s.json --ca ca.pem --round ab",
        ))
        .unwrap();
        assert_eq!(
            (parsed.static_source.as_str(), parsed.round.as_str()),
            ("https://a/s.json", "ab")
        );
        assert!(parse(&args("--ca ca.pem --round ab")).is_err());
        assert!(parse(&args("--chain-id svote-1")).is_err());
    }
}

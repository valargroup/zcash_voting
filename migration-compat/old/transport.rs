//! A deliberately small host HTTP adapter; the released SDK owns wire encoding.
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};
use zcash_voting::confirmation::TxEvent;
static EVIDENCE_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static ATTEMPT: AtomicUsize = AtomicUsize::new(0);

pub fn record_responses(directory: PathBuf) -> Result<()> {
    std::fs::create_dir(&directory)?;
    EVIDENCE_DIRECTORY
        .set(directory)
        .map_err(|_| anyhow::anyhow!("response recording already initialized"))
}

/// One attempt only. An ambiguous POST aborts capture instead of being repeated.
pub fn request(url: &str, body: Option<&str>) -> Result<Value> {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--max-time",
        "120",
        "--write-out",
        "\n%{http_code}",
        url,
    ]);
    if body.is_some() {
        command.args([
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(body) = body {
        child
            .stdin
            .take()
            .context("request stdin")?
            .write_all(body.as_bytes())?;
    }
    let response = child.wait_with_output()?;
    let split = response
        .stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .context("missing HTTP status")?;
    let status: u16 = std::str::from_utf8(&response.stdout[split + 1..])?.parse()?;
    if let Some(directory) = EVIDENCE_DIRECTORY.get() {
        let attempt = ATTEMPT.fetch_add(1, Ordering::Relaxed);
        std::fs::write(
            directory.join(format!("{attempt:05}.json")),
            serde_json::to_vec(&serde_json::json!({
                "url":url, "method":if body.is_some() {"POST"} else {"GET"}, "http_status":status,
                "transfer_complete":response.status.success(), "response":String::from_utf8_lossy(&response.stdout[..split])
            }))?,
        )?;
    }
    ensure!(
        response.status.success(),
        "HTTP transfer failed; capture stopped without replaying a POST"
    );
    ensure!(
        (200..300).contains(&status),
        "HTTP status {status}; response retained, POST not repeated"
    );
    Ok(serde_json::from_slice(&response.stdout[..split])?)
}

pub fn submit(server: &str, route: &str, body: &str) -> Result<String> {
    let response = request(
        &format!("{}/shielded-vote/v1/{route}", server.trim_end_matches('/')),
        Some(body),
    )?;
    ensure!(
        response["code"].as_u64() == Some(0),
        "chain refused submission (code {:?})",
        response["code"]
    );
    let hash = response["tx_hash"]
        .as_str()
        .context("missing transaction hash")?;
    ensure!(
        hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid transaction hash"
    );
    Ok(hash.to_string())
}

/// Only chain-confirmed successful transactions produce durable confirmations.
pub fn confirm(rpc: &str, hash: &str) -> Result<(Value, Vec<TxEvent>)> {
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if let Ok(response) = request(
            &format!("{}/tx?hash=0x{hash}", rpc.trim_end_matches('/')),
            None,
        ) {
            if response["result"]["tx_result"].is_object() {
                ensure!(
                    response["result"]["hash"]
                        .as_str()
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(hash)),
                    "confirmation hash mismatch"
                );
                ensure!(
                    response["result"]["tx_result"]["code"].as_u64() == Some(0),
                    "transaction rejected on chain"
                );
                let events =
                    serde_json::from_value(response["result"]["tx_result"]["events"].clone())?;
                return Ok((response, events));
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    anyhow::bail!("chain confirmation deadline exceeded")
}

//! Ordered dynamic config mirror attempts, on top of the shared mirror walk.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zakura_service_config::{
    resolve_from_mirrors, AttemptFailure, CandidateRank, MirrorAttempt, MirrorError,
};

use super::{
    resolve_dynamic_voting_config, ConfigCondition, ConfigConditionKind,
    ResolveVotingConfigOptions, ResolvedStaticVotingConfig, ResolvedVotingConfig,
    VotingConfigError,
};

/// One mirror's fetch outcome, supplied by the wallet's transport.
///
/// Config resolution stays transport-agnostic: the wallet fetches each URL from
/// [`ResolvedStaticVotingConfig::dynamic_config_urls`] with its own networking
/// stack and reports the result here. `result` carries the response bytes, or
/// the transport's error message for a mirror that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicConfigAttempt {
    pub url: String,
    pub result: Result<Vec<u8>, String>,
}

impl DynamicConfigAttempt {
    /// Records bytes successfully fetched from `url`.
    pub fn fetched(url: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            url: url.into(),
            result: Ok(bytes),
        }
    }

    /// Records a transport failure for `url`.
    pub fn failed(url: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            result: Err(error.into()),
        }
    }
}

/// Why a dynamic config mirror was passed over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicConfigMirrorFailure {
    pub url: String,
    pub reason: String,
}

/// Resolve the dynamic voting config from ordered mirror attempts.
///
/// Walks `attempts` in order and returns the first one that both fetched and
/// resolved, together with the ordered mirrors that were passed over so the
/// caller can log or surface the degradation.
///
/// A mirror is skipped when its transport failed, when its bytes did not
/// decode, or when it advertised versions this wallet does not support.
///
/// A mirror that resolves but authenticates no rounds is *deprioritized*, not
/// skipped: later mirrors are tried first in case one carries a verifiable
/// round set, but if none does, the round-less resolution is returned. It is a
/// valid config — a round set can legitimately be empty, and rounds signed by
/// keys or under a layout this build does not accept are reported through
/// `skipped_round_ids` — so failing on it would turn a resolution that
/// succeeds today into an error.
///
/// Falling back widens availability, not trust. Every candidate is
/// authenticated against the same static trusted keys by
/// [`resolve_dynamic_voting_config`], so a mirror can serve a stale round set
/// but cannot forge one.
///
/// # Errors
///
/// Returns [`VotingConfigError::InvalidInput`] if `attempts` is empty. When no
/// mirror resolved at all, a single attempt reports its own error verbatim,
/// and multiple attempts report [`VotingConfigError::AllMirrorsFailed`] with a
/// message enumerating every mirror and its reason.
pub fn resolve_dynamic_voting_config_from_attempts(
    resolved_static: ResolvedStaticVotingConfig,
    attempts: Vec<DynamicConfigAttempt>,
    options: ResolveVotingConfigOptions,
) -> Result<(ResolvedVotingConfig, Vec<DynamicConfigMirrorFailure>), VotingConfigError> {
    if attempts.is_empty() {
        return Err(no_attempts());
    }
    let attempts = attempts.into_iter().map(|attempt| MirrorAttempt {
        url: attempt.url,
        fetched: attempt.result,
    });
    let walked = resolve_from_mirrors(
        attempts,
        |_, bytes| resolve_dynamic_voting_config(resolved_static.clone(), bytes, options.clone()),
        |resolved| {
            if resolved.authenticated_rounds.is_empty() {
                CandidateRank::Fallback {
                    reason: "resolved config authenticated no rounds".to_string(),
                }
            } else {
                CandidateRank::Preferred
            }
        },
    );

    match walked {
        Ok(resolution) => {
            let skipped = resolution
                .passed_over
                .into_iter()
                .map(|failure| DynamicConfigMirrorFailure {
                    url: failure.url,
                    reason: failure.reason,
                })
                .collect();
            Ok(finish(resolution.value, resolution.url, skipped))
        }
        Err(MirrorError::NoAttempts) => Err(no_attempts()),
        // Nothing to enumerate, so report the mirror's own error verbatim. This
        // keeps the single-mirror path — every v1 static config — identical to
        // a direct `resolve_dynamic_voting_config` call, kind and message alike.
        // The transport cause stays in the message so callers still see DNS
        // failures and HTTP statuses.
        Err(MirrorError::Exhausted { failures, last }) if failures.len() == 1 => Err(match last {
            AttemptFailure::Transport(error) => VotingConfigError::RemoteAuthenticationFailed {
                message: format!("dynamic config fetch failed: {error}"),
            },
            AttemptFailure::Resolve(error) => error,
        }),
        // Mirrors routinely fail for different reasons, and no one mirror's
        // error can represent the set. Each reason already renders that
        // mirror's own error, so the enumeration preserves every kind,
        // including `UnsupportedVersion`, whose fields have nowhere else to go.
        Err(error) => Err(VotingConfigError::AllMirrorsFailed {
            message: error.summary(),
        }),
    }
}

fn no_attempts() -> VotingConfigError {
    VotingConfigError::InvalidInput {
        message: "dynamic config attempts must contain at least one entry".to_string(),
    }
}

/// Default bound on a single dynamic-config mirror fetch for reference transports.
///
/// A blackholed connection or a server that stops sending must not leave a
/// healthy later mirror unused. Wallets with their own networking stack should
/// apply an equivalent per-attempt deadline before reporting a fetch failure to
/// [`resolve_dynamic_voting_config_from_attempts`].
pub const DYNAMIC_MIRROR_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Lazily fetch and resolve dynamic config mirrors, bounding each attempt.
///
/// Calls `fetch` for each URL in
/// [`ResolvedStaticVotingConfig::dynamic_config_urls`] in order, wrapping every
/// call in `timeout`. After each attempt, preference rules from
/// [`resolve_dynamic_voting_config_from_attempts`] decide whether to stop (a
/// resolution with authenticated rounds) or keep walking (a round-less
/// resolution is kept while later mirrors are still tried).
///
/// `fetch` should return response bytes or a transport error string. A timeout
/// is recorded as a fetch failure with an explicit timed-out reason so it
/// participates in the same skip / all-mirrors-failed path as any other
/// transport error.
///
/// # Errors
///
/// Propagates the error from [`resolve_dynamic_voting_config_from_attempts`]
/// when every mirror fails and no round-less resolution was kept. Returns
/// [`VotingConfigError::InvalidInput`] if the static config names no mirrors.
pub async fn resolve_dynamic_voting_config_over_mirrors<F, Fut>(
    resolved_static: ResolvedStaticVotingConfig,
    timeout: Duration,
    options: ResolveVotingConfigOptions,
    mut fetch: F,
) -> Result<(ResolvedVotingConfig, Vec<DynamicConfigMirrorFailure>), VotingConfigError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, String>>,
{
    let urls = resolved_static.dynamic_config_urls.clone();
    if urls.is_empty() {
        return Err(VotingConfigError::InvalidInput {
            message: "dynamic config attempts must contain at least one entry".to_string(),
        });
    }

    let mut attempts = Vec::new();
    let mut best = None;

    for url in urls.iter() {
        let attempt = match tokio::time::timeout(timeout, fetch(url.clone())).await {
            Ok(Ok(bytes)) => DynamicConfigAttempt::fetched(url.clone(), bytes),
            Ok(Err(error)) => DynamicConfigAttempt::failed(url.clone(), error),
            Err(_) => DynamicConfigAttempt::failed(
                url.clone(),
                format!("timed out after {}s", timeout.as_secs()),
            ),
        };
        attempts.push(attempt);

        match resolve_dynamic_voting_config_from_attempts(
            resolved_static.clone(),
            attempts.clone(),
            options.clone(),
        ) {
            Ok(outcome) => {
                let has_rounds = !outcome.0.authenticated_rounds.is_empty();
                best = Some(outcome);
                if has_rounds {
                    break;
                }
            }
            Err(e) if attempts.len() == urls.len() && best.is_none() => return Err(e),
            Err(_) => {}
        }
    }

    best.ok_or_else(|| VotingConfigError::InvalidInput {
        message: "dynamic config attempts must contain at least one entry".to_string(),
    })
}

/// Attach the fallback condition when earlier mirrors were passed over.
fn finish(
    mut resolved: ResolvedVotingConfig,
    url: String,
    skipped: Vec<DynamicConfigMirrorFailure>,
) -> (ResolvedVotingConfig, Vec<DynamicConfigMirrorFailure>) {
    if !skipped.is_empty() {
        resolved.conditions.push(ConfigCondition {
            kind: ConfigConditionKind::DynamicMirrorFallbackUsed,
            status: true,
            message: format!(
                "resolved from fallback mirror {} after skipping {} mirror(s)",
                url,
                skipped.len()
            ),
        });
    }
    (resolved, skipped)
}

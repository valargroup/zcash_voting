# zcash_voting

Client-side library for integrating [Zcash shielded voting](https://github.com/valargroup/vote-sdk) into a wallet. Wraps the Halo 2 ZKPs, voting hotkey construction from stored app-owned secret material, share construction, and governance-PCZT assembly that a wallet needs to participate in an on-chain voting round.

## Usage

**Upgrading from v3.x or v4.0.x?** Read
[Migrating to v5](../docs/migrating-to-v5.md) for release-specific prerequisites,
API replacements, examples, and database compatibility limits. The
[v5 release notes](../CHANGELOG.md#v500) summarize the final released behavior.

Import `zcash_voting::prelude::*` and use the SDK-owned lifecycle:

1. Open a wallet-scoped sidecar with `VotingDb::open_wallet_sidecar` and persist
   bundle setup through `DelegationPipeline::setup_bundles` or the round APIs.
   Snapshot selection is Ironwood / NU6.3-only. Bundle policy is persisted per
   round; its default includes privacy trimming. Use
   `recoverable_bundle_policy_v1()` when reproducible bundle reconstruction is
   required, and preserve the existing sidecar and voting hotkey secret.
2. Bind a `RoundExecutor` to the round, network, authenticated proposal roster,
   and voting hotkey. Record each choice or explicit skip with
   `set_ballot_intents`, including the expected network and option counts.
3. Supply current service configuration, timing, and signing inputs through a
   `RoundHostSource`, then call `RoundDriver::run`. Inspect its report and stop
   reason to handle missing ballot decisions, bundle setup, signatures,
   failures, or background share work. Software signing stays at the wallet
   boundary; Keystone signatures are persisted for restart.
4. Restore pending shares with `share::pending_rounds_for_accounts` and run
   `ShareTrackingDriver` in the background. It owns polling and recovery cadence
   through vote end. Default foreground execution confirms only the designated
   immediate share, not every share in the round.

Fresh local delegation and choices use `delegate-and-cast-vote-batch` together.
Existing delegations use `cast-vote-batch` for multiple due proposals and
`cast-vote` for a singleton. The chain must support these routes. Imported
capability delegations remain signer-free and poll-only until confirmed.

For specialized manual delivery, use `CommittedVote::prepare_share_delivery`
with the complete authenticated roster, then recover a fresh committed handle
after confirmation and convert it with `CommittedVote::confirmed`. Only
`ConfirmedVote::submit_prepared_shares` submits the prepared plan. Pass the
complete current helper fleet and preserve the stored plan across restart.
The SDK owns entropy, placement, payload construction, and attempt journaling.
Helper confirmation requires two distinct helpers when at least two are
configured, or the only helper for a one-helper fleet.

## Crate layout

| Crate | Purpose |
|---|---|
| **`zcash_voting`** (this crate) | Stable wallet API: round setup, note bundles, delegation precompute/proving, voting hotkey reconstruction from stored app-owned secret material, and round-state storage. |
| [`vote-commitment-tree`](../vote-commitment-tree) | Append-only Poseidon Merkle tree for VANs and vote commitments. |
| [`vote-commitment-tree-client`](../vote-commitment-tree-client) | HTTP client + CLI for syncing the vote commitment tree from a running chain node. |

## Public modules

| Module | Purpose |
|---|---|
| `prelude` | Recommended imports for wallet SDKs. |
| `round` | `VotingDb`, `RoundParams`, `RoundInfo`, idempotent `ensure_bundles`, and policy-aware bundle planning. |
| `precompute` | Shielded note witness generation and PIR precompute wrappers. |
| `delegate` | PCZT setup, coordinated proof generation, signing requests, and capability-export assembly. |
| `chain_submission` | Durable chain dispatch, polling, exact-tree recovery, and atomic confirmation. |
| `round_drive` / `vote_work` | Round driver, bound executor, host inputs, and progress reports. |
| `delegation_pipeline` | Reusable wallet-bound setup, eligibility, proving, and external signing. |
| `share_tracking_drive` | Background helper confirmation and recovery loop. |
| `vote` | ZKP2 construction, bounded parallel batch proving, cast-vote signing, and atomic recovery-bundle persistence. |
| `share` | Helper-share state, policy, and account-scoped pending-round queries. |
| `session` | Durable ballot intent plus the round-level resume planner. |
| `phases` | Per-bundle `DelegationPhase` derived from persisted artifacts. |
| `config` | Static and dynamic voting config validation, signature checks, and switch decisions. |
| `pir` | PIR endpoint selection helpers and client re-exports. |
| `hotkey` | Voting hotkey reconstruction from stored app-owned secret material plus random app-owned hotkeys. |
| `governance` | Low-level governance derivations, `BALLOT_DIVISOR`, and the circuit note-slot count. |

Wallet integrations should use the lifecycle modules above instead of writing
storage rows directly. An atomic batch preserves the original proof's
privacy for choices, notes, amounts, and voting keys. Its deliberate metadata
tradeoff is transaction-level linkage: observers can see that the ordered
proposal actions in the batch were submitted together.

API workflow calls support [optional observability](../docs/observability.md),
including per-bundle proof and submission timings and diagnostics on errors.

## Config resolution

The `config` module keeps voting service config policy in Rust while letting
wallets choose URLs and transport. This is a two-step flow because the dynamic
config URL is trusted only after the static config bytes pass hash-pin and
schema validation. Dynamic config must include the top-level PIR geometry used
by the selected service:

```json
{
  "pir_layout": {
    "pir_depth": 19,
    "tier0_layers": 12,
    "tier1_layers": 7,
    "poly_len": 4096
  }
}
```

Resolution fails closed when the field is missing or malformed, the tier-layer
sum does not equal `pir_depth`, the depth is outside the voting circuit's
supported range of 1 through 29, or `poly_len` is not `2048` or `4096`.

Roll out the additive `pir_layout` object (including `poly_len`) in published
dynamic config before shipping wallet builds that require it. Older wallets
ignore unknown fields. New clients that resolve config without it, or that
connect to a PIR server that does not advertise matching `/root.pir_layout`
and `/params/tier1.poly_len`, fail closed at connect time before any private
query.

This validation describes layouts the compiled client can consume. Snapshot
tooling and fleet deployment determine which of those layouts is currently
available. Wallets intentionally do not require equality with a compiled
production default, so a consistently advertised service layout can change
without requiring a wallet release.

After resolution, wallets typically connect PIR through
`pir::connect_pir_blocking` (or `pir::connect_pir`) with the resolved config's
`pir_layout` and a caller-chosen endpoint URL. The helpers run the
config/server layout and YPIR-degree handshake and fail closed before any
private query (`VotingError::InvalidInput` on mismatch); they do not re-check
advertised-endpoint membership. Do not pass a compiled-client layout constant
in place of `resolved.pir_layout`.

```rust
use std::sync::Arc;
use zcash_voting::{connect_pir_blocking, HyperTransport};

# fn example(
#     resolved: &zcash_voting::wire::ResolvedVotingConfig,
#     pir_url: &str,
# ) -> Result<(), zcash_voting::VotingError> {
let pir_client = connect_pir_blocking(
    resolved.pir_layout,
    pir_url,
    Arc::new(HyperTransport::new()),
)?;
# let _ = pir_client;
# Ok(())
# }
```

When the caller already selected an endpoint (for example after exact-height
snapshot probing), pass that URL together with `resolved.pir_layout`.

```rust
use zcash_voting::config::{
    decide_config_switch, resolve_dynamic_voting_config, resolve_static_voting_config,
    ResolveVotingConfigOptions,
};

# fn example(static_bytes: &[u8], dynamic_bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
let source = "https://example.com/static.json?checksum=sha256:...";

// The wallet resolves the static trust anchor, learns the dynamic config URLs
// from it, fetches one with its chosen transport, then resolves the dynamic
// config bytes against the authenticated static config.
let resolved_static = resolve_static_voting_config(source, static_bytes)?;
let _dynamic_config_urls = &resolved_static.dynamic_config_urls;

let resolved = resolve_dynamic_voting_config(
    resolved_static,
    dynamic_bytes,
    ResolveVotingConfigOptions::default(),
)?;

let switch_decision = decide_config_switch(
    None,
    (&resolved).into(),
);
# Ok(())
# }
```

Hash-pin mismatch and dynamic round signature verification failure are reported
as `VotingConfigError::RemoteAuthenticationFailed`, so callers can surface a
clear "remote authentication failed" message.

### Static config versions and dynamic config fallback

Two static config schema versions are supported, and both resolve through the
same `resolve_static_voting_config` call:

- **v1** (`static_config_version: 1`) names one `dynamic_config_url`.
- **v2** (`static_config_version: 2`) names an ordered `dynamic_config_urls`
  list of mirrors, most preferred first. Every mirror serves the same document;
  the list exists so a wallet is not stranded when the origin it is pinned to is
  unreachable.

Each version owns exactly one of those fields, and a document carrying the
other version's field is rejected rather than reinterpreted. Existing v1 pins
are unaffected: they resolve exactly as before.

`ResolvedStaticVotingConfig::dynamic_config_urls` is always non-empty, so a
wallet can walk it uniformly regardless of schema version.
`dynamic_config_url` remains as its first entry for callers written against v1.

`resolve_dynamic_voting_config_from_attempts` takes the ordered fetch outcomes
and returns the first that resolves, plus the mirrors it passed over:

```rust
use zcash_voting::config::{
    resolve_dynamic_voting_config_from_attempts, DynamicConfigAttempt,
    ResolveVotingConfigOptions, ResolvedStaticVotingConfig,
};

# fn example(
#     resolved_static: ResolvedStaticVotingConfig,
#     fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
# ) -> Result<(), Box<dyn std::error::Error>> {
let attempts = resolved_static
    .dynamic_config_urls
    .iter()
    .map(|url| match fetch(url) {
        Ok(bytes) => DynamicConfigAttempt::fetched(url, bytes),
        Err(e) => DynamicConfigAttempt::failed(url, e),
    })
    .collect();

let (resolved, skipped) = resolve_dynamic_voting_config_from_attempts(
    resolved_static,
    attempts,
    ResolveVotingConfigOptions::default(),
)?;
# let _ = (resolved, skipped);
# Ok(())
# }
```

Async wallets can instead call `resolve_dynamic_voting_config_over_mirrors`,
which walks the same list lazily and wraps each fetch in
`DYNAMIC_MIRROR_FETCH_TIMEOUT` (30s) so a stalled primary cannot block a
healthy later mirror. The wallet-example and `config_fetcher` reference
transports use that helper; wallets with a custom stack should apply an
equivalent per-attempt deadline before recording a fetch failure.
A mirror is skipped when its fetch failed, its bytes did not decode, or it
advertised unsupported versions. A mirror that resolves but authenticates no
rounds is deprioritized rather than skipped: later mirrors are tried first, but
if none carries a verifiable round set, the round-less resolution is returned —
an empty authenticated round set is a valid outcome, with unverifiable rounds
reported through `skipped_round_ids`. When no mirror resolves at all, the error
is `VotingConfigError::AllMirrorsFailed`, whose message enumerates each URL and
its reason — no single mirror's error can stand in for the set, since mirrors
commonly fail for different reasons. A one-mirror list, which is every v1
static config, has nothing to enumerate and reports its own error verbatim —
including the transport cause when the fetch itself failed.

Fallback widens **availability, not trust**. Whichever mirror answers, the
static hash pin still covers the trust anchor and every round is still
authenticated against the static `trusted_keys`, so a mirror can serve a stale
round set but cannot forge one. Resolving from a mirror other than the first
emits a `ConfigConditionKind::DynamicMirrorFallbackUsed` condition so wallets
can surface the degradation.

`ConfigConditionKind::StaticHashPinVerified` reports whether a pin was actually
checked: a source without a `?checksum=sha256:` query resolves, but that
condition is reported as `false`.

`decide_config_switch` classifies the semantic wallet transition as
`InitialLoad`, `Unchanged`, `SameChainServiceUpdate`, `NewChainOrRound`, or
`ProtocolChanged`. The wallet owns executing that branch. Endpoint and signing
key changes and PIR layout changes are same-chain service updates, so wallets
should restart network-derived work, including PIR precompute, while keeping
durable artifacts indexed by round id. Summaries persisted before `pir_layout`
was recorded remain readable and cause the first newly known layout to register
as a service update.
Authenticated round-set changes should reload and reselect the active round
context, but do not by themselves require wiping hotkeys or vote commitments
for old round ids.

A direct-HTTPS reference transport lives in the `wallet-example` crate as
`example_config`. It pairs the `resolve_static_voting_config` /
`resolve_dynamic_voting_config` calls with a `DirectHttpsFetcher` and shows how
to persist the resolved summary used for future switch decisions:

- `resolve_voting_config_over_https` fetches the static config, then walks its
  `dynamic_config_urls` lazily via `resolve_dynamic_voting_config_over_mirrors`
  — each attempt bounded by `DYNAMIC_MIRROR_FETCH_TIMEOUT`, stopping at the
  first mirror that both fetches and authenticates — and returns the
  `ResolvedVotingConfig` together with the mirrors it skipped.
- `resolve_config_switch` resolves the config and classifies it against the
  previously stored summary, returning the `ConfigSwitchDecision` plus the
  `StoredConfigState` to persist for the next run.
- `read_config_state` / `write_config_state` load and save that state, so the
  first run reports an initial load and later runs detect service, round-set,
  or protocol changes.
- `connect_pir_from_resolved` connects a PIR client with that config's
  `pir_layout` and a caller-chosen PIR URL (layout handshake; no hardcoded
  depth/split). Delegation example helpers
  (`precompute_delegation_bundle`, `prove_and_submit_*`) take `PirLayout` plus
  the selected PIR URL instead of the full resolved config.

## Crates.io diagram

```text
zcash_voting
├── config
│   ├── static hash-pin verification
│   ├── dynamic config validation
│   ├── Ed25519 round signature verification
│   └── config-switch decisions
├── vote-commitment-tree-client ─── vote-commitment-tree
├── pir-client / vote-nullifier-pir types
├── voting-circuits
└── librustzcash crates
```

## Shared wallet policy helpers

The `share_policy` module contains pure helpers for wallet-side voting behavior
that should stay consistent across SDKs:

- last-moment helper-share window, deadline, and mode decisions from round
  timing
- delayed helper-share `submit_at` scheduling, capped at 100 hours while still
  ending before the round's last-moment window
- progressive helper timing: inspect ready responses after two seconds, keep
  waiting for the half-fleet target (capped by protocol policy at 10 helpers),
  and stop at 30 seconds
- 30-second helper POST attempts with bounded initial-delivery concurrency
- helper confirmation polling bounded to four concurrent requests and ten
  seconds per share so stalled helpers cannot starve later shares
- share-count-derived batch planning with independent entropy per share, a
  minimum capacity pool, and a hard initial quota of `floor(3S / 4)` shares per
  helper (12 when `S = 16`); retries remain liveness-first and may exceed it
- resubmission ordering with untried helpers first; overdue recovery then
  retries outcome-unknown helpers before falling back to already-sent helpers
- share tracking summaries, readiness checks, retry thresholds, and polling delay

The SDK-owned delivery workflow supplies entropy and applies these policies.
Hosts supply authenticated timing and the complete helper fleet; use
`ShareTrackingDriver` for pass scheduling.

## Secret boundaries

Wallet seed material should stay in the wallet integration. For v5 integrations,
generate a random app-owned voting hotkey with `generate_random_voting_hotkey`,
store `VotingHotkey::stored_secret()` in platform secure storage, and
reconstruct a typed hotkey with `VotingHotkey::from_stored_secret` when needed.
Software and hardware wallets should follow the same random hotkey model. The
hotkey is not deterministic across fresh installs unless the stored hotkey
secret is restored. For local delegation, the crate derives each bundle's VAN
blinding from that restored secret and the exact network, round parameters,
bundle index, note positions, commitments, and values. Rebuilding from the same
secret and `recoverable_bundle_policy_v1()` therefore reconstructs the same VAN
without a separate recovery table. The policy owns the complete versioned
bundle shape, including the 25,000 ZEC addition threshold; callers do not need
to layer that recovery input on separately. Public-target custody delegation
has no hotkey secret and retains its existing persisted-randomness recovery
contract.

Delegation signing follows the same boundary. After `setup_delegation`, call
`delegation_signing_request` to load the account index, network, seed
fingerprint, PCZT sighash, and spend auth randomizer. Software wallets should
derive the account SpendAuth key locally, randomize it with `alpha`, and sign
the sighash through the pipeline's `SpendAuthSigner` callback. For manual
standalone delegation advancement, pass only the resulting signature to
`ChainSubmissionClient::advance_delegation_with_recovery` with
`ChainRecoveryMode::ExactTree`; the client reloads the authoritative sighash
and randomized verification key from the locked bundle.
The crate no longer accepts root wallet seed material for delegation signing.
An imported capability delegation instead uses
`ChainSubmissionClient::advance_imported_delegation`: it adopts the package's
stored transaction hash and only polls it, without a signer or POST.

### Keystone proof warmup

`DelegationPipeline::ensure_proof` can warm a Keystone bundle before the device
signs. The SDK stores the exact finalized PCZT with its signing context;
`DelegationPipeline::keystone_request` reloads it after warmup or a process
restart. Concurrent request creation and proof setup converge on one durable
transaction. Proof generation retains its existing single-flight coordination.
The host owns scheduling, cancellation, hotkey custody, and device signing.

Schema 22 adds nullable PCZT storage and preserves existing rounds. A legacy
bundle whose original PCZT is unavailable returns
`DelegationPcztUnavailable` when asked for a Keystone request. Do not
reset or rebuild such a bundle automatically. Existing validated software
proof reuse and authoritative chain submissions do not require PCZT bytes.

## Dependency notes

This crate contains the canonical implementation and retains mutually
exclusive `lrz`/`zakura` features. Its default is the Zakura wallet-libraries
family; use `--no-default-features --features lrz` for upstream librustzcash.

Wallet-family selection is consolidated in `zakura-wallet-lib` using its only
two complete backend modes: `zakura` and `lrz`. Unlike its former generic
capability selectors, those features never weak-reference both optional
backend families. External-consumer regression tests verify that selecting
`zcash_voting/lrz` puts no Zakura forks in Cargo lockfiles or resolved
metadata. The selected wallet facade release is `zakura-wallet-lib
0.1.0-rc5`.

`Cargo.toml` is the source of truth for version and feature requirements, and
`Cargo.lock` records the exact package sources and versions used by this branch.
This release line requires Rust 1.91 or newer.

- **`orchard 0.16`** from [zcash/orchard](https://github.com/zcash/orchard),
  with `unstable-voting-circuits` enabled for the governance proof paths
  (or `zakura-orchard 1.2.0` with the `zakura` feature).
- **`voting-circuits 0.12.1`** from [valargroup/voting-circuits](https://github.com/valargroup/voting-circuits)
  for the delegation and vote proof circuits.
- **`vote-commitment-tree 0.6.1`** and
  **`vote-commitment-tree-client 0.8.1`** for vote commitment tree state
  and HTTP sync.
- **`pczt 0.10.0-pre.0`, `zcash_client_backend 0.25.0-pre.0`,
  `zcash_client_sqlite 0.23.0-pre.0`, `zcash_keys 0.17.0-pre.0`,
  `zcash_primitives 0.31.0-pre.0`, and `zcash_protocol 0.11.0-pre.0`** from
  the librustzcash NU7 pre-releases (or the stable `zakura-*` family and RC5 wallet crates in
  `zakura` builds).

## Downstream test fixtures

Downstream integration and FFI tests can enable the non-default
`test-fixtures` feature as a development dependency when they need committed
vote recovery state without building ZKP2. Use the same source and version as
the runtime dependency and add `features = ["test-fixtures"]` to the crate's
development dependency entry.

Create the round and its bundles through the normal public setup APIs, then use
`zcash_voting::vote::insert_recovery_fixture`. The helper atomically stores the
resulting post-commit state but deliberately skips every commit-time
verification gate. It leaves transaction and confirmation fields unset so
tests can exercise the public submission and confirmation APIs. Only pass
trusted fixture data. Cargo features are additive and are not a security
boundary, so production builds should not enable this feature.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../LICENSE-MIT) and [LICENSE-APACHE](../LICENSE-APACHE).

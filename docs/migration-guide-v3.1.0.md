# Migrating wallet integrations from v3.0.0 to v3.1.0

## Scope

This guide is for wallet integrators upgrading `zcash_voting` from `v3.0.0`
to `v3.1.0`. It separates required compatibility work from additive APIs that
can be adopted later.

The stable `v3.1.0` release contains the same implementation as
`v3.1.0-rc.16`. The Vizor examples below followed the release candidates as
they were published and therefore mention RC versions. New integrations should
pin the stable release directly rather than replaying that version sequence.

Do not include changes from the current `Unreleased` section when performing
this migration. In particular, the proposal-ID and atomic-batch range increase
to 50 is newer than `v3.1.0`.

## Recommended migration order

1. Choose one wallet backend and align the complete dependency graph.
2. Open a copy of an existing voting database and verify its in-place schema
   migration before changing wallet orchestration.
3. Update bundle planning and privacy-trim reporting.
4. Replace removed lightwalletd and helper-share APIs.
5. Move initial helper delivery, confirmation, and recovery to the SDK-owned
   lifecycle.
6. Add restart, cancellation, account deletion, and wallet-reset coverage.
7. Adopt config mirrors, background precomputation, prepared vote commits, and
   atomic vote batches as separate follow-ups.

## 1. Align the backend and dependency graph

`v3.1.0` defaults to the Zakura wallet and cryptography stack. An integrator
using Zakura can use the default:

```toml
[dependencies]
zcash_voting = "=3.1.0"
```

An integrator remaining on upstream librustzcash must select the mutually
exclusive `lrz` backend:

```toml
[dependencies]
zcash_voting = { version = "=3.1.0", default-features = false, features = ["lrz"] }
```

Do not enable both backends. Align direct wallet, PCZT, Orchard, cryptography,
PIR, and vote-commitment-tree dependencies with the selected family; mixed
families produce incompatible public Rust types and duplicate dependency
graphs. `v3.1.0` requires Rust 1.91.

Vizor examples:

- [#530](https://github.com/chainapsis/vizor-wallet/pull/530) moved the complete
  wallet and voting graph to Zakura while the backend still required an
  explicit feature.
- [#581](https://github.com/chainapsis/vizor-wallet/pull/581) adopted the final
  Zakura-default feature contract and aligned the published dependency family.
- [#585](https://github.com/chainapsis/vizor-wallet/pull/585) pinned the
  code-identical final release candidate. Vizor `main` still names that RC tag;
  new integrations should use stable `v3.1.0`.

## 2. Preserve existing voting state

Voting databases created by the launched v3 line migrate in place. Test the
upgrade against a copy containing:

- a round with planned bundles;
- a submitted delegation awaiting a vote;
- a committed vote awaiting chain confirmation;
- accepted, delayed, and unconfirmed helper shares; and
- more than one wallet or account scope, if the host supports them.

Do not delete or rebuild the sidecar merely because the library version
changed. The migration preserves round policy, PIR cache state, helper delivery
evidence, and recovery material. There is no longer a standalone
`recovery::clear` or `VotingDb::clear_recovery_state` operation. Ordinary reset
must preserve submission evidence; explicit round or account deletion is the
destructive boundary.

Schema migrations and concurrent writes can occur when a connection opens.
Serialize sidecar opening and short write phases per wallet, and drain voting
work before account deletion or wallet reset.

Vizor examples:

- [#541](https://github.com/chainapsis/vizor-wallet/pull/541) serialized
  sidecar writes while keeping proving and network work outside the write lock.
- [#589](https://github.com/chainapsis/vizor-wallet/pull/589) drained PIR and
  proving precomputation before destructive wallet operations.
- [#585](https://github.com/chainapsis/vizor-wallet/pull/585) removed Vizor's
  obsolete recovery-clear wrapper.

## 3. Update bundle planning and privacy reporting

`BundlePolicy::default()` now trims trailing low-value bundles toward two
privacy bundles, bounded by both one percent of selected value and 1,000 ZEC.
An integrator has two valid choices:

- keep the new default and display the raw value omitted by the privacy trim;
  or
- explicitly opt out with `.with_max_privacy_bundles(None)`.

Do not silently accept the default while displaying the pre-trim voting power.
Use the round-aware APIs so resumed rounds use their persisted policy:

- `VotingDb::effective_bundle_policy`;
- `voting_power_for_round`;
- `note_bundles_for_round`;
- `bundle_notes_for_index_for_round`;
- `VotingNoteSelectionResultView::from_selected_for_round`; and
- `minimum_voting_eligibility_and_plan_for_notes`.

Update struct literals and generated bindings for the new privacy fields.
`BundleLayout` uses the flat `privacy_trim_dropped_*` fields.
`SignedDelegationBundle` and `SignedDelegationPayloadView` do not carry privacy
trim fields. `BundlePolicy::with_privacy_drop_bps` now returns
`Result<Self, VotingError>`.

Wallets that must reconstruct identical bundle identities after loss of the
voting database should use `recoverable_bundle_policy_v1()`. This policy also
owns the 25,000 ZEC ZIP-318 bundle-addition threshold; do not maintain a
separate host copy.

Vizor examples:

- [#535](https://github.com/chainapsis/vizor-wallet/pull/535) adopted the
  privacy trim, used round-aware planning, regenerated bindings, and surfaced
  omitted value to voters.
- [#532](https://github.com/chainapsis/vizor-wallet/pull/532) introduced
  Vizor's original 25,000 ZEC policy.
- [#585](https://github.com/chainapsis/vizor-wallet/pull/585) replaced that
  host policy with `recoverable_bundle_policy_v1()`.

## 4. Keep network routing host-owned

The URL-taking lightwalletd helpers that opened a direct channel were removed:

- `latest_block_height`;
- `latest_block_height_with_retry`;
- `tree_state_bytes`;
- `anchor_tree_state_with_retry`; and
- `anchor_tree_state_bytes_with_retry`.

Open the lightwalletd client through the wallet's selected direct, Tor, proxy,
or pooled route and call `get_latest_block`, `get_tree_state`, or
`lwd::anchor_tree_state_with_retry_on`. A privacy-preserving route must fail
closed rather than fall back to direct networking.

Helper networking follows the same rule. Implement `HelperTransport`, or inject
a route-aware connector into `HyperTransport`, and return enough transport
metadata for the SDK to distinguish definite pre-dispatch failures from
ambiguous POST outcomes. Invalidate pooled connections when the wallet changes
to a route that forbids their reuse.

Vizor examples:

- [#549](https://github.com/chainapsis/vizor-wallet/pull/549) moved snapshot
  fetching onto a caller-owned lightwalletd route.
- [#557](https://github.com/chainapsis/vizor-wallet/pull/557) retained multiple
  exact-height PIR endpoints for transport failover.
- [#570](https://github.com/chainapsis/vizor-wallet/pull/570) routed
  `HelperTransport` through Vizor's direct/Tor policy without silent fallback.

## 5. Replace host-owned helper delivery

This is the largest required migration. Remove use of
`submit_share_to_helpers(ShareSubmissionRequest)` and any host exposure of
encrypted helper payload construction, placement, confirmation persistence, or
recovery policy.

For each committed vote:

1. Record every proposal choice or skip as a terminal ballot intent.
2. Pass the complete authenticated proposal-ID roster, not only answered
   proposals.
3. Construct a `HelperClient` with the host's `HelperTransport`.
4. Call `HelperClient::preflight_fleet` with the complete authenticated helper
   fleet. Invalid, empty, duplicate, or canonically equivalent helper URLs are
   configuration errors and must not be silently dropped.
5. Call `CommittedVote::prepare_share_delivery`. The SDK derives the immediate
   share, owns helper ordering and timing entropy, validates aggregate
   placement, and persists one complete generation-bound plan before any POST.
6. Record vote-chain confirmation and its real VC-tree position.
7. Recover a fresh `CommittedVote`; the pre-confirmation handle is stale after
   the confirmation transition.
8. Call `CommittedVote::submit_prepared_shares` with the complete current
   configured fleet.
9. Continue calling `track_pending_shares` until all shares are confirmed or
   the round ends. Use `confirm_pending_share` only for a focused immediate
   confirmation check; it applies the same quorum rules.
10. On process start or wallet unlock, restore unfinished work from
    `share::pending_rounds`.

The host remains responsible for authenticated configuration, the network
route, cancellation, app lifecycle, timers, and safe draining. The SDK owns
placement, payload validation, POST retry classification, durable attempt
journaling, confirmation quorum, and recovery ordering.

Do not retry an ambiguous POST as if it were definitely unsent. Do not mark a
share confirmed from one helper when two or more helpers are configured.
Detailed requirements are in
[helper submission invariants](helper_submission_invariants.md).

Vizor examples:

- [#528](https://github.com/chainapsis/vizor-wallet/pull/528) added restart
  discovery and background share recovery using durable SDK state.
- [#569](https://github.com/chainapsis/vizor-wallet/pull/569) adopted the
  SDK-derived immediate share and confirmation gate.
- [#573](https://github.com/chainapsis/vizor-wallet/pull/573) adopted shared
  readiness, timing, target balancing, and progressive delivery policy.
- [#570](https://github.com/chainapsis/vizor-wallet/pull/570) is the primary
  final-state example: it replaced host planning, delivery, polling, retries,
  health scoring, and recovery with the SDK-owned lifecycle.
- [#567](https://github.com/chainapsis/vizor-wallet/pull/567) carried the
  stacked PIR-cache, immediate-share, and progressive-delivery work into
  Vizor's `main`.

Earlier Vizor work in
[#539](https://github.com/chainapsis/vizor-wallet/pull/539),
[#546](https://github.com/chainapsis/vizor-wallet/pull/546), and
[#548](https://github.com/chainapsis/vizor-wallet/pull/548) improved the former
host-owned submission path. Use their lifecycle and end-to-end tests as
references, but do not copy their superseded host-side helper policy.

## 6. Adopt config mirrors

Static config version 1 remains supported, so mirror adoption need not block
the binary upgrade. It is nevertheless recommended for production resilience.

For static config version 2:

1. Embed independently hosted static URLs carrying the same SHA-256 pin.
2. Try static mirrors in order and authenticate every response.
3. Read `ResolvedStaticVotingConfig::dynamic_config_urls`.
4. Apply a per-attempt deadline while fetching those dynamic mirrors.
5. Pass ordered `DynamicConfigAttempt` values to
   `resolve_dynamic_voting_config_from_attempts`, or use
   `resolve_dynamic_voting_config_over_mirrors`.
6. Preserve and expose mirror failure attribution for diagnostics.

Fallback expands availability, not trust. Every successful candidate still
requires the static hash pin and dynamic-round signatures.

Vizor examples:

- [#525](https://github.com/chainapsis/vizor-wallet/pull/525) added a resilient
  gateway before multi-origin static pins were available.
- [#534](https://github.com/chainapsis/vizor-wallet/pull/534) is the complete
  v2 static and dynamic mirror integration.

## 7. Additive APIs that can follow the compatibility migration

### Background PIR and delegation precomputation

Use `precompute_pir_proofs` and `validate_cached_pir_proofs` to warm verified
real-note proofs before round setup. Use `precompute_snapshot_bundles` once the
wallet has scanned through the round snapshot. Treat warm-up as best-effort;
the foreground proving path remains the correctness fallback.

If precomputation can create a voting hotkey or write the sidecar, register it
with the wallet's destructive-operation drain.

Examples:

- [#549](https://github.com/chainapsis/vizor-wallet/pull/549) added PIR cache
  warm-up and snapshot precomputation.
- [#582](https://github.com/chainapsis/vizor-wallet/pull/582) continued into
  background software-wallet ZKP1 generation.
- [#584](https://github.com/chainapsis/vizor-wallet/pull/584) deduplicated
  completed precompute passes.
- [#589](https://github.com/chainapsis/vizor-wallet/pull/589) made those jobs
  safe around reset and account deletion.

### Prepared vote commits

`prepare_commit`, `prepare_commit_batch`, `persist_prepared_commit`, and
`persist_prepared_commit_batch` allow expensive ZKP2 work to run outside the
SQLite write transaction. Persist only if authority, ballot intent, and vote
state are unchanged. `warm_zkp2_proving_cache` can move parameter
initialization out of the foreground path.

[#541](https://github.com/chainapsis/vizor-wallet/pull/541) demonstrates this
split, bounded proving concurrency, per-bundle authority-chain ordering, and
partial-failure recovery.

### Atomic multi-proposal batches

`commit_atomic_vote_batch`, `prepare_atomic_vote_batch`, and
`recover_atomic_vote_batch` submit one canonical `SignedVoteBatch` to
`cast-vote-batch`. The chain accepts the complete ordered authority chain or
none of it. Existing batch APIs remain singleton compatibility wrappers, so
atomic batching can be adopted separately.

Before adopting it, verify that the target vote chain supports
`cast-vote-batch`, preserve canonical proposal order, persist the shared
transaction hash, confirm the complete batch atomically, and defer member
helper delivery until confirmation provides every VC-tree position.

## 8. Verification checklist

Before releasing an upgraded wallet:

- Build with exactly one backend and inspect dependency metadata for duplicate
  upstream/Zakura families.
- Run with Rust 1.91 or newer.
- Migrate a copy of a real v3.0.0 sidecar and verify in-flight rounds survive.
- Verify resumed rounds retain their pre-upgrade bundle policy.
- Verify new rounds either display privacy-trimmed value or explicitly disable
  trimming.
- Reject invalid and canonically duplicate helper URLs before network I/O.
- Exercise one-helper and multi-helper fleets; multi-helper confirmation must
  require two distinct matching responses.
- Simulate definite helper failures, ambiguous POST outcomes, cancellation,
  process restart, helper removal, and helper replacement.
- Confirm every POST is durably journaled before dispatch and ambiguous
  outcomes are not treated as definitely unsent.
- Confirm a pre-confirmation `CommittedVote` is not reused after chain
  confirmation.
- Drain initial delivery, tracking, PIR warm-up, and proving work before wallet
  reset or account deletion.
- Verify Tor or proxy mode never creates an unintended direct lightwalletd,
  PIR, or helper connection.
- Regenerate and test language bindings for changed public structs and errors.
- Run an end-to-end vote through delegation, vote-chain confirmation, helper
  delivery, restart restoration, and final share confirmation.

## Vizor PR index

The complete set of useful Vizor migration examples identified between its
stable-v3.0 integration and final-v3.1 implementation is:

- [#525 — resilient voting config gateway](https://github.com/chainapsis/vizor-wallet/pull/525)
- [#528 — background share recovery](https://github.com/chainapsis/vizor-wallet/pull/528)
- [#530 — Zakura stack](https://github.com/chainapsis/vizor-wallet/pull/530)
- [#532 — original 25,000 ZEC policy](https://github.com/chainapsis/vizor-wallet/pull/532)
- [#534 — config mirrors](https://github.com/chainapsis/vizor-wallet/pull/534)
- [#535 — privacy trim](https://github.com/chainapsis/vizor-wallet/pull/535)
- [#539 — helper submission and regtest E2E](https://github.com/chainapsis/vizor-wallet/pull/539)
- [#541 — prepared commits and proving concurrency](https://github.com/chainapsis/vizor-wallet/pull/541)
- [#544 — v3.1 dependency and mobile integration](https://github.com/chainapsis/vizor-wallet/pull/544)
- [#546 — background share completion](https://github.com/chainapsis/vizor-wallet/pull/546)
- [#548 — helper failover and preflight](https://github.com/chainapsis/vizor-wallet/pull/548)
- [#549 — PIR cache and snapshot precompute](https://github.com/chainapsis/vizor-wallet/pull/549)
- [#557 — PIR endpoint failover](https://github.com/chainapsis/vizor-wallet/pull/557)
- [#567 — post-launch voting integration branch](https://github.com/chainapsis/vizor-wallet/pull/567)
- [#569 — immediate-share confirmation](https://github.com/chainapsis/vizor-wallet/pull/569)
- [#570 — SDK-owned helper lifecycle](https://github.com/chainapsis/vizor-wallet/pull/570)
- [#573 — progressive helper delivery](https://github.com/chainapsis/vizor-wallet/pull/573)
- [#581 — Zakura-default final dependency shape](https://github.com/chainapsis/vizor-wallet/pull/581)
- [#582 — background software ZKP1](https://github.com/chainapsis/vizor-wallet/pull/582)
- [#584 — precompute deduplication](https://github.com/chainapsis/vizor-wallet/pull/584)
- [#585 — deterministic VAN policy and final v3.1 implementation](https://github.com/chainapsis/vizor-wallet/pull/585)
- [#589 — destructive-operation draining](https://github.com/chainapsis/vizor-wallet/pull/589)

For historical context,
[#506](https://github.com/chainapsis/vizor-wallet/pull/506) is the Vizor PR
that established the stable `v3.0.0` baseline.

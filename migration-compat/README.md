# Real-release migration compatibility

This suite distinguishes **a real historical voting database** from a database
whose schema merely resembles an old release. The producer builds against
v3.0.0 (`37a0ea9530a26d8b3e965db09fafc119441dee38`) without changing its source,
lockfile, or crypto dependencies. It does not enable `test-fixtures` or write
SQL voting records. The added host adapter calls the release's wallet scanning,
delegation, proving, signing, confirmation, and share APIs.

The existing `storage/migrations/historical/v3_0_0_completed_round.sql` remains
useful for migration mechanics. Its recovery JSON is incomplete, and it includes
unfinished votes. It is not evidence that a real completed vote displays after
migration. Do not promote it to a live capture or hand-fill its missing fields.

## Running

All entry points are Make targets, run from the repository root:

- `make migration-compat-build`: build the old producer and both history readers.
- `make migration-compat-capture`: provision a new staging round, complete voting
  with v3.0.0, capture both accepted and confirmed share states, and replay them.
  This uses real staging services and needs `VOTE_MANAGER_VOTE_SDK` and
  `VOTE_SDK_VOTER_TEST` in the runtime environment. For example:

  ```sh
  infisical run --env=staging \
    --projectId=40862c6d-a089-4355-b405-0477be0ee3b1 \
    -- make migration-compat-capture
  ```

- `make migration-compat-capture CAPTURE_CONFIG=/absolute/path/capture.json`:
  consume previously provisioned parameters. The artifact directory must not
  exist; a failed capture is preserved, never overwritten or resumed implicitly.
  Configuration requires an explicit `chain_id` matching the RPC response;
  production `zvote-1` is rejected. Custom capture configurations bypass the
  staging-only preflight and use their own PIR endpoint during delegation.
- `make migration-compat-replay FIXTURE_DIR=/absolute/path/capture-directory`:
  compare immutable captures against the old reader and the current reader, then
  run interruption tests on copies of each capture. No voting credentials are
  needed. Initial builds may fetch dependencies; history readers perform no
  network operations and on macOS run with network access denied by sandbox-exec.
- `make migration-compat-verify-old FIXTURE_DB=/absolute/path/wallet.sqlite.voting`:
  verify retained delegation proofs with v3.0.0's own verifier and reject a
  damaged-proof control. This does not broadcast or alter the database.
- `make migration-compat-unit`: test that the artifact comparator detects loss,
  same-length BLOB substitution, changed choices/recovery, and preserves WAL data.
- `make migration-compat-regression`: deterministic concurrent migration regression.
- `make migration-compat-faults FIXTURE_DB=/absolute/path/capture.sqlite`: deny SQL
  or kill child processes inside early, middle, and late migration transactions,
  and immediately after commit.

The producer is a release build in `target/migration-compat/old-build`; main's
reader uses `target/zakura`. The old tracked source and lockfile are checked
against the pinned Git archive before each build. Both readers compile the exact
same `history.rs`, so the baseline is not a second hand-coded approximation of
what the old SDK returned. The capture/replay tools never enter `make test`'s
live-network path. Native migration regressions are part of `make test`.

## Isolated voting chain

The default provisioning target remains staging-specific. To provision against a
separate compatible vote chain, use:

```sh
make migration-compat-provision-isolated \
  DEPLOYMENT_CONFIG=/absolute/path/deployment.json \
  ARTIFACT_DIR=/absolute/path/new-capture \
  CAPTURE_CONFIG=/absolute/path/new-capture.json
make migration-compat-capture CAPTURE_CONFIG=/absolute/path/new-capture.json
```

Provisioning needs `MIGRATION_V3_VM_MNEMONIC` injected at runtime. The deployment
JSON supplies `chain_id` (beginning `migration-v3-`), `chain_rpc`, `vote_server`,
`helper_urls`, `pir_url`, `pir_layout`, `lightwalletd`, and `manager_address`.
The adapter verifies the chain ID and manager address before sending a single
round-creation transaction, retains its receipt, then waits for chain confirmation
and an active ceremony. Existing staging-only conformance guards remain intact.

The isolated voting chain may reuse read-only Zcash testnet lightwalletd and PIR
sources; this is not an entirely disconnected network. Helper queues, vote-chain
state, keys, and voting submissions are isolated. Deployment evidence must record
these shared dependencies. Retain hosts until both captured phases pass replay;
afterward preserve the local evidence before deleting the temporary hosts.

## What constitutes a successful capture

The producer scans the fixed staging wallet using the old wallet implementation,
requires multiple bundles, chooses option 0 on proposal 1 and option 1 on
proposal 2, and skips proposal 3. The provisioning ballot has two, three, and four
options respectively. It generates real delegation and vote proofs, sends actual
transactions, and passes successful chain transaction events through the old
confirmation APIs. The old verifier checks delegation proofs before dispatch.

The capture profile uses 16 shares per vote, the staging primary helper, and the
release's supported expedited scheduling (`last_moment_buffer_seconds=7200`).
This exercises full recovery material, not a single-share shortcut. It does not
claim coverage of normal delayed timing or multiple-helper availability.

An HTTP failure never triggers another POST. Complete responses, including
rejections, are retained in `http-evidence/`. An ambiguous attempt fails the run;
it is not labelled confirmed. Accepted shares must make the old SDK report
`completed_for_display` before the first snapshot is accepted. The second
snapshot requires all local share confirmations obtained from the real helper.
Helpers are trusted to report confirmation, as in the released SDK contract.

`VACUUM INTO` captures committed WAL contents into standalone SQLite files.
Private artifact directories use a restrictive umask. Seed and hotkey secrets
are not written into the run configuration or logs. The wallet and sidecar still
contain sensitive wallet/recovery material: retain them in controlled local or
private artifact storage, not Git. Hashes and a non-sensitive result summary can
be shared independently.

The manifest contains release and lockfile identity, producer hash, round
parameters, wallet scope, profile, evidence hash, capture hashes and old SDK
history. A fresh old process must reproduce that history before migration.
Main must reproduce its choices, skips, timestamps, state and wire display after
migration, reopen, and creation of another round. Another wallet scope must not
see the history. The comparator checks exact legacy fields, allowing only the
specified recovery metadata normalization and IMT-to-PIR cache migration. It
also checks schema equivalence, integrity and foreign keys.

Each replay runs real migration rollback and process-kill tests on copies of the
capture. A missing input, empty capture set, checksum mismatch, failed baseline,
missing crash boundary, interrupted test or failed assertion is a failure, not a
skip. Never run the comparator under `python -O`.

## Live-validation result

See [the compatibility report](../docs/migration-compatibility.md). A real
v3.0.0 capture on isolated vote-sdk v1.3.0 passed the full replay for both accepted
and confirmed share snapshots: three delegation bundles, six votes, 96 shares,
and identical history after schema 13 → 24 migration. Current staging had rejected
an old-valid delegation proof; advertised protocol versions and a PIR handshake
alone did not establish verifier compatibility. The report records exact builds,
artifact hashes, shared read-only dependencies, and cleanup evidence.

## External tally dependency

The SDK sidecar stores the voter's choices and recovery history, not aggregate
election results. Wallet integrations must retain the sidecar and stable wallet
identity, supply the historical proposal roster and option labels, and obtain
final tally results from the correct network/round service or their own cache.
This suite does not test a wallet UI, guarantee historical service availability,
or establish that a locally recorded choice contributed to the final tally.
A passing replay proves the stated SDK compatibility properties for its captured
states; it is not a universal no-break guarantee.

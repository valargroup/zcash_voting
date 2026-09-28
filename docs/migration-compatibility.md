# v3.0.0 → main SDK migration compatibility

## Verdict and scope

**Completed real v3.0.0 voting history survived migration to main plus the
concurrent-opening fix in this change.** Both accepted-share and confirmed-share
captures passed the full replay. The successful run used an isolated vote-sdk
v1.3.0 testnet after current staging rejected the released SDK's delegation proof.

This is SDK compatibility validation. No universal “no breaks” guarantee follows
from these tests. Aggregate round tallies are an external dependency, described
below. Do not describe the incomplete staging capture as a completed round.

The baseline producer is pinned to v3.0.0 commit
`37a0ea9530a26d8b3e965db09fafc119441dee38`, schema 13. The current migration target
is schema 24. Local validation was performed on main `45d1a039` plus this change;
replay records the actual checked-out commit and reader binary hash so that a
future main tip must be tested again.

## Defect found

Two SDK processes could read the same old `PRAGMA user_version` before either
acquired the migration write lock. The second process then selected a stale
migration ladder after the first committed an upgrade. That could repeat DDL
against an already-upgraded database and fail opening the sidecar.

`migrate` now rereads the version under its immediate transaction lock. It
continues from that committed version, accepts an already-current database, or
rejects a newer unsupported version. A deterministic regression arranges the
lock contention with a SQLite busy handler. It was also run against unpatched
main and failed with `table chain_submissions already exists`, confirming the
regression detects the actual defect. The fixed source was restored afterward.
It covers another opener advancing
to an intermediate version, the current version, and an unsupported version.

## Implemented evidence suite

The [capture/replay instructions](../migration-compat/README.md) describe the
Make entry points. The old producer builds the unmodified release and lockfile,
adds only a host adapter, and uses released wallet scanning, proving, signing,
confirmation and share APIs. It requires multiple bundles, two selected
proposals and one skipped proposal. Both released and current history readers
compile the same source and call SDK storage/planning/display APIs.

A successful capture must provide accepted-share and confirmed-share snapshots.
Offline replay checks frozen old history, simultaneous current-process opens,
exact legacy values including BLOBs, allowed recovery normalization, cache
migration, wallet isolation, schema equivalence, foreign keys, integrity,
reopening, and history after creating another round. The manifest binds the
release, lockfile, producer, configuration, historical roster, HTTP evidence,
and databases by identity or checksum. Reader processes receive no signing
credentials; on macOS their network access is denied.

Native fault tests deny DDL and kill real child processes at early, middle and
late migration boundaries. They require the original schema, rows and version
to survive, then retry the migration. A fourth process-kill boundary after
commit requires the fully migrated state to survive. The same tests accept a
retained release database and always work on copies. Comparator unit tests
reject row loss, same-length BLOB replacement, altered choices and altered
recovery decisions; they also exercise copying committed WAL data.

The live profile uses 16 shares per vote and one helper with expedited scheduling.
It does not cover normal delayed submission timing or multi-helper availability.
The full completed-capture path executed successfully on both retained snapshots.

## Observed staging failure

Two fresh capture attempts were made. Neither completed delegation, and neither
was retried after an ambiguous POST. The second attempt retained the complete
HTTP response: status 422, code 1, delegation proof verification failed with
`ConstraintSystemFailure`. The actual retained proof passes v3.0.0's own verifier;
a deliberately damaged copy fails it. This establishes a concrete compatibility
failure at the server boundary, not its exact deployed root cause. Advertised
protocol versions and the old PIR handshake had passed.

Controlled local artifacts (sensitive, excluded from Git):

- `target/migration-compat/capture-1789274773989775000/wallet.sqlite.voting`
- `target/migration-compat/capture-1789274773989775000/http-evidence/`
- `target/migration-compat/capture-1789274773989775000/partial-migration-report.json`

The retained sidecar SHA-256 is
`02dc6004528020b88e0178f61d9d6645539647e8f9715021c7519561fb705e79`.
The old and main readers returned equal history on a copy, and exact legacy
values survived migration. Both correctly report that this round is incomplete.
Rollback and process-kill tests also passed on copies of this actual database.
Those partial findings alone did not establish completed-round compatibility;
the subsequent isolated run supplied the completed-round evidence.

## Validation performed

- `make check`: passed.
- `make test`: 1,745 passed, 11 skipped (default Zakura suite), including a final
  run after restoring the fixed source from the unpatched countercheck.
- `make test-lrz`: passed; run because test-owned SQLite hooks changed dependency
  feature unification.
- `make clippy`: passed with existing repository warnings.
- `make migration-compat-unit`: all seven comparator tests passed.
- `make migration-compat-build`: old producer and both readers built successfully.
- `make migration-compat-regression`: deterministic lock-race regression passed.
- `make migration-compat-faults FIXTURE_DB=...`: passed on the retained real sidecar.
- `make migration-compat-verify-old FIXTURE_DB=...`: retained proof accepted and
  damaged control rejected.
- Targeted formatting passes. Repository-wide `make fmt` reports unrelated
  existing formatting differences in `recovery-conformance/examples/plan_probe.rs`
  and `recovery-conformance/src/child/crash_transport.rs`.

## Successful isolated testnet proof

The run used vote-sdk `v1.3.0`, commit
`a52ebc69c70a29894ad72f744c6e16511fd5f792`, whose verifier pins
`voting-circuits = "=0.10.0"`. Its published Linux AMD64 archive was checked
against SHA-256
`5c33e7f79a1584b25f50a2690fe9beecc160df1d31737bc62010f2aeb6eb2164`.
SDK source and dependency pins remained unchanged.

Two temporary DigitalOcean hosts were placed in ValarGroup's `misc` project in
`nyc1`: primary 4 vCPU/16 GB/200 GB and secondary 4 vCPU/8 GB/160 GB. They used
chain ID `migration-v3-1`, a separate vote manager, private helper queues, and
isolated chain/configuration endpoints. Public Zcash testnet lightwalletd and
staging PIR were reused as read-only data/proof sources; this was not an offline
or fully independent Zcash/PIR deployment. No staging voting transactions were
sent by this run.

The released join script registered the secondary but hit a missing install
directory in its local-binary service-install path. Service installation was
completed using its generated configuration and identity. The secondary synced,
reported `catching_up=false`, and appeared in the primary join queue. It was not
promoted to a bonded validator; the primary conducted the round's ceremony.

Round `644d568237b5e78d9df7d113db5a3b499cf0d99bae80cdd606ea01ffde829d12`
completed with three real delegation bundles, six vote commitments, and 96
confirmed helper shares. The old and current SDKs displayed proposal 1 choice 0,
proposal 2 choice 1, and proposal 3 skipped, with the same `voted_at` timestamp.
The run passed the full replay for both schema-13 snapshots, migrating to 24.
Transient transaction-query HTTP errors before block inclusion were retained;
successful chain receipts, rather than initial submission responses, established
transaction confirmation.

Controlled local evidence is retained outside the build directory at
`~/migration-evidence/zcash-voting-v3-to-main-20260913/completed-capture/`.
A working copy also remains under
`target/migration-compat/isolated/completed-capture/`:

| Artifact | SHA-256 |
| --- | --- |
| `accepted.sqlite` | `26ca168faf50547d553d9772dff7c4a7232ace95e1cf9c625f420d120ca9c77d` |
| `confirmed.sqlite` | `284064a6635f308416fd3d83e418d4809fdbac23fc08d75a6b9a653879d8dce5` |
| `manifest.json` | `b4c38f1db114fc5f731c953f768e68c24286415faaf84911a1e6d6781d955f5a` |
| `initial-replay-report.json` (cleanup gate) | `21eabdd7d1338d4b831e6105a683771eb6a002ae8046a805af31e3d09681584f` |
| `replay-report.json` (after cleanup) | `1f1f7123e5b94e9e52e8cdb1a460b33a94821e86407d795118a0a504ede3f462` |

The initial replay identifies main `45d1a039664941bd69112fff51c49d3f3166d9c0` plus this
change through reader SHA-256
`dc38020fd95e845daa843f97d88f13640fb4a1f03053969c394f2d888b075a39`.
The post-cleanup rebuild also passed, with reader SHA-256
`ec586479aeb0d1d652702879ba31a06fdf60aec261c7315c7efb9c8624984e6d`.
The captures and manifest contain private recovery material and are not committed.
Run `make migration-compat-replay FIXTURE_DIR=/absolute/path/to/completed-capture`
to repeat the proof against a future main build. A changed binary naturally has
a different replay-report checksum; source capture checksums must remain stable.

Deployment identity, release hashes, primary/secondary status, and the join queue
are retained beside the durable capture directory and in the working `isolated/`
directory. The full replay passed again after the hosts were deleted, confirming
the history read requires no running vote-chain or helper services. The user authorized deleting the
hosts only after proof. Cleanup verifies the successful two-phase replay and
capture hashes before deleting the two ledgered host IDs, then verifies absence;
`cleanup-report.json` records that outcome and the proof-report hash.
Both hosts (IDs `600048037` and `600048060`) were confirmed absent after deletion;
the dedicated firewall and tag were deleted as well.

## External tally dependency

The SDK sidecar stores personal choices and recovery history. It does not store
aggregate election results or provide their reader here. Viewing a historical
round in a wallet also requires the retained sidecar, stable wallet scope,
historical proposal roster/option labels, and final tally data from the correct
network/round service or a wallet-owned cache. Local completion does not prove a
choice contributed to the final aggregate tally. This suite does not validate
wallet UI behavior or historical service availability.

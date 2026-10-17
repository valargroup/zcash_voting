# v3.1.0 → main SDK migration compatibility

**Passed:** real v3.1.0 voting history survived schema 17 → 24 migration
for both accepted-share and confirmed-share snapshots. Each retained capture
contains three bundles, six votes, 96 share delegations, six durable helper
plans, and the three ballot intents. Choices remain proposal 1 = option 0,
proposal 2 = option 1, and proposal 3 = skipped; both SDKs report
`completed_for_display=true`.

## Scope and reproduction

The v3.1.0 producer is pinned to commit
`7e0ef89126155966f91a1eb6933cdeb48794acdd` and schema 17. Its tracked source,
Cargo.lock, and crypto dependencies remain unchanged. The added host adapter
scans the real testnet wallet, generates proofs, sends transactions, and uses
the release's complete share planning, submission, and confirmation APIs.
The current reader targets schema 24 on main `45d1a039` plus PR #343's
concurrent-opening fix. This does not establish safety of unpatched main.

Use the [capture/replay suite](../migration-compat/README.md) with
`RELEASE_TAG=v3.1.0`. Replay requires no signing credentials. It compares the
same public SDK history reader compiled against both releases, verifies exact
legacy rows and BLOBs, checks reopening and another round, and runs concurrent
opens plus transaction-denial and process-kill fault tests on database copies.
The schema-17 fault boundaries are `chain_submissions`, `round_immediate_share`,
`combined_cast_rejections`, and immediately after commit.

## Compatible isolated deployment

The temporary DigitalOcean network uses the Valargroup `misc` project, region
`nyc1`, a primary `s-4vcpu-16gb-amd` and a secondary `s-4vcpu-8gb-amd`.
Its chain ID is `migration-v3-31-1`. Both hosts run identical binaries; the
secondary joined through direct peering, registered in the primary's queue,
and reached `catching_up=false` with increasing block height. The primary is
the bonded validator for this SDK compatibility test.

This server is an explicitly adapted compatibility build, not an unmodified
published vote-sdk release. Chain source is pinned to
`074e05270b5d59bfdeed000b2a781abb6d5bb6b6`. Its Git overrides for
`voting-circuits` and `voting-crypto-deps` were removed in the isolated checkout
so it resolves the exact published `0.11.2` and `0.2.2` packages in the v3.1.0
SDK lockfile. Registry source/checksum identities, including the Orchard
backend, were compared. The Go binary was rebuilt with `GOFLAGS=-a` to avoid
reusing a cgo artifact linked against a different verifier. Both hosts' `svoted`
SHA-256 is `1a913da4c7d60af3f55a5e02d1a045fb66ac3edcb50f2a54a4b11d914af3a3e9`.
The join script used its explicit local-fork version override; it downloaded no
replacement binary. Source, scripts, lockfile, adaptation and binary hashes are
retained with the private evidence.

Vote-chain transactions and helper submissions target only this isolated
network. Read-only sources are `https://testnet.zec.rocks:443` and
`https://stage.pir.valargroup.org`. This is not a disconnected Zcash deployment.

The first capture failed before delegation while streaming a 2,000-block
lightwalletd range: HTTP/2 reported `too_many_data_frames`. Its incomplete
wallet is retained separately and is not completed-vote evidence. The adapter
now scans in 100-block batches and requires a complete, contiguous response
before advancing the scan cursor. A fresh round and capture were used afterward.

## External tally dependency

The sidecar preserves personal choices and recovery history. Aggregate round
results must come from the correct historical network/round service or an
external cache. Consumers also need the historical proposal roster and option
labels and must retain the sidecar and wallet identity. This suite does not test
a wallet UI, tally retention, delayed scheduling, multiple-helper availability,
or whether a locally recorded choice was counted in the final aggregate.
It cannot provide a universal no-break guarantee.

## Retained evidence and checks

Private evidence is outside the build directory at
`~/migration-evidence/zcash-voting-v3.1-to-main-20260913/`. Do not commit the
wallet, sidecars, recovery material, or HTTP evidence. Original schema-17
captures remain immutable; all migrations operate on copies.

| Artifact | SHA-256 |
| --- | --- |
| `accepted.sqlite` | `f6765dd2b2bddc16c907f5ff08e192fc6fd4dc00437bfe9c24eed676ef17c845` |
| `confirmed.sqlite` | `5059f1d5b49488c1889bdb8fd249540aab1390358157d51355b17bf6f2434fcf` |
| `manifest.json` | `2cb76490efd97744205bed359b337ba0d26b6ed674e1d0a7db80a5c927619b70` |
| Initial replay report | `bae949456553fe6dae70935184dd2292950fbb805100f81d3c0b16c4e5793e1d` |

The live run recorded three successful delegation events, six vote events,
six complete share-delivery reports, and 96 real helper confirmations. Each
retained delegation proof passed the released verifier while a damaged control
was rejected. Both snapshots passed history equality, exact row preservation,
schema equivalence, SQLite integrity/foreign-key checks, six concurrent openers,
reopening, wallet isolation, and history after creating another round. Both
also passed real transaction-denial and process-kill tests with retry.

`make check` and `make test` passed (1,745 tests, 11 skipped). The comparator's
seven tests and deterministic concurrent-opening regression passed. Both
release adapters built against their untouched pinned releases. The retained
real v3.0.0 capture also passed the generalized replay as a regression check.

The temporary primary (`600057785`) and secondary (`600057790`) were deleted
only after both snapshots passed and a checksum-verified evidence copy existed
outside `target/`. DigitalOcean returned 404 for both hosts; the dedicated
firewall and tag were removed. `cleanup-report.json` binds deletion to the
successful pre-deletion replay report. Full replay passed again after deletion
at commit `5dafef2d`. The post-cleanup replay report SHA-256 is
`32a183fd5fe3de2269c02e985102db3c0848d3394456b65a3547bc1eafcba0b0`. Earlier reports are archived by checksum before replacement.

Reproduce that offline check from the repository root:

```sh
make migration-compat-replay RELEASE_TAG=v3.1.0 \
  FIXTURE_DIR="$HOME/migration-evidence/zcash-voting-v3.1-to-main-20260913/completed-capture"
```

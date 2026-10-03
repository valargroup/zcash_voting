# zcash_voting

Client-side cryptographic library for Zcash shielded voting. Implements proof generation, vote construction, and tree synchronization for the [Zally governance protocol](https://github.com/valargroup/shielded-vote-book).

## Workspace Crates

| Crate | Description |
|-------|-------------|
| **zcash_voting** | Core library: ZKP delegation and vote proofs (Halo2), El Gamal encryption, governance PCZT construction, Merkle witness generation, chain confirmation parsing, SQLite round-state persistence |
| **vote-commitment-tree** | Append-only Poseidon Merkle tree for Vote Authority Notes and Vote Commitments |
| **vote-commitment-tree-client** | HTTP client and CLI for syncing the vote commitment tree from a chain node |

## Architecture

```
zcash_voting
├── config ───────────────────── config resolution + switch decisions
├── vote-commitment-tree-client ─ vote-commitment-tree
├── pir-client / vote-nullifier-pir types
├── voting-circuits ───────────── ZK delegation + vote proofs
└── librustzcash crates ───────── pczt, zcash_keys, zcash_client_sqlite, ...
```

The config resolver itself is transport-agnostic. Wallets choose the static
config source and network transport, fetch bytes, and pass those bytes into
`zcash_voting::config`. The `wallet-example::example_config` module shows a
direct HTTPS implementation for Rust consumers that do not need a custom
transport.

## Building

Use the supported Make targets, which keep separate build directories for each
backend:

```bash
make check  # type-check the default Zakura stack
make test   # run the default Zakura test suite
```

Use `make test-lrz` for LRZ/backend-feature changes and `make test-vct` for
changes to the tree crates. Run `make help` for the complete list.

## Releases and Branching

`main` is the development line for the next release. Each shipped release
series is maintained on a `release/vMAJOR.MINOR.x` branch, and semver-compatible
fixes reach those branches through reviewed automated backports rather than
direct pushes. The historical `release/v3.x` branch and the
`release/v4.0.x` and `release/v5.0.x` lines are currently supported.

See [Release branches and backports](docs/release-branches.md) for the backport
labels and the rules for what may ship on a maintenance line, and
[CONTRIBUTING.md](CONTRIBUTING.md) for the build, test, and code standards.

## Wallet API Lifecycle

**Upgrading from v3.x or v4.0.x?** Start with
[Migrating to v5](docs/migrating-to-v5.md) and the
[v5 release notes](CHANGELOG.md#v500). The guide distinguishes the release
lines, maps removed APIs to their replacements, and explains the limits on
upgrading a wallet with in-flight legacy submissions.

New integrations should import `zcash_voting::prelude::*` and use the SDK's
round and share-tracking drivers:

1. Open the wallet's voting sidecar and persist bundle setup through
   `DelegationPipeline::setup_bundles` or the `round` APIs. Bind the round,
   authenticated proposal roster, network, and voting hotkey to a
   `RoundExecutor`.
2. Record choices and explicit skips with `set_ballot_intents`. Supply a
   `RoundHostSource` with fresh timing, authenticated service configuration,
   and delegation signing inputs. Software wallets supply a `SpendAuthSigner`;
   Keystone wallets supply device signatures. Root wallet seeds stay in the
   wallet.
3. Call `RoundDriver::run`. It selects work from durable state, overlaps
   independent bundles, owns chain submission/recovery and initial helper
   delivery, and returns a `RoundRunReport`. Read its stop reason, failures,
   and durable effects before deciding whether to collect input or run again.
4. Run `ShareTrackingDriver` for pending helper shares, including after restart.
   The default foreground plan confirms only the designated immediate share;
   chain confirmation and remaining helper confirmation are separate milestones.

Fresh local delegation and chosen proposals are submitted together through
`delegate-and-cast-vote-batch`, including a single chosen proposal. This path
needs no initial vote-tree sync. Existing standalone and imported delegations
keep their recovery paths; multiple due proposals on an existing delegation
use `cast-vote-batch`. The target chain must support these routes. Atomic
transactions reveal that the included proposal actions were submitted together.

Use `HyperTransport::with_route` over a host `RouteHttp` for Tor or proxies,
and bind routed transports for PIR and tree sync as well as helper and chain
traffic. `PirFleet` supplies endpoint failover. The SDK handles request
encoding, timeout classification, and durable dispatch evidence.

Stage APIs remain available for specialized integrations. `delegate` owns
setup, proof reuse, and signing requests; `vote` owns commitment preparation and
persistence; `ChainSubmissionClient` owns submission and confirmation; and
`ConfirmedVote` owns initial helper submission. Hosts do not parse chain events,
write confirmation columns, or execute `NextStep` values themselves.
`session::resume_plan` remains a read-only projection for UI and recovery status.

See the [migration walkthrough](docs/migrating-to-v5.md#replace-the-host-workflow),
[wallet examples](wallet-example/src),
[delegation signing transaction](docs/delegation-signing-transaction.md),
[capability handoff](docs/exporting-to-external-software.md), and
[optional observability](docs/observability.md).

## Dependency Strategy

The LRZ backend uses one Ironwood dependency stack:

- **`orchard 0.16`** from [zcash/orchard](https://github.com/zcash/orchard),
  with `unstable-voting-circuits` enabled for the governance proof paths.
- **`pczt 0.10.0-pre.0`, `zcash_client_backend 0.25.0-pre.0`,
  `zcash_client_sqlite 0.23.0-pre.0`, `zcash_keys 0.17.0-pre.0`,
  `zcash_primitives 0.31.0-pre.0`, and `zcash_protocol 0.11.0-pre.0`** from
  the librustzcash NU7 pre-releases.
- **`voting-circuits 0.12.2`** from
  [valargroup/voting-circuits](https://github.com/valargroup/voting-circuits)
  for the delegation and vote proof circuits.

`vote-commitment-tree` and `vote-commitment-tree-client` default to Zakura and
select their proving stack through mutually exclusive `zakura`/`lrz` features;
build with `--no-default-features --features lrz` for the LRZ VCT backend.

The published `zcash_voting` crate defaults to Zakura and exposes LRZ through
the mutually exclusive `lrz` feature. Wallet-family selection is consolidated
in published `zakura-wallet-lib 0.1.0-rc5`, whose complete `zakura` and `lrz`
modes never weak-reference both backend families. Gemini selects
`zcash_voting` with `default-features = false, features = ["lrz"]`; Vizor uses
the defaults. External-consumer regression tests verify that Gemini's Cargo
lockfile and resolved metadata contain no Zakura forks.

`Cargo.toml` is the source of truth for version and feature requirements, and
`Cargo.lock` records the exact package sources and versions used by this branch.
Its `[patch.crates-io]` table moves both backends onto the stable `bip32` 0.6
stack until the corresponding releases are published; a consumer must declare
the same table.
The current PIR and IMT releases require Rust 1.91 or newer.

## FFI

Mobile FFI bindings live in [zcash-swift-wallet-sdk](https://github.com/valargroup/zcash-swift-wallet-sdk) (hand-rolled C FFI + Swift wrappers). This repo is a pure Rust workspace.

## License

TODO

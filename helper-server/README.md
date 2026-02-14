# helper-server

Relay server for the Zally voting protocol. Receives encrypted share payloads
from wallets after they cast a vote, delays them for temporal unlinkability,
generates a VC Merkle witness and share nullifier, and submits `MsgRevealShare`
to the vote chain.

This sits between the iOS wallet ([PR #34](https://github.com/z-cale/zally/pull/34))
and the vote chain. The wallet calls `delegateShares(payloads:)` after
`submitVoteCommitment` — those payloads arrive here.

## Architecture

```
Wallet (iOS)                    helper-server                     Vote chain
────────────                    ─────────────                     ──────────
buildSharePayloads()
        │
        ├── POST /api/v1/shares ──► enqueue with random delay
                                    (10–300s, configurable)
                                            │
                                    process loop (every 2s):
                                    ├─ sync tree (vote-commitment-tree-client)
                                    ├─ generate VC Merkle witness
                                    ├─ derive share_nullifier (Poseidon)
                                    ├─ generate ZKP #3 (mocked)
                                    └─ POST /zally/v1/reveal-share ──► verify + accumulate
```

### Modules

| File              | Purpose                                              |
| ----------------- | ---------------------------------------------------- |
| `api.rs`          | HTTP routes: `POST /api/v1/shares`, `GET /api/v1/status` |
| `store.rs`        | In-memory share queue with per-round bucketing, random delay scheduling, retry backoff |
| `tree.rs`         | `TreeSync` wrapper around `vote-commitment-tree-client` with background sync loop |
| `processor.rs`    | Main processing pipeline: witness → nullifier → proof → chain submit |
| `nullifier.rs`    | Poseidon-based share nullifier derivation (Gov Steps V1 §5.3) |
| `chain_client.rs` | HTTP client for `MsgRevealShare` submission           |
| `types.rs`        | Wire format structs (`SharePayload`, `MsgRevealShareJson`) and config |
| `mock_tree.rs`    | In-memory mock tree server with chain-compatible REST endpoints |

## Wire format

`SharePayload` (wallet → helper server) follows the spec in
[`docs/mobile-voting-api.md`](../docs/mobile-voting-api.md). One notable
addition: `vote_round_id` is included so the server can key its queue by round.
The iOS `SharePayload` struct doesn't have this field yet — the client has the
data via `VoteCommitmentBundle.voteRoundId` and will include it when
`buildSharePayloads` is wired to the real network layer.

`MsgRevealShareJson` (helper server → chain) matches the protobuf in `tx.proto`.
The `vote_comm_tree_anchor_height` field is a `u64` height (not a root hash) —
this matches the proto despite the spec doc (`cosmos-sdk-messages-spec.md §4.4`)
saying `vote_comm_tree_root`.

## Crate dependencies

- **`vote-commitment-tree`** — the Poseidon Merkle tree implementation. The
  helper server uses it indirectly through `TreeSync`, which wraps the tree
  client and provides witness generation for VC leaf positions.
- **`vote-commitment-tree-client`** — `HttpTreeSyncApi` for syncing the tree
  over REST. `TreeSync::sync()` calls this to pull new leaves from the chain
  (or mock tree).
- **`imt-tree`** — Poseidon hash used for share nullifier derivation.

## Usage

### Helper server

```
cargo run --bin helper-server -- \
  --port 9090 \
  --tree-node http://localhost:8080 \
  --min-delay 10 \
  --max-delay 300
```

| Flag                | Default                  | Description                                  |
| ------------------- | ------------------------ | -------------------------------------------- |
| `--port`            | `9090`                   | Listen port                                  |
| `--tree-node`       | `http://localhost:8080`  | Chain REST API (or mock tree) URL            |
| `--chain-submit`    | same as `--tree-node`    | Separate URL for `MsgRevealShare` submission |
| `--min-delay`       | `10`                     | Minimum random delay (seconds)               |
| `--max-delay`       | `300`                    | Maximum random delay (seconds)               |
| `--sync-interval`   | `5`                      | Tree sync polling interval (seconds)         |
| `--process-interval`| `2`                      | Share processing loop interval (seconds)     |

Logging is controlled via `RUST_LOG`:
```
RUST_LOG=helper_server=debug cargo run --bin helper-server
```

### Mock tree (development)

Wraps an in-memory tree behind the same REST API as the real chain, so
`TreeSync`, the helper server, and the iOS client can all sync against it:

```
cargo run --bin mock-tree -- --port 8080
```

Admin endpoints for inserting test leaves:
- `POST /admin/append` — `{ "leaves": ["<base64>", ...] }`
- `GET /admin/status` — tree size and height

Chain-compatible query endpoints:
- `GET /zally/v1/commitment-tree/latest` — current tree state
- `GET /zally/v1/commitment-tree/leaves?from_height=N&to_height=M` — leaf batches
- `POST /zally/v1/reveal-share` — mock chain submission (returns `{ tx_hash, code, log }`)

## Tests

```
cargo test -p helper-server
```

Integration tests exercise the full stack without a real chain:
- Mock tree insert/query and `TreeSync` witness verification
- Share intake validation (field sizes, base64, hex, range checks)
- End-to-end pipeline: insert leaves → sync tree → enqueue share → take ready → witness → submit `MsgRevealShare`

## What's mocked

- **ZKP #3 proof** — `process_share` emits 192 zero bytes. Will be replaced
  when the Halo2 share-reveal circuit exists.
- **Vote commitment reconstruction** — uses `shares_hash` as a proxy for the
  VC leaf value when deriving the nullifier. In production, the VC will come
  from the circuit's public inputs.

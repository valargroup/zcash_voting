# Handoff: Wire Zashi to Helper Server

## Goal

Get Zashi (iOS app) submitting shares through the helper server instead of directly to the chain, completing the production voting flow end-to-end.

## Current state

### What works (validated)

The full voting pipeline is validated by an integration test (`voting_flow_librustvoting_path`, CI green on PR #80):

```
Zashi / E2E Test              Chain (1318)           Helper Server (9091)
────────────────              ────────────           ──────────────────
discover round ──────────────► GET /rounds/active
delegate-vote (ZKP#1) ──────► POST /delegate-vote
                               ◄── tree updated (VAN leaf)
sync tree ───────────────────► GET /commitment-tree/leaves
build VAN witness
build vote commitment (ZKP#2)
sign cast-vote
cast-vote ───────────────────► POST /cast-vote
                               ◄── tree updated (+2 leaves)
build share payloads
POST 4 shares ─────────────────────────────────────► POST /api/v1/shares
                                                     │ sync tree from chain
                                                     │ generate VC witness
                                                     │ derive share nullifier
                                                     │ generate ZKP #3 (30-60s)
                               ◄── POST /reveal-share┘
                               (×4 shares, with delay)
auto-tally via PrepareProposal
```

**Rust layer**: Complete. `librustvoting` handles delegation, ZKP #2, cast-vote signing, and share payload construction. The FFI (`zcash-voting-ffi`) exposes all of this to Swift.

**Zashi iOS**: Full pipeline wired — delegation (Keystone and non-Keystone), vote commitment, and share submission to the helper server.

**Helper server**: Complete. Accepts shares, generates real ZKP #3 (Halo2 circuit from PR #71), handles temporal delay for unlinkability, submits `MsgRevealShare` to chain.

### What's been fixed

1. **Non-Keystone delegation** — PR #77 fixed the wiring gap: `VotingStore` now calls `startDelegationProof` for software wallets
2. **Real ZKP #3** — PR #71 added the Halo2 share reveal circuit to the helper server (no more mock proofs)
3. **Helper server wiring** — `delegateShares` now POSTs to the helper server (`/api/v1/shares`) with the correct wire format including `allEncShares`, `shares_hash`, `tree_position`, and hex `vote_round_id`. The helper server generates ZKP #3 and submits reveal-share TXs to the chain.

## What was changed

All tasks from the original handoff are now complete:

- **Task 1** (allEncShares): `SharePayload` in `VotingModels.swift` now carries `allEncShares: [EncryptedShare]`, mapped through from FFI in `VotingCryptoClientLiveKey.swift`
- **Task 2** (helper server URL): `ZallyAPIConfig.helperServerURL` added (`http://localhost:9091`), with a dedicated `postHelperJSON` helper
- **Task 3** (delegateShares rewrite): `VotingAPIClientLiveKey.swift` now POSTs to helper server `/api/v1/shares` with correct wire format (hex round ID, all 4 enc shares, no proof/nullifier/anchor — helper server handles those). `anchorHeight` removed from the `delegateShares` signature.
- **Task 4** (non-Keystone delegation): Fixed by PR #77

The Rust layer already handles software-wallet signing — this is purely a Swift wiring gap.

## Reference: helper server wire format

The helper server expects this JSON at `POST /api/v1/shares`:

```json
{
    "shares_hash": "<base64, 32 bytes>",
    "proposal_id": 1,
    "vote_decision": 1,
    "enc_share": {
        "c1": "<base64, 32 bytes>",
        "c2": "<base64, 32 bytes>",
        "share_index": 0
    },
    "tree_position": 2,
    "vote_round_id": "<hex, 64 chars>",
    "all_enc_shares": [
        {"c1": "<base64>", "c2": "<base64>", "share_index": 0},
        {"c1": "<base64>", "c2": "<base64>", "share_index": 1},
        {"c1": "<base64>", "c2": "<base64>", "share_index": 2},
        {"c1": "<base64>", "c2": "<base64>", "share_index": 3}
    ]
}
```

Response: `{"status": "queued"}` (200 OK)

The reference implementation is in `e2e-tests/src/payloads.rs:helper_share_payload()`.

## Key files

| File | Role |
|------|------|
| **Zashi iOS** | |
| `VotingStore.swift` | Orchestrates full flow: init → delegation → vote → shares |
| `VotingAPIClientLiveKey.swift` | REST calls to chain + helper server — **main file to change** |
| `VotingAPIClientInterface.swift` | API client protocol |
| `VotingCryptoClientLiveKey.swift` | FFI calls to librustvoting — needs `allEncShares` mapping |
| `VotingModels.swift` | Swift types — needs `allEncShares` field on `SharePayload` |
| **Rust / FFI** | |
| `zcash-voting-ffi/rust/src/lib.rs` | FFI boundary (SharePayload already has `all_enc_shares`) |
| `librustvoting/src/storage/operations.rs` | Core Rust logic for share payloads |
| **Reference** | |
| `e2e-tests/tests/voting_flow_librustvoting.rs` | Canonical e2e test (steps 9-10 show share flow) |
| `e2e-tests/src/payloads.rs` | `helper_share_payload()` — exact wire format |
| `helper-server/src/types.rs` | `SharePayload` struct the server deserializes |

## Running locally

```bash
# Terminal 1: Chain
cd sdk && make init && make start

# Terminal 2: Helper server
cd helper-server && cargo run --release --bin helper-server -- \
  --tree-node http://127.0.0.1:1318 \
  --chain-submit http://127.0.0.1:1318 \
  --min-delay 1 --max-delay 3 \
  --db-path :memory:

# Terminal 3: E2E test (validates the full flow)
cargo test --release --manifest-path e2e-tests/Cargo.toml \
  voting_flow_librustvoting_path -- --nocapture --ignored
```

## Remaining items (not in scope for this task)

| Item | Priority | Details |
|------|----------|---------|
| Keystone spendAuthSig not persisted | Medium | `VotingStore.swift:693` — sig extracted but not stored in DB |
| IMT server URL hardcoded | Medium | `VotingStore.swift:570,713` — `http://46.101.255.48:3000` |
| ZKP #2 Condition 5 disabled | Low | Range-check layout conflict in `orchard/src/vote_proof/` |
| VC tree not persisted to SQLite | Low | In-memory only; app restart re-downloads all leaves |

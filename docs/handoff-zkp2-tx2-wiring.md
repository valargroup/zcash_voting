# Handoff: `greg/zkp2-tx2-wiring` (PR #72)

Branch: `greg/zkp2-tx2-wiring` — 22 commits ahead of `main`

## What this branch does

Wires the full voting flow end-to-end from Rust through FFI into the Zashi iOS app. Two major work streams, now complete:

**ZKP #2 wiring**: Replaced the mock ZKP #2 stub with the real Halo2 prover. Added DB persistence for delegation data needed by ZKP #2, vote commitment tree sync via HTTP, VAN witness generation, real share payload construction with Poseidon-hashed `shares_hash`. Verified by a librustvoting e2e test (`voting_flow_librustvoting.rs`).

**Delegation submission + cast-vote signing**: Added `get_delegation_submission` (reconstructs chain-ready delegation payload from DB + seed), `sign_cast_vote` (domain-separated Blake2b sighash + spend auth signature). Wired both through FFI into VotingStore. Replaced all VotingAPIClient mocks with real REST calls to the Zally chain.

**Final wiring**: Round discovery via `GET /zally/v1/rounds/active`, commitment tree polling after TX submission (avoids position race conditions), aligned iOS with PR #53's `gov_comm` -> `van_cmx` proto rename.

## Current state

**Builds**: Zashi compiles cleanly (zero errors, zero new warnings).

**Keystone path**: Wired through delegation proof generation, Keystone QR signing flow, delegation TX submission, VAN position polling, vote commitment (ZKP #2), cast-vote signing + submission, and share payload construction. Should work end-to-end against a live chain, though the share reveal step currently bypasses the helper server (see remaining work).

**Non-Keystone (software wallet) path**: **Not working end-to-end.** The delegation pipeline is skipped entirely — `witnessVerificationCompleted` and `delegationApproved` set `delegationProofStatus = .complete` without running `buildAndProveDelegation`, submitting the delegation TX, or storing the VAN position. When the user tries to vote, `syncVoteTree` / `generateVanWitness` will fail because no VAN leaf exists on chain. The Rust side fully supports software-wallet signing (derives `rsk` from seed), so this is purely a Swift wiring gap.

## Remaining work

### High priority

| Item | Location | Details |
|------|----------|---------|
| Wire non-Keystone delegation | `VotingStore.swift` ~lines 466, 527 | `witnessVerificationCompleted` and `delegationApproved` skip the entire delegation pipeline for `!isKeystoneUser`. Need to call `startDelegationProof` (or equivalent) so the non-Keystone path runs `buildAndProveDelegation`, submits the delegation TX, stores VAN position, then proceeds to proposal list. The Rust layer already handles software-wallet signing. |
| Wire `delegateShares` to helper server | `VotingAPIClientLiveKey.swift:264-292` | Currently posts directly to `/zally/v1/reveal-share` with a mock proof. In the real architecture, the mobile app sends share payloads to the **helper server**, which handles ZKP #3 generation, temporal delay for unlinkability, VC tree witness, and chain submission. |
| Keystone spendAuthSig not persisted | `VotingStore.swift:693` | The sig is extracted from the Keystone-signed PCZT but not stored in DB. Needed for on-chain delegation submission in the Keystone flow. |

### Medium priority

| Item | Location | Details |
|------|----------|---------|
| IMT server URL hardcoded | `VotingStore.swift:570,713` | Nullifier IMT server is `http://46.101.255.48:3000`. Should come from VotingSession or server config. |
| `ZallyAPIConfig.baseURL` hardcoded | `VotingAPIClientLiveKey.swift:10` | Defaults to `http://localhost:1317`. Needs app config or server discovery for production. |
| ZKP #2 Condition 5 disabled | `orchard/src/vote_proof/` | Proposal Authority Decrement condition disabled due to range-check layout conflict. Separate circuit work. |

### Low priority

| Item | Location | Details |
|------|----------|---------|
| VC tree not persisted to SQLite | `librustvoting` `VotingDatabase` | `TreeClient` is in-memory (`Mutex<Option<TreeClient>>`). App restart re-downloads all leaves. Should implement `ShardStore` backed by SQLite (same pattern as `zcash_client_sqlite`). |
| `enc_memo` is mock | `librustvoting/src/storage/operations.rs` | `[0x05; 64]` placeholder. Matches e2e test but needs real memo content. |

## Key files

| File | Role |
|------|------|
| `VotingStore.swift` | Orchestrates the full flow: init -> witnesses -> delegation proof -> vote -> shares |
| `VotingAPIClientLiveKey.swift` | All REST calls to the Zally chain (and eventually helper server) |
| `VotingAPIClientInterface.swift` | API client protocol with `awaitCommitmentTreeGrowth` polling helper |
| `VotingCryptoClientLiveKey.swift` | All FFI calls to librustvoting |
| `VotingModels.swift` | Shared Swift types (`DelegationRegistration`, `VoteCommitmentBundle`, `CastVoteSignature`, etc.) |
| `librustvoting/src/storage/operations.rs` | Core Rust logic: delegation proof, vote commitment, share payloads |
| `librustvoting/src/vote_commitment.rs` | ZKP #2 builder + `sign_cast_vote` |
| `zcash-voting-ffi/rust/src/lib.rs` | FFI boundary between Rust and Swift |

## Architecture notes

- **ZKP #3 runs on the helper server**, not the mobile client. The app sends encrypted share payloads; the helper server adds temporal delay, gathers a VC Merkle witness, generates ZKP #3, and submits `MsgRevealShare` to chain.
- **Commitment tree polling**: After submitting delegation or cast-vote TXs, the app polls `GET /zally/v1/commitment-tree/latest` every 1s (30s timeout) until `nextIndex` grows, then reads the new leaf position. This avoids the race condition where the TX hasn't landed in a block yet.
- **Sighash schemes**: Delegation uses `Blake2b-256("ZALLY_DELEGATION_SIGHASH_V0")`. Cast-vote uses `Blake2b-256("ZALLY_CAST_VOTE_SIGHASH_V0")` binding round_id, r_vpk, van_nullifier, vote_authority_note_new, vote_commitment, proposal_id, anchor_height.
- **PR #53 alignment**: Proto field `gov_comm` was renamed to `van_cmx`. iOS models updated: `DelegationRegistration.govComm` -> `.vanCmx`, `DelegationAction.govCommRand` -> `.vanCommRand`, JSON key `"gov_comm"` -> `"van_cmx"`.

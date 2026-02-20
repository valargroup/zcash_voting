# EA Key Ceremony

The Election Authority (EA) key ceremony must run once on a live chain before any votes can be created. Its purpose is to distribute the EA secret key to all validators so that each node can independently decrypt vote tallies via `PrepareProposal`.

## Ceremony state machine

```
REGISTERING  →  DEALT  →  CONFIRMED
     ↑              |
     └──────────────┘  (timeout with < 2/3 acks: full reset)
```

- **REGISTERING** — validators submit their Pallas public keys. No timeout; persists until a deal is submitted.
- **DEALT** — the dealer has published `ea_pk` and one ECIES-encrypted `ea_sk` share per validator. Each validator's node must decrypt its share and auto-ack within 30 seconds (`phase_timeout`). If fewer than 2/3 ack in time, the state resets to REGISTERING and the ceremony must be restarted.
- **CONFIRMED** — all validators have acknowledged. The chain is ready for voting.

## Roles

- **Every validator** — registers their Pallas public key in Phase 1.
- **Dealer** — one designated validator (typically genesis) who runs Phase 2 once all registrations are on-chain. The dealer holds `ea.sk` and `ea.pk`.

## Phase 1 — Register Pallas keys (every validator)

Each validator submits their Pallas public key (`~/.zallyd/pallas.pk`). This key is used in Phase 2 to ECIES-encrypt that validator's copy of `ea_sk`.

```bash
./ceremony.sh register
```

Check how many validators have registered at any time:

```bash
./ceremony.sh status
```

## Phase 2 — Deal the EA key (dealer only)

Once all validators have registered, the dealer runs:

```bash
./ceremony.sh run
```

This is fully automated: it registers the dealer's own key, waits until that registration is confirmed on-chain, encrypts `ea_sk` for every registered validator, submits the deal transaction (failing immediately on a non-zero response code), then polls until the ceremony reaches `CONFIRMED`.

> **Note:** all non-dealer validators must have already run `./ceremony.sh register` before the dealer runs `./ceremony.sh run`. The dealer deals to exactly the set of validators registered at the moment `deal` is submitted — latecomers require a reset.

If you want to run the deal step separately (e.g. you registered manually first):

```bash
./ceremony.sh deal   # encrypt + broadcast
./ceremony.sh wait   # poll until CONFIRMED
```

`deal` internally:
1. Queries all registered Pallas keys and ECIES-encrypts `ea_sk` for each, writing payloads to `/tmp/payloads.json`.
2. Submits a `deal-ea-key` transaction containing `ea_pk` and the encrypted payloads, transitioning `REGISTERING → DEALT` and starting the 30-second ack window.

## Phase 3 — Auto-ack (automatic, every validator)

No manual action required. On every block while the ceremony is `DEALT`, each node's `PrepareProposal` handler:

1. Decrypts its payload from the chain using `~/.zallyd/pallas.sk`
2. Verifies that `ea_sk * G == ea_pk` (integrity check)
3. Injects a `MsgAckExecutiveAuthorityKey` into the block it proposes
4. Writes `ea_sk` to `~/.zallyd/ea.sk` for use by the auto-tally system

When all validators have acked, the ceremony transitions `DEALT → CONFIRMED`.

**Prerequisite:** `vote.pallas_sk_path` must be set in `~/.zallyd/config/app.toml` (done automatically by `make init`):

```bash
grep pallas_sk_path ~/.zallyd/config/app.toml
# vote.pallas_sk_path = "/root/.zallyd/pallas.sk"
```

## Verify completion

```bash
./ceremony.sh status
```

The state field should read `3` (CONFIRMED).

## Resetting the ceremony

To return the ceremony to `REGISTERING` (all validators must re-register):

```bash
./ceremony.sh reset
```

## Environment overrides

All defaults can be overridden via environment variables:

| Variable | Default | Description |
|---|---|---|
| `ZALLY_HOME` | `~/.zallyd` | Node home directory |
| `ZALLY_CHAIN_ID` | `zvote-1` | Chain ID |
| `ZALLY_NODE_RPC` | `tcp://localhost:26657` | Tendermint RPC endpoint |
| `ZALLY_REST_API` | `http://localhost:1318` | REST API base URL |
| `ZALLY_FROM` | `validator` | Key name for signing |
| `ZALLY_KEYRING` | `test` | Keyring backend |
| `ZALLY_EA_SK` | `$ZALLY_HOME/ea.sk` | Path to EA secret key |
| `ZALLY_EA_PK` | `$ZALLY_HOME/ea.pk` | Path to EA public key |
| `ZALLY_PAYLOADS` | `/tmp/payloads.json` | Path for encrypted payloads |

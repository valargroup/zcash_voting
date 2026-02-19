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

## Phase 1 — Register Pallas keys

Each validator submits their Pallas public key (`~/.zallyd/pallas.pk`). This key is used in Phase 2 to ECIES-encrypt that validator's copy of `ea_sk`.

```bash
zallyd tx vote register-pallas-key \
  --from validator \
  --keyring-backend test \
  --home ~/.zallyd \
  --chain-id zvote-1 \
  --node tcp://localhost:26657 \
  --yes
```

Wait ~6 seconds for the block to commit, then confirm registration:

```bash
curl -s http://localhost:1318/zally/v1/ceremony | jq '.ceremony.validators | length'
# Should print 1 (or however many validators have registered)
```

## Phase 2 — Deal the EA key

Once all validators have registered, the genesis validator (dealer) encrypts `ea_sk` for each registered validator and broadcasts the deal.

**2a. Produce `payloads.json`** — queries the chain for all registered Pallas keys and ECIES-encrypts `ea_sk` for each:

```bash
zallyd encrypt-ea-key ~/.zallyd/ea.sk \
  --node http://localhost:1318 \
  --output /tmp/payloads.json
```

Each entry in `payloads.json` contains the recipient's `validator_address`, an `ephemeral_pk` (random per-recipient Pallas point), and a `ciphertext` (32-byte `ea_sk` + 16-byte Poly1305 tag, encrypted with ChaCha20-Poly1305 using an ECDH-derived key).

**2b. Submit the deal** — publishes `ea_pk` on-chain alongside the encrypted payloads. Transitions `REGISTERING → DEALT` and starts the 30-second ack window:

```bash
EA_PK_HEX=$(xxd -p ~/.zallyd/ea.pk | tr -d '\n')

zallyd tx vote deal-ea-key "$EA_PK_HEX" /tmp/payloads.json \
  --from validator \
  --keyring-backend test \
  --chain-id zvote-1 \
  --node tcp://localhost:26657 \
  --yes
```

## Phase 3 — Auto-ack (automatic)

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
zallyd q vote ceremony-state
# status: CEREMONY_STATUS_CONFIRMED
```

## Resetting the ceremony

By resetting the ceremony, we are back to the state where more validators can join.

```
zallyd tx vote reinitialize-election-authority \
  --from validator \
  --keyring-backend test \
  --chain-id zvote-1 \
  --node tcp://localhost:26657 \
  --yes
```

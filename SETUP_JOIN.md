# Sync a New Node and Create a Validator

## Current Chain State

The genesis validator is already running:

- Chain ID: `zvote-1`
- P2P: `0.0.0.0:26656` (externally accessible)
- RPC: `127.0.0.1:26657` (local only — see note in Step 6)
- REST API: port `1318`
- Home: `~/.zallyd`
- Binary: `/root/go/bin/zallyd` (not in `$PATH` — prefix commands with full path or add to PATH)

## Step 0 — Prerequisites on the New Node

The `zallyd` binary must be built with the `halo2` and `redpallas` FFI tags. Clone the repo and install:

```bash
export GOPATH=$HOME/go
export PATH=$PATH:$GOPATH/bin

git clone https://github.com/z-cale/zally
cd zally/sdk
make install-ffi   # builds with -tags "halo2,redpallas"
```

This places `zallyd` at `~/go/bin/zallyd`.

## Step 1 — Initialize the Node

```bash
NEW_HOME=~/.zallyd-new
MONIKER=my-validator    # choose a name

zallyd init $MONIKER --chain-id zvote-1 --home $NEW_HOME
```

## Step 2 — Copy genesis.json from the Genesis Validator

From **this host** (or expose the file via HTTP/SCP from the genesis node):

```bash
# On the genesis node, the genesis file is at:
cat ~/.zallyd/config/genesis.json

# On the new node, copy it:
scp root@164.92.137.124:~/.zallyd/config/genesis.json $NEW_HOME/config/genesis.json
```

## Step 3 — Generate Cryptographic Keys

```bash
# Cosmos account key (for signing transactions)
zallyd keys add validator --keyring-backend test --home $NEW_HOME

# Save the new validator's account address
NEW_VAL_ADDR=$(zallyd keys show validator -a --keyring-backend test --home $NEW_HOME)
echo "New validator address: $NEW_VAL_ADDR"

# Pallas keypair (required for ceremony registration)
zallyd pallas-keygen --home $NEW_HOME

# EA keypair (required for PrepareProposal auto-ack/tally)
zallyd ea-keygen --home $NEW_HOME
```

## Step 4 — Configure config.toml

Edit `$NEW_HOME/config/config.toml`:

```bash
# Set persistent peer to the genesis validator
sed -i 's|persistent_peers = ""|persistent_peers = "7f186559fb472f9c414ca34ee3e7dfa8d530f6f6@164.92.137.124:26656"|' $NEW_HOME/config/config.toml

# Increase broadcast timeout for ZKP verification
sed -i 's/^timeout_broadcast_tx_commit = .*/timeout_broadcast_tx_commit = "120s"/' $NEW_HOME/config/config.toml
```

If running on the same host as the genesis validator, also offset the ports to avoid conflicts (using the multi-validator convention):

```bash
# Example for a 2nd validator on the same host:
sed -i 's|laddr = "tcp://0.0.0.0:26656"|laddr = "tcp://0.0.0.0:26256"|' $NEW_HOME/config/config.toml
sed -i 's|laddr = "tcp://127.0.0.1:26657"|laddr = "tcp://127.0.0.1:26257"|' $NEW_HOME/config/config.toml
sed -i 's/addr_book_strict = true/addr_book_strict = false/' $NEW_HOME/config/config.toml
sed -i 's/allow_duplicate_ip = false/allow_duplicate_ip = true/' $NEW_HOME/config/config.toml
```

## Step 5 — Configure app.toml

Append the vote module config to `$NEW_HOME/config/app.toml`:

```bash
cat >> $NEW_HOME/config/app.toml <<EOF

[vote]
ea_sk_path = "$NEW_HOME/ea.sk"
pallas_sk_path = "$NEW_HOME/pallas.sk"
comet_rpc = "http://localhost:26257"   # adjust to the new node's RPC port
EOF
```

Also enable the REST API if needed:

```bash
sed -i '/\[api\]/,/\[.*\]/ s/enable = false/enable = true/' $NEW_HOME/config/app.toml
sed -i 's|address = "tcp://localhost:1317"|address = "tcp://0.0.0.0:1518"|' $NEW_HOME/config/app.toml
```

## Step 6 — Fund the New Validator Account

The new account must be funded before it can create a validator. From the **genesis validator** (on this host), send stake:

```bash
export PATH=$PATH:/root/go/bin

zallyd tx bank send validator $NEW_VAL_ADDR 20000000stake \
  --keyring-backend test \
  --chain-id zvote-1 \
  --home ~/.zallyd \
  --node tcp://127.0.0.1:26657 \
  --yes
```

> Note: the genesis node's RPC is bound to `127.0.0.1:26657` (localhost only). The `bank send` command must be run on the genesis node itself, not remotely.

## Step 7 — Start the New Node and Wait for Sync

```bash
zallyd start --home $NEW_HOME > $NEW_HOME/node.log 2>&1 &

# Monitor sync status
watch -n2 'zallyd status --home $NEW_HOME 2>/dev/null | python3 -c "import sys,json; s=json.load(sys.stdin)[\"sync_info\"]; print(\"catching_up:\", s[\"catching_up\"], \"height:\", s[\"latest_block_height\"])"'
```

Wait until `catching_up: False` before proceeding.

## Step 8 — Register as Validator via CreateValidatorWithPallasKey

Use the `create-val-tx` helper tool (located in `sdk/scripts/create-val-tx`). From the repo:

```bash
cd /root/zally/sdk    # or wherever the repo is on the new node

go run ./scripts/create-val-tx --moniker $MONIKER --amount 10000000stake --rpc-url tcp://localhost:26657
```

This will:

1. Read `priv_validator_key.json` from the new home for the consensus pubkey
2. Read `pallas.pk` from the new home
3. Build, sign, and broadcast `MsgCreateValidatorWithPallasKey` to the chain

## Step 9 — Verify

```bash
# Check the new validator appears in the validator set
zallyd query staking validators \
  --node tcp://localhost:26257 \
  --output json | python3 -c "import sys,json; [print(v['description']['moniker'], v['status']) for v in json.load(sys.stdin)['validators']]"
```

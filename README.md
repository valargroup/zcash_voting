# Zally

Zally is a shielded voting protocol built on a Cosmos SDK chain. Votes are zero-knowledge proofs (Halo2 circuits + RedPallas signatures) that prove eligibility without revealing the voter's identity. Orchard nullifiers are ingested from the Zcash chain and used to generate non-inclusion proofs that prevent double-voting.

## Infrastructure Setup

| Guide | Purpose |
|---|---|
| [SETUP_GENESIS.md](SETUP_GENESIS.md) | Bootstrap the genesis validator — build the binary, initialise the chain, open P2P, and record the node identity |
| [SETUP_JOIN.md](SETUP_JOIN.md) | Add a new validator — sync a post-genesis node, fund it, and submit `MsgCreateValidatorWithPallasKey` |
| [SETUP_CEREMONY.md](SETUP_CEREMONY.md) | Run the EA key ceremony — register Pallas keys, deal the encrypted EA secret key to all validators, and confirm |

## Architecture

| Component | Language | Description |
|---|---|---|
| `sdk/` | Go + Rust (CGo) | Cosmos SDK chain (`zallyd`) with vote module, ante handlers, and ZK verification |
| `nullifier-ingest/` | Rust | Ingests Orchard nullifiers from a lightwallet server into flat binary files and serves exclusion proofs |
| `shielded_vote_generator_ui/` | TypeScript / React | UI for constructing and submitting shielded votes |
| `zcash-voting-ffi/` | Rust + Swift | iOS FFI bindings for the voting circuits |
| `e2e-tests/` | Rust | End-to-end API tests against a running chain |

## Prerequisites

- **Go 1.24.1+** — [https://go.dev/dl/](https://go.dev/dl/) (Go 1.24.0 has a known incompatibility with `github.com/bytedance/sonic/loader` — use 1.24.1 or later); ensure `$GOPATH/bin` is on your `PATH`:
  ```sh
  export PATH=$PATH:$HOME/go/bin
  ```
- **Rust 1.93.1 / Cargo 1.93.1** (stable) — [https://rustup.rs/](https://rustup.rs/) — install via:
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **make** and **C toolchain** (`gcc`, `cc`) — install via:
  ```sh
  apt install build-essential
  ```

## Setup

### Quick start (single command)

```sh
make up
```

This runs the full setup sequence:

1. **`make init`** — builds the Rust circuits, installs the `zallyd` binary with real Halo2 + RedPallas verification, and initialises a single-validator chain (wipes existing chain data).
2. **`make ingest`** — incrementally fetches Orchard nullifiers from `https://zec.rocks:443` into local flat binary files (`nullifiers.bin`, `nullifiers.checkpoint`, `nullifiers.tree`).
3. **`make ingest-serve`** and **`make start`** run in parallel — the nullifier exclusion proof query server (default port `3000`) and the chain node start concurrently. Both processes are detached and their output is written to `ingest-serve.log` and `zallyd.log`.

`make up` will refuse to start if `zallyd`, `query-server`, or `ingest-nfs` are already running. Use `make down` first to stop them.

### Stopping the stack

```sh
make down
```

Kills any running `zallyd`, `query-server`, and `ingest-nfs` processes and prints a confirmation of what was stopped.

### Checking status

```sh
make status
```

Prints a combined status summary:

- **Chain Node** — moniker, current block height, syncing flag, and latest block time. Shows `(node not running)` if `zallyd` is not up.
- **Nullifier Ingest** — nullifier count, last synced height, and sizes of the data files.

### Step-by-step

```sh
# 1. Install the chain binary (with ZK verification)
make init

# 2. Start the chain
make start

# 3. Ingest nullifiers (separate terminal; incremental, safe to re-run)
make ingest

# 4. Start the nullifier query server (separate terminal)
make ingest-serve
```

### Configuration

The nullifier ingest targets accept environment variable overrides:

| Variable | Default | Description |
|---|---|---|
| `LWD_URL` | `https://zec.rocks:443` | Lightwallet server to fetch nullifiers from |
| `DATA_DIR` | `nullifier-ingest/service` | Directory for `nullifiers.bin` / `.checkpoint` / `.tree` |
| `PORT` | `3000` | Port for the exclusion proof query server |

Example:

```sh
LWD_URL=https://my-node:443 DATA_DIR=/data make ingest
```

# Zally

Zally is a shielded voting protocol built on a Cosmos SDK chain. Votes are zero-knowledge proofs (Halo2 circuits + RedPallas signatures) that prove eligibility without revealing the voter's identity. Orchard nullifiers are ingested from the Zcash chain and used to generate non-inclusion proofs that prevent double-voting.

## Architecture

| Component | Language | Description |
|---|---|---|
| `sdk/` | Go + Rust (CGo) | Cosmos SDK chain (`zallyd`) with vote module, ante handlers, and ZK verification |
| `nullifier-ingest/` | Rust | Ingests Orchard nullifiers from a lightwallet server into flat binary files and serves exclusion proofs |
| `shielded_vote_generator_ui/` | TypeScript / React | UI for constructing and submitting shielded votes |
| `zcash-voting-ffi/` | Rust + Swift | iOS FFI bindings for the voting circuits |
| `e2e-tests/` | Rust | End-to-end API tests against a running chain |

## Prerequisites

- **Go 1.24+** — [https://go.dev/dl/](https://go.dev/dl/) (required by `sdk/go.mod`; `mise.toml` pins `1.23` — run `mise install` if using [mise](https://mise.jdx.dev/))
- **Rust 1.83+ / Cargo** (stable) — [https://rustup.rs/](https://rustup.rs/) — install via:
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

## Setup

### Quick start (single command)

```sh
make up
```

This runs the full setup sequence:

1. **`make init`** — builds the Rust circuits, installs the `zallyd` binary with real Halo2 + RedPallas verification, and initialises a single-validator chain (wipes existing chain data).
2. **`make ingest`** — incrementally fetches Orchard nullifiers from `https://zec.rocks:443` into local flat binary files (`nullifiers.bin`, `nullifiers.checkpoint`, `nullifiers.tree`).
3. **`make ingest-serve`** and **`make start`** run in parallel — the nullifier exclusion proof query server (default port `3000`) and the chain node start concurrently.

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

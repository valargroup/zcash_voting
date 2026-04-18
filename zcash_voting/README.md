# zcash_voting

Client-side library for integrating [Zcash shielded voting](https://github.com/valargroup/vote-sdk) into a wallet. Wraps the Halo 2 ZKPs, hotkey derivation, share construction, and governance-PCZT assembly that a wallet needs to participate in an on-chain voting round.

## Usage

Wallets typically consume this through a language bridge:

- **Rust wallets**: add `zcash_voting = "0.1"` to `Cargo.toml`.
- **iOS wallets**: depend on `valargroup/zcash-swift-wallet-sdk` branch `shielded-vote`, which bundles this crate.

See the [wallet integration guide](https://github.com/valargroup/vote-sdk/blob/main/docs/wallet-integration.md) for the full flow.

## Crate layout

| Crate | Purpose |
|---|---|
| **`zcash_voting`** (this crate) | Top-level API: proof generation, hotkey derivation, share construction, PCZT assembly, round-state storage. |
| [`vote-commitment-tree`](../vote-commitment-tree) | Append-only Poseidon Merkle tree for VANs and vote commitments. |
| [`vote-commitment-tree-client`](../vote-commitment-tree-client) | HTTP client + CLI for syncing the vote commitment tree from a running chain node. |

## Dependency notes

`zcash_voting` depends on two Valar Group maintenance forks published on crates.io under `valar-*` names:

- **`valar-orchard 0.11`** — fork of [`orchard`](https://github.com/zcash/orchard) adding governance-visibility methods needed by the voting circuits.
- **`valar-pczt 0.5`** — fork of [`pczt`](https://github.com/zcash/librustzcash) adding shielded-voting getters.

Both are consumed via cargo's `package = "valar-*"` rename trick so consumer code writes `use orchard::…` and `use pczt::…` unchanged. These forks will be dropped once the changes land in upstream ECC releases.

## License

Dual-licensed under MIT or Apache-2.0. See [LICENSE-MIT](../LICENSE-MIT) and [LICENSE-APACHE](../LICENSE-APACHE).

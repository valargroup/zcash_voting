# Migrating from 5.3 to 5.4

Version 5.4 keeps the 5.3 voting lifecycle, wire formats, vote-tree encoding,
voting-sidecar schema, verification keys, and Rust 1.97 minimum. It moves the
default backend from Zakura Common 4.0.0 to Common 5.4.0 and the wallet facade
from `zakura-wallet-lib 0.1.0-rc9.6` to `0.1.0-rc9.8`. The `lrz` backend is
unchanged. Read the section for your backend.

## Zakura backend

Default-backend public types, including notes, addresses, witnesses, and keys,
now belong to Common 5. They cannot be mixed with Common 4 types, so move these
dependencies together in one change:

| Dependency | Version |
| --- | --- |
| zcash_voting | 5.4.0 |
| Zakura Common crates (`zakura-protocol`, `zakura-orchard`, ...) | 5.4.0 |
| voting-circuits | 0.15.0 |
| voting-crypto-deps | 0.5.0 |
| vote-commitment-tree | 0.9.0 |
| vote-commitment-tree-client | 0.11.0 |
| imt-tree | 0.8.0 |
| pir-types | 0.9.0 |
| pir-client | 0.10.0 |
| zakura-wallet-lib | 0.1.0-rc9.8 |
| zakura-client-backend | 0.1.0-rc9.8 |
| zakura-client-sqlite | 0.1.0-rc9.8 |
| zakura-pczt | 0.1.0-rc9.1 |

Move direct dependencies and any `[patch.crates-io]` entries that point at a
Common or wallet-libraries commit in the same change. Do not use the wallet
crates' `0.1.0-rc9.7` releases. They cannot be built from crates.io, and rc9.8
replaces them.

`voting-circuits 0.15.0` expands commitment inputs through Common 5's canonical
field encodings instead of the removed field-bit API. It yields the same bits,
so proofs and verification keys are unchanged and 5.3 and 5.4 clients can vote
in the same round.

### Wallet API changes

The rc9.7 wallet releases change some wallet APIs. `zcash_voting` itself does
not use them, so this matters only if your application calls them directly:

| rc9.6 | From rc9.7 |
| --- | --- |
| `WalletDb::prepare_receive_reservation(..)` | Takes the caller's live chain tip as a `tip: BlockHeight` argument, as `reserve_refund_key` does. |
| `WalletDb::check_one_time_history_for` | `WalletDb::recheck_one_time_history_for`, which first forgets each account's history results. |
| `WalletDb::reserve_refund_key` within 10 blocks of the tip | Needs the wallet scanned to within 1 block of the tip, and returns `ReservationPolicy::FundingHistory` until then. |
| `schemerz 0.2` and `schemerz-rusqlite 0.370` | `schemerz 0.3` and `schemerz-rusqlite 0.371`, for code naming `MigratorError` or implementing `RusqliteMigration`. |
| `SchedulingError::Distribution` | Removed. It was unreachable. |
| Upstream Arti types behind the `tor` feature | Zakura's Arti fork, published as `zakura-arti-client 0.47.0-zakura.1`. The new `tor-client` feature provides Tor without the exchange-rate helpers. |

The released changelogs have the complete list:
[`zakura-client-backend`](https://github.com/zakura-core/wallet-libraries/blob/0d51d3ce9d723b48548d928ebfc48e07bdbbcc77/librustzcash/zcash_client_backend/CHANGELOG.md),
[`zakura-client-sqlite`](https://github.com/zakura-core/wallet-libraries/blob/0d51d3ce9d723b48548d928ebfc48e07bdbbcc77/librustzcash/zcash_client_sqlite/CHANGELOG.md),
and [`zakura-wallet-lib`](https://github.com/zakura-core/wallet-libraries/blob/0d51d3ce9d723b48548d928ebfc48e07bdbbcc77/wallet-lib/CHANGELOG.md).

### Existing wallet databases

`zcash_voting` does not create or migrate the wallet database. Your
application's `zakura-client-sqlite` upgrade applies the rc9.7 migrations,
which only add indexes on `transactions`, `transparent_spend_map`, and
`addresses`. They drop no data. The voting sidecar database is unaffected.

## LRZ backend

The `lrz` backend selects the same upstream releases as 5.3: `orchard 0.16`,
`zcash_primitives 0.31`, `zcash_protocol 0.11`, `zcash_client_backend 0.25`,
`zcash_client_sqlite 0.23`, `zcash_keys 0.17`, and `pczt 0.10`. LRZ clients
need only the `zcash_voting` version bump.

## Wallet crates are pinned

The facade's requirements on the Zakura wallet crates are pre-release ranges.
`zakura-wallet-lib 0.1.0-rc9.6` asks for `zakura-client-backend ^0.1.0-rc9.6`
and `zakura-pczt ^0.1.0-rc9`, which also accept the Common 5.4 releases rc9.8
and rc9.1. 5.4 pins `zakura-client-backend` and `zakura-client-sqlite` to
`0.1.0-rc9.8` and `zakura-pczt` to `0.1.0-rc9.1` on the default backend, so a
later wallet pre-release on another Common family cannot be selected under it.

## Staying on 5.3

The 5.3.x line keeps Common 4.0.0 and the rc9.6 facade. A fresh resolution of
5.3.0 or 5.2.0 without a lockfile now selects the Common 5.4 wallet crates
beside those releases' Common 4 crates, and does not build. To stay on 5.3,
keep your lockfile, or pin the wallet crates your facade was built against:

```toml
zakura-client-backend = "=0.1.0-rc9.6"
zakura-client-sqlite = "=0.1.0-rc9.6"
zakura-pczt = "=0.1.0-rc9"
```

# Migrating from 5.2 to 5.3

Version 5.3 keeps the 5.2 voting lifecycle, wire formats, vote-tree encoding,
voting-sidecar schema, and Rust 1.97 minimum. It moves the wallet facade from
`zakura-wallet-lib 0.1.0-rc9.2` to `0.1.0-rc9.6`, which raises the minimum
wallet crate versions on both backends. Read the section for your backend.

## Wallet crates follow the facade

`zcash_voting::backend` re-exports the wallet crates selected by
`zakura-wallet-lib` (`zcash_client_backend`, `zcash_client_sqlite`,
`zcash_keys`, `orchard`, `pczt`, and `zcash_primitives`). Those re-exports are
part of this crate's public API. The Zakura wallet crates are pre-releases, and
a requirement such as `0.1.0-rc9.2` also accepts newer pre-releases like rc9.6,
even though their APIs differ.

That is why 5.3 is a minor release: it raises the minimum within a range that
5.2.0 already accepts. A fresh resolution of `zcash_voting =5.2.0` on the
default backend already selects `zakura-client-backend` and
`zakura-client-sqlite` `0.1.0-rc9.6`. If your application uses the wallet
crates directly, pin them to the versions you have tested, or keep your
lockfile, rather than relying on the `zcash_voting` version alone.

## LRZ backend

The `lrz` backend now requires the stable upstream wallet releases instead of
accepting their pre-releases:

| Dependency | Version |
| --- | --- |
| zcash_voting | 5.3.0 |
| zakura-wallet-lib | 0.1.0-rc9.6 |
| orchard | 0.16 |
| zcash_primitives | 0.31 |
| zcash_protocol | 0.11 |
| zcash_client_backend | 0.25 |
| zcash_client_sqlite | 0.23 |
| zcash_keys | 0.17 |
| pczt | 0.10 |

Clients already building against these stable releases need no other change.
Clients still on an upstream pre-release must move to the stable release first.
The stable releases changed some APIs relative to their pre-releases, for
example `zcash_primitives` replaced the `TxDigests` struct with an enum and
replaced the free `sighash::signature_hash` function with a method on
precomputed sighash data. Consult the upstream changelogs for the full list.

## Zakura backend

The default backend now requires these Zakura wallet releases:

| Dependency | Version |
| --- | --- |
| zcash_voting | 5.3.0 |
| zakura-wallet-lib | 0.1.0-rc9.6 |
| zakura-client-backend | 0.1.0-rc9.6 |
| zakura-client-sqlite | 0.1.0-rc9.6 |
| zakura-pczt | 0.1.0-rc9 |

The Zakura Common 4.0.0 crates, `voting-circuits 0.14.0`,
`voting-crypto-deps 0.4.0`, `vote-commitment-tree 0.8.1`, and
`vote-commitment-tree-client 0.10.1` are unchanged from 5.2.0.

Move direct dependencies on the Zakura wallet crates, including any
`[patch.crates-io]` entries that point at a wallet-libraries commit, to
releases compatible with the table above in the same change. An exact pin on
an earlier release, such as `zakura-client-backend = "=0.1.0-rc9.4"`, does not
resolve with 5.3.

The rc9.3 through rc9.6 releases change wallet APIs. `zcash_voting` itself does
not use the changed APIs, so this matters only if your application calls them
directly. The main adaptations are:

| Before (rc9.2) | From rc9.6 |
| --- | --- |
| Scanning with one-time IVKs (`scan_cached_blocks_with_one_time_ivks`, `decrypt_transaction_with_one_time_ivks`, `OneTimeIvkRead`, `OneTimeIvkWrite`) | Ordinary sync scans with account keys. Private payment recovery runs through the bounded PIR lookup queue in `one_time_ivk::restore`. |
| `WitnessRequest`, `WitnessAnswer`, `RecoveryStatus::witnesses`, `WalletDb::record_one_time_witness`, `WalletDb::refresh_one_time_witness` | Witness payloads arrive with each lookup record and are stored by `WalletDb::record_one_time_lookup`. |
| `WalletDb::record_one_time_filters` | `WalletDb::record_one_time_filter_span`, which returns a `FilterOutcome`. |
| `WalletDb::set_one_time_recovery_enabled` | `WalletDb::enable_one_time_recovery`. Recovery can no longer be paused. |
| `WalletDb::begin_one_time_recovery(.., direct)` | `WalletDb::begin_one_time_recovery(.., complete)`. Direct catch-up is gone. |
| `ambassador` delegation of the wallet data traits under `test-dependencies` | Implement the traits on your wrapper type directly. |

The released changelogs have the complete list:
[`zakura-client-backend`](https://github.com/zakura-core/wallet-libraries/blob/5164faa8176afc77063e32bd7ded475cb3f79f88/librustzcash/zcash_client_backend/CHANGELOG.md),
[`zakura-client-sqlite`](https://github.com/zakura-core/wallet-libraries/blob/5164faa8176afc77063e32bd7ded475cb3f79f88/librustzcash/zcash_client_sqlite/CHANGELOG.md),
and [`zakura-wallet-lib`](https://github.com/zakura-core/wallet-libraries/blob/5164faa8176afc77063e32bd7ded475cb3f79f88/wallet-lib/CHANGELOG.md).

## Existing wallet databases

`zcash_voting` does not create or migrate the wallet database. It opens the
application's existing connection with `WalletDb::from_connection`, so the
application's own `zakura-client-sqlite` upgrade applies the rc9.6 migrations.

Those migrations include `one_time_reset`, which drops one-time recovery
candidates and progress and cannot be reverted. It does not rebuild use
markers, receive windows, or bootstrap lookups, so `zakura-client-sqlite`
recommends recreating development wallets that hold one-time data, and some
databases from earlier development builds no longer open. Back up wallet
databases before upgrading, and qualify an existing supported database through
open, migration, synchronization, and voting round recovery before shipping.
The voting sidecar database is unaffected.

## Staying on 5.2

The 5.2.x line keeps the `zakura-wallet-lib 0.1.0-rc9.2` facade and receives
semver-compatible fixes. Staying on it does not by itself keep earlier Zakura
wallet crates, as described in
[Wallet crates follow the facade](#wallet-crates-follow-the-facade). To keep an
earlier wallet stack, stay on 5.2.x and keep your wallet crate pins or lockfile
until you are ready to move to 5.3. 5.3 cannot resolve with wallet crates older
than rc9.6.

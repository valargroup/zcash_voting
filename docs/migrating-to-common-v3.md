# Moving the v5.1 release line to Common v3

Version 5.1.1-rc.4 keeps the v5.1 round, signing, submission, and share-tracking APIs.
The breaking change is the cryptographic dependency family exposed by those APIs.
Use Rust 1.97 or newer and rebuild bindings and downstream Rust consumers together.

## Align the dependency family

| Dependency | Version for this release |
| --- | --- |
| zcash_voting | 5.1.1-rc.4 |
| voting-circuits | 0.13.0 |
| voting-crypto-deps | 0.3.0 |
| imt-tree | 0.6.0 |
| pir-types | 0.7.0 |
| pir-client | 0.8.0 |
| vote-commitment-tree | 0.7.0 |
| vote-commitment-tree-client | 0.9.0 |
| zakura-wallet-lib | 0.1.0-rc8 |

The default `zakura` backend uses Common 3.0.0. Replace direct Common v2
requirements with the matching Common v3 packages, and remove temporary Git
patches used during integration. A note, field element, address, or witness
from a different package version is a different Rust type even if its bytes
have the same format. Prefer the matching exports from `voting-crypto-deps`
when constructing circuit inputs and Orchard witnesses.

The optional `lrz` backend uses the current upstream Orchard and wallet
prereleases selected by the wallet facade. Keep `default-features = false`
when selecting `features = ["lrz"]`; selecting both backends is invalid.
The Vote Commitment Tree still uses incrementalmerkletree 0.8 independently
of Orchard's selected tree API. Use `vote_commitment_tree::incrementalmerkletree`
for a frontier over `MerkleHashVote`.

## Existing wallet state and protocol inputs

This release introduces no database migration, vote-tree encoding change, or
new voting-service prerequisite. It retains Ironwood / NU6.3 eligibility and
the existing authenticated snapshot, roster, and chain-confirmation checks.
It does not change voting circuit public inputs or domain separation.

Retain the existing wallet-scoped sidecar and app-owned voting secret.
Integrations from v3 or v4 must first follow the applicable
[v5 migration path](migrating-to-v5.md), including its restrictions on
in-flight historical submissions. The dependency update does not expand
those supported upgrade paths or make old databases safe to downgrade.

This release also includes the v5.0.2 SQLite writer reservation and tree-recovery
fixes, plus v5.1.0's opt-in Ledger output review. Integrations already using
those releases retain that behavior; consult the historical changelog entries
for their caller requirements.

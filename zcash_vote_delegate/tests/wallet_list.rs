//! The wallet list payload (§12.3) against vote-sdk's known answer. The
//! shared vectors pin the encoding from this crate's side; this pins it from
//! the Go implementation's.

use sha2::{Digest, Sha256};
use zcash_vote_delegate::{DelegateSigningKey, WalletList, WalletListRow};

/// The SHA-256 of the payload vote-sdk's `internal/votingconfig` tests and
/// its signing page compute for [`go_test_list`].
const GO_PAYLOAD_SHA256: &str = "39981de7f3ad357c18048c141ac2359955485d12945dbc3670e4b5a9d036989c";

/// The `zvdk1` key vote-sdk's tests derive for `label`: the public key of
/// the seed `SHA-256("votingconfig wallet list test/" || label)`.
fn go_test_key(label: &str) -> String {
    let seed: [u8; 32] = Sha256::digest(format!("votingconfig wallet list test/{label}")).into();
    DelegateSigningKey::from_secret_seed(seed)
        .delegate_key()
        .encode()
}

/// vote-sdk's two-row test list: alice on X, and bob on GitHub with a
/// non-ASCII NFC name.
fn go_test_list() -> WalletList {
    WalletList {
        wallet_id: "vizor".to_owned(),
        chain_id: "zvote-1".to_owned(),
        version: 1,
        issued_at: 1_790_000_000,
        reserved_names: ["zcash", "electriccoin", "zodl"]
            .map(str::to_owned)
            .to_vec(),
        delegates: vec![
            WalletListRow {
                index: 17,
                provider: 1,
                handle: "alice".to_owned(),
                dk: go_test_key("alice"),
                name: "Alice Example".to_owned(),
            },
            WalletListRow {
                index: 23,
                provider: 2,
                handle: "bob-dev".to_owned(),
                dk: go_test_key("bob"),
                name: "B\u{f3}b \u{d1}and\u{fa}".to_owned(),
            },
        ],
    }
}

#[test]
fn the_payload_is_the_one_vote_sdk_computes() {
    let payload = go_test_list().payload().unwrap();
    assert_eq!(hex::encode(Sha256::digest(payload)), GO_PAYLOAD_SHA256);
}

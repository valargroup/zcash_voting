//! The wallet list payload (§12.3) against vote-sdk's known answer. The
//! shared vectors pin the encoding from this crate's side; this pins it from
//! the Go implementation's.

use sha2::{Digest, Sha256};
use zcash_vote_delegate::{DelegateSigningKey, WalletList, WalletListRow};

/// The SHA-256 of the payload vote-sdk's `internal/votingconfig` tests and
/// its signing page compute for [`go_test_list`].
const GO_PAYLOAD_SHA256: &str = "a16b7b13f55d8a0a9d2f396c3a4f272c81a5f86f33b46c3f17e1093c35e897b2";

/// A 128 × 128 lossless WebP of 82 bytes.
const PICTURE_WEBP: &str = "524946464a000000574542505650384c3d0000002f7fc01f000f708fcc7b58df457ffe\
                            030f8ada3692cc9f7400ecd16f44ff27a1a028d88bcef6004541517b511414839def\
                            ef3bde03d0d91ea028286a2f00";

/// The `zvdk1` key vote-sdk's tests derive for `label`: the public key of
/// the seed `SHA-256("votingconfig wallet list test/" || label)`.
fn go_test_key(label: &str) -> String {
    let seed: [u8; 32] = Sha256::digest(format!("votingconfig wallet list test/{label}")).into();
    DelegateSigningKey::from_secret_seed(seed)
        .delegate_key()
        .encode()
}

/// vote-sdk's two-row test list: alice on X with a statement and a picture,
/// and bob on GitHub with a non-ASCII NFC name.
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
                statement: Some(
                    "I vote for privacy first.\nAsk me anything about my ballots.".to_owned(),
                ),
                picture: Some(hex::decode(PICTURE_WEBP).unwrap()),
            },
            WalletListRow {
                index: 23,
                provider: 2,
                handle: "bob-dev".to_owned(),
                dk: go_test_key("bob"),
                name: "B\u{f3}b \u{d1}and\u{fa}".to_owned(),
                statement: None,
                picture: None,
            },
        ],
    }
}

#[test]
fn the_payload_is_the_one_vote_sdk_computes() {
    let payload = go_test_list().payload().unwrap();
    assert_eq!(hex::encode(Sha256::digest(payload)), GO_PAYLOAD_SHA256);
}

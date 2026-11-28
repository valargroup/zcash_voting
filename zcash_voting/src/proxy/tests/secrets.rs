//! DC secrets come from the round hotkey alone and agree with the shared
//! golden vectors (contract §11.1, §13 `dc_secrets`).

use std::collections::BTreeSet;

use zcash_vote_delegate::dc_seed;

use crate::{
    proxy::{proxy_round_key, DcSeedInputs, DcShareLayout},
    types::Network,
};

/// The shared crate's client vector file, which every implementation tests
/// against.
const PROXY_DELEGATION_VECTORS: &str =
    include_str!("../../../../zcash_vote_delegate/test-vectors/proxy_delegation_v1.json");

/// Decodes a lower-case hex vector field of exactly `N` bytes.
fn hex_bytes<const N: usize>(value: &serde_json::Value) -> [u8; N] {
    hex::decode(value.as_str().expect("a hex string"))
        .expect("valid hex")
        .try_into()
        .expect("the vector's byte length")
}

/// The network whose `network_rank` is `net_u8`.
fn network_of(net_u8: u64) -> Network {
    match net_u8 {
        0 => Network::Mainnet,
        1 => Network::Testnet,
        2 => Network::Regtest,
        other => panic!("unknown net_u8 {other}"),
    }
}

/// Every `dc_secrets` vector, in both layouts, gives the expected `hrk` and
/// `dc_seed` through this crate's network mapping and key.
#[test]
fn dc_seeds_match_the_shared_vectors_for_both_layouts() {
    let vectors: serde_json::Value = serde_json::from_str(PROXY_DELEGATION_VECTORS).unwrap();
    let cases = vectors["dc_secrets"].as_array().expect("dc_secrets cases");
    let mut layouts = BTreeSet::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let inputs = &case["inputs"];
        let expected = &case["expected"];
        let hotkey_secret: [u8; 32] = hex_bytes(&inputs["hotkey_secret"]);
        let net_u8 = inputs["net_u8"].as_u64().unwrap();
        let round_id: [u8; 32] = hex_bytes(&inputs["round_id"]);
        let layout_u8 = inputs["layout"].as_u64().unwrap();
        layouts.insert(layout_u8);
        let dc = DcSeedInputs {
            van_nullifier: hex_bytes(&inputs["van_nullifier"]),
            delegate_index: u32::try_from(inputs["delegate_index"].as_u64().unwrap()).unwrap(),
            weight_ballots: inputs["weight_ballots"].as_u64().unwrap(),
            layout: match layout_u8 {
                0 => DcShareLayout::Standard,
                1 => DcShareLayout::SingleShare,
                other => panic!("{name}: unknown layout {other}"),
            },
        };

        // This crate's network mapping and key give the vector's `hrk`, and
        // the `dc_seed` it keys.
        let hrk = proxy_round_key(&hotkey_secret, network_of(net_u8), &round_id);
        assert_eq!(*hrk, hex_bytes::<32>(&expected["hrk"]), "{name}: hrk");
        assert_eq!(
            dc_seed(&hrk, &dc),
            hex_bytes::<32>(&expected["dc_seed"]),
            "{name}: dc_seed"
        );
    }
    assert_eq!(layouts, BTreeSet::from([0, 1]), "both layouts are covered");
}

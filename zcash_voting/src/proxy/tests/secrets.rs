//! DC secrets come from the round hotkey alone and agree with the shared
//! golden vectors (contract §11.1, §13 `dc_secrets`).

use std::collections::BTreeSet;

use zcash_vote_delegate::{dc_seed, hotkey_round_key, ZcashNetwork};

use crate::{
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    proxy::{DcSeedInputs, DcShareLayout, ProxyRoundKey},
    types::Network,
};

/// The shared crate's client vector file, which every implementation tests
/// against.
const PROXY_DELEGATION_VECTORS: &str =
    include_str!("../../../../zcash_vote_delegate/test-vectors/proxy_delegation_v1.json");

/// A stored hotkey seed, as Vizor keeps one per account and round.
const HOTKEY_SEED: [u8; 64] = [0xAB; 64];
/// A round id's 32 bytes.
const ROUND_ID_BYTES: [u8; 32] = [0x07; 32];

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

/// The shared crate's network for `network`; its `net_u8` must be this
/// crate's network rank.
fn shared_network(network: Network) -> ZcashNetwork {
    match network {
        Network::Mainnet => ZcashNetwork::Mainnet,
        Network::Testnet => ZcashNetwork::Testnet,
        Network::Regtest => ZcashNetwork::Regtest,
    }
}

/// The inputs of one standard-layout DC.
fn standard_dc() -> DcSeedInputs {
    DcSeedInputs {
        van_nullifier: [0x09; 32],
        delegate_index: 17,
        weight_ballots: 40,
        layout: DcShareLayout::Standard,
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

        // `hrk` is the contract formula over the vector's hotkey secret, with
        // this crate's network mapped to its `net_u8`.
        let network = network_of(net_u8);
        assert_eq!(
            u64::from(shared_network(network).net_u8()),
            net_u8,
            "{name}"
        );
        let hrk = hotkey_round_key(&hotkey_secret, shared_network(network), &round_id);
        assert_eq!(hrk, hex_bytes::<32>(&expected["hrk"]), "{name}: hrk");

        // This crate's derivation, keyed by its own `hrk`, gives the vector's
        // `dc_seed`.
        let seed =
            ProxyRoundKey::from_hotkey_secret(&hotkey_secret, network, &round_id).dc_seed(&dc);
        assert_eq!(
            seed.expose_secret(),
            &hex_bytes::<32>(&expected["dc_seed"]),
            "{name}: dc_seed"
        );
    }
    assert_eq!(layouts, BTreeSet::from([0, 1]), "both layouts are covered");
}

/// `hotkey_secret` is the Orchard spending key the vote flow derives from the
/// stored hotkey seed.
#[test]
fn the_hotkey_secret_is_the_spending_key_the_vote_flow_derives() {
    for network in [Network::Mainnet, Network::Testnet, Network::Regtest] {
        let spending_key =
            spending_key_from_hotkey_seed(&HOTKEY_SEED, network, VOTING_HOTKEY_ACCOUNT_INDEX)
                .unwrap();
        let from_seed = ProxyRoundKey::from_hotkey_seed(&HOTKEY_SEED, network, &ROUND_ID_BYTES)
            .unwrap()
            .dc_seed(&standard_dc());
        let contract = dc_seed(
            &hotkey_round_key(
                spending_key.to_bytes(),
                shared_network(network),
                &ROUND_ID_BYTES,
            ),
            &standard_dc(),
        );
        assert_eq!(from_seed.expose_secret(), &contract, "{network:?}");
    }
}

/// Neither `hrk` nor a DC seed appears in `Debug` output.
#[test]
fn secrets_never_appear_in_debug_output() {
    let round_key =
        ProxyRoundKey::from_hotkey_seed(&HOTKEY_SEED, Network::Testnet, &ROUND_ID_BYTES).unwrap();
    let seed = round_key.dc_seed(&standard_dc());
    assert_eq!(format!("{round_key:?}"), "ProxyRoundKey(..)");
    assert_eq!(format!("{seed:?}"), "DcSeed(..)");
}

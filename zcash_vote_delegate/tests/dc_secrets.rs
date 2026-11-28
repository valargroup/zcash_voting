//! DC secret derivation from the round hotkey (§11.1).

use zcash_vote_delegate::{dc_seed, hotkey_round_key, DcSeedInputs, DcShareLayout, ZcashNetwork};

const HOTKEY_SECRET: [u8; 32] = [7; 32];
const ROUND_ID: [u8; 32] = [8; 32];

/// The inputs of one standard-layout DC.
fn standard_dc() -> DcSeedInputs {
    DcSeedInputs {
        van_nullifier: [9; 32],
        delegate_index: 17,
        weight_ballots: 40,
        layout: DcShareLayout::Standard,
    }
}

/// Keyed, personalized BLAKE2b-256, computed independently of the crate.
fn keyed_blake2b_256(key: &[u8; 32], personalization: &[u8; 16], message: &[u8]) -> [u8; 32] {
    blake2b_simd::Params::new()
        .hash_length(32)
        .key(key)
        .personal(personalization)
        .hash(message)
        .as_bytes()
        .try_into()
        .unwrap()
}

/// `hrk` and `dc_seed` are the contract's keyed BLAKE2b formulas, with a
/// 33-byte and a 45-byte message.
#[test]
fn secrets_follow_the_contract_formulas() {
    let hrk = hotkey_round_key(&HOTKEY_SECRET, ZcashNetwork::Testnet, &ROUND_ID);
    let mut hrk_message = vec![1u8];
    hrk_message.extend_from_slice(&ROUND_ID);
    assert_eq!(
        hrk,
        keyed_blake2b_256(&HOTKEY_SECRET, b"ZVoteProxyHRK_v1", &hrk_message)
    );

    let dc = DcSeedInputs {
        layout: DcShareLayout::SingleShare,
        ..standard_dc()
    };
    let mut seed_message = dc.van_nullifier.to_vec();
    seed_message.extend_from_slice(&17u32.to_le_bytes());
    seed_message.extend_from_slice(&40u64.to_le_bytes());
    seed_message.push(1);
    assert_eq!(seed_message.len(), 45);
    assert_eq!(
        dc_seed(&hrk, &dc),
        keyed_blake2b_256(&hrk, b"ZVoteProxySeed01", &seed_message)
    );
}

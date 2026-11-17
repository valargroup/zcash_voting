use crate::delegation_key_fingerprint::DelegationKeyFingerprint;

/// The fingerprint the vote chain's verifier reports for its delegation key.
///
/// The vote chain rejects a delegation whose fingerprint differs from its
/// own, so this value must change only together with a circuit change that
/// the chain has also taken. The vote-sdk repository pins the same value.
const VOTE_CHAIN_DELEGATION_KEY_FINGERPRINT: &str =
    "e1773ecd2eb8531cc54c1df5b935a978788ef4eea4949cb45a00edf550ecee81";

/// Runs delegation key generation, so it is a release-only proof test
/// (`make proofs`), which covers both wallet backends.
#[test]
#[ignore = "generates the delegation circuit keys; run with `make proofs`"]
fn this_build_proves_with_the_delegation_key_the_vote_chain_verifies_with() {
    let (_, _, verifying_key) =
        voting_circuits::delegation::delegation_cached_keys().expect("delegation keys");

    let fingerprint = DelegationKeyFingerprint::of_process_delegation_key(verifying_key);

    assert_eq!(fingerprint.to_hex(), VOTE_CHAIN_DELEGATION_KEY_FINGERPRINT);
}

use super::*;

use pasta_curves::group::ff::PrimeField;
use voting_circuits::delegation::{ImtProvider, SpacedLeafImtProvider};

const WALLET: &str = "chunked-pir-wallet";

/// Ten distinct raw nullifiers, more than two full fetch chunks.
fn extra_nullifiers() -> Vec<Vec<u8>> {
    (7u64..17)
        .map(|value| pallas::Base::from(value).to_repr().to_vec())
        .collect()
}

/// Answers each requested nullifier with a valid proof from `imt`.
fn proofs_for(
    imt: &SpacedLeafImtProvider,
    nullifiers: &[pallas::Base],
) -> Result<Vec<pir_client::ImtProofData>, VotingError> {
    Ok(nullifiers
        .iter()
        .map(|nullifier| {
            let proof = imt.non_membership_proof(*nullifier).unwrap();
            pir_client::ImtProofData {
                root: proof.root,
                nf_bounds: proof.nf_bounds,
                leaf_pos: proof.leaf_pos,
                path: proof.path,
            }
        })
        .collect())
}

fn cached_proof_count(db: &VotingDb) -> u32 {
    db.conn()
        .query_row("SELECT COUNT(*) FROM pir_proof_cache", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn missing_proofs_are_fetched_in_bounded_chunks() {
    let db = VotingDb::open_in_memory().unwrap();
    db.set_wallet_id(WALLET);
    let imt = SpacedLeafImtProvider::new();
    let mut chunk_sizes = Vec::new();

    let result = db
        .precompute_pir_proof_cache_inner(
            WALLET,
            &[],
            &extra_nullifiers(),
            Network::Testnet,
            imt.root(),
            |nullifiers| {
                chunk_sizes.push(nullifiers.len());
                proofs_for(&imt, nullifiers)
            },
        )
        .unwrap();

    assert_eq!(chunk_sizes, vec![4, 4, 2]);
    assert_eq!(result.fetched_count, 10);
    assert_eq!(cached_proof_count(&db), 10);
}

#[test]
fn a_failed_chunk_still_sends_every_chunk_and_saves_nothing() {
    // Which queries fail can depend on the wallet's nullifiers, so neither the
    // number sent now nor the number a retry sends may depend on the failure.
    for retryable in [false, true] {
        let db = VotingDb::open_in_memory().unwrap();
        db.set_wallet_id(WALLET);
        let imt = SpacedLeafImtProvider::new();
        let nullifiers = extra_nullifiers();
        let mut calls = 0;

        let error = db
            .precompute_pir_proof_cache_inner(
                WALLET,
                &[],
                &nullifiers,
                Network::Testnet,
                imt.root(),
                |requested| {
                    calls += 1;
                    if calls == 1 {
                        return Err(VotingError::PirUnavailable {
                            endpoint: None,
                            http_status: None,
                            retryable,
                            message: "chunk failed".to_string(),
                        });
                    }
                    proofs_for(&imt, requested)
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("chunk failed"), "{error}");
        assert_eq!(calls, 3, "every chunk was still sent");
        assert_eq!(cached_proof_count(&db), 0, "nothing was kept");

        let mut retried = 0;
        db.precompute_pir_proof_cache_inner(
            WALLET,
            &[],
            &nullifiers,
            Network::Testnet,
            imt.root(),
            |requested| {
                retried += requested.len();
                proofs_for(&imt, requested)
            },
        )
        .unwrap();
        assert_eq!(retried, 10, "the retry sends the full set again");
    }
}

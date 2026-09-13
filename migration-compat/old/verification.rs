//! Check retained delegation proofs with the verifier shipped alongside v3.0.0.
//! Ledger roots come from the captured round, not from reconstructed prover input.
use anyhow::{ensure, Context, Result};
use pasta_curves::{group::ff::PrimeField, pallas};
use voting_circuits::delegation::{derive_nullifier_domain, verify_delegation_proof, Instance};

fn field(bytes: &[u8]) -> Result<pallas::Base> {
    let encoded: [u8; 32] = bytes.try_into().context("field length")?;
    Option::from(pallas::Base::from_repr(encoded)).context("noncanonical field")
}

pub fn verify(path: &std::path::Path) -> Result<()> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare("SELECT p.proof,b.rk,b.nf_signed,b.cmx_new,b.gov_comm,b.gov_nullifiers_blob,r.round_id,r.nc_root,r.nullifier_imt_root
        FROM proofs p JOIN bundles b USING(round_id,wallet_id,bundle_index) JOIN rounds r USING(round_id,wallet_id) WHERE p.success=1 ORDER BY b.bundle_index")?;
    let mut rows = statement.query([])?;
    let mut verified = 0;
    while let Some(row) = rows.next()? {
        let proof: Vec<u8> = row.get(0)?;
        let rk: Vec<u8> = row.get(1)?;
        let nf: Vec<u8> = row.get(2)?;
        let cmx: Vec<u8> = row.get(3)?;
        let van: Vec<u8> = row.get(4)?;
        let nullifiers: Vec<u8> = row.get(5)?;
        let round: String = row.get(6)?;
        let nc_root: Vec<u8> = row.get(7)?;
        let imt_root: Vec<u8> = row.get(8)?;
        let round = field(&super::decode_hex(&round)?)?;
        let nullifiers = nullifiers
            .chunks_exact(32)
            .map(field)
            .collect::<Result<Vec<_>>>()?;
        let instance = Instance::from_parts(
            Option::from(orchard::note::Nullifier::from_bytes(
                &nf.try_into()
                    .map_err(|_| anyhow::anyhow!("nullifier length"))?,
            ))
            .context("invalid nullifier")?,
            orchard::primitives::redpallas::VerificationKey::try_from(
                <[u8; 32]>::try_from(rk).map_err(|_| anyhow::anyhow!("key length"))?,
            )
            .map_err(|_| anyhow::anyhow!("invalid verification key"))?,
            field(&cmx)?,
            field(&van)?,
            round,
            field(&nc_root)?,
            field(&imt_root)?,
            nullifiers
                .try_into()
                .map_err(|_| anyhow::anyhow!("nullifier count"))?,
            derive_nullifier_domain(round),
        )?;
        verify_delegation_proof(&proof, &instance).map_err(anyhow::Error::msg)?;
        let mut damaged = proof.clone();
        ensure!(!damaged.is_empty(), "empty proof");
        damaged[0] ^= 1;
        ensure!(
            verify_delegation_proof(&damaged, &instance).is_err(),
            "negative proof control was accepted"
        );
        verified += 1;
    }
    ensure!(verified > 0, "no delegation proof to verify");
    println!("v3.0.0 verifier accepted {verified} retained delegation proof(s); damaged controls rejected");
    Ok(())
}

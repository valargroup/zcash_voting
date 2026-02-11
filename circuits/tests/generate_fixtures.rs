//! Integration test that generates fixture files for Go tests.
//!
//! Run with: cargo test --release -- generate_fixtures --ignored --nocapture
//!
//! Generates:
//!   crypto/zkp/testdata/toy_valid_proof.bin      - valid Halo2 proof bytes
//!   crypto/zkp/testdata/toy_valid_input.bin      - correct public input (Fp, 32-byte LE)
//!   crypto/zkp/testdata/toy_wrong_input.bin      - wrong public input for negative tests
//!   crypto/redpallas/testdata/valid_rk.bin       - 32-byte RedPallas verification key
//!   crypto/redpallas/testdata/valid_sighash.bin  - 32-byte sighash (message)
//!   crypto/redpallas/testdata/valid_sig.bin      - 64-byte valid RedPallas signature
//!   crypto/redpallas/testdata/wrong_sig.bin      - 64-byte signature over wrong message

use pasta_curves::group::ff::PrimeField;
use std::fs;
use std::path::Path;

use rand::thread_rng;
use reddsa::{orchard, SigningKey, VerificationKey};

use zally_circuits::toy;
use zally_circuits::redpallas as rp;

/// Generate fixture files for Go tests.
///
/// Marked `#[ignore]` so it only runs when explicitly requested
/// (e.g., `cargo test --release -- generate_fixtures --ignored`).
/// This avoids regenerating fixtures on every `cargo test`.
#[test]
#[ignore]
fn generate_fixtures() {
    generate_halo2_fixtures();
    generate_redpallas_fixtures();
    println!("\nAll fixtures generated and validated successfully.");
}

/// Generate Halo2 toy circuit proof fixtures.
fn generate_halo2_fixtures() {
    let testdata_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("crypto/zkp/testdata");

    fs::create_dir_all(&testdata_dir).expect("failed to create testdata directory");

    // Generate a valid proof with known inputs: a=2, b=3, constant=7.
    // c = 7 * 2^2 * 3^2 = 7 * 4 * 9 = 252
    let (proof, c) = toy::create_toy_proof(2, 3);

    // Serialize the public input as 32-byte little-endian (Pallas Fp repr).
    let c_bytes = c.to_repr();

    // Write valid proof.
    let proof_path = testdata_dir.join("toy_valid_proof.bin");
    fs::write(&proof_path, &proof).expect("failed to write proof fixture");
    println!(
        "Wrote valid proof ({} bytes) to {}",
        proof.len(),
        proof_path.display()
    );

    // Write valid public input.
    let input_path = testdata_dir.join("toy_valid_input.bin");
    fs::write(&input_path, c_bytes.as_ref()).expect("failed to write input fixture");
    println!(
        "Wrote valid input ({} bytes) to {}",
        c_bytes.as_ref().len(),
        input_path.display()
    );

    // Write wrong public input (c = 999, which does not match any valid (a,b) for constant=7).
    use halo2_proofs::pasta::Fp;
    let wrong_c = Fp::from(999u64);
    let wrong_bytes = wrong_c.to_repr();
    let wrong_path = testdata_dir.join("toy_wrong_input.bin");
    fs::write(&wrong_path, wrong_bytes.as_ref()).expect("failed to write wrong input fixture");
    println!(
        "Wrote wrong input ({} bytes) to {}",
        wrong_bytes.as_ref().len(),
        wrong_path.display()
    );

    // Verify the generated proof works before committing the fixtures.
    assert!(
        toy::verify_toy(&proof, &c).is_ok(),
        "generated proof should verify against correct input"
    );
    assert!(
        toy::verify_toy(&proof, &wrong_c).is_err(),
        "generated proof should NOT verify against wrong input"
    );

    println!("Halo2 fixtures generated and validated.");
}

/// Generate RedPallas SpendAuth signature fixtures.
fn generate_redpallas_fixtures() {
    let testdata_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("crypto/redpallas/testdata");

    fs::create_dir_all(&testdata_dir).expect("failed to create redpallas testdata directory");

    let mut rng = thread_rng();

    // Generate a signing key and derive the verification key (rk).
    let sk = SigningKey::<orchard::SpendAuth>::new(&mut rng);
    let vk = VerificationKey::from(&sk);

    // The sighash is a 32-byte message (in production, derived from raw tx bytes).
    let sighash: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
        0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
        0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
    ];

    // Sign the sighash.
    let sig = sk.sign(&mut rng, &sighash);

    // Serialize to byte arrays.
    let rk_bytes: [u8; 32] = vk.into();
    let sig_bytes: [u8; 64] = sig.into();

    // Write valid rk.
    let rk_path = testdata_dir.join("valid_rk.bin");
    fs::write(&rk_path, &rk_bytes).expect("failed to write rk fixture");
    println!("Wrote valid rk ({} bytes) to {}", rk_bytes.len(), rk_path.display());

    // Write valid sighash.
    let sighash_path = testdata_dir.join("valid_sighash.bin");
    fs::write(&sighash_path, &sighash).expect("failed to write sighash fixture");
    println!(
        "Wrote valid sighash ({} bytes) to {}",
        sighash.len(),
        sighash_path.display()
    );

    // Write valid signature.
    let sig_path = testdata_dir.join("valid_sig.bin");
    fs::write(&sig_path, &sig_bytes).expect("failed to write sig fixture");
    println!(
        "Wrote valid sig ({} bytes) to {}",
        sig_bytes.len(),
        sig_path.display()
    );

    // Generate a wrong signature: sign a different message.
    let wrong_msg: [u8; 32] = [0xff; 32];
    let wrong_sig = sk.sign(&mut rng, &wrong_msg);
    let wrong_sig_bytes: [u8; 64] = wrong_sig.into();

    let wrong_sig_path = testdata_dir.join("wrong_sig.bin");
    fs::write(&wrong_sig_path, &wrong_sig_bytes).expect("failed to write wrong sig fixture");
    println!(
        "Wrote wrong sig ({} bytes) to {}",
        wrong_sig_bytes.len(),
        wrong_sig_path.display()
    );

    // Verify the generated fixtures work before committing.
    assert!(
        rp::verify_spend_auth_sig(&rk_bytes, &sighash, &sig_bytes).is_ok(),
        "valid signature should verify"
    );
    assert!(
        rp::verify_spend_auth_sig(&rk_bytes, &sighash, &wrong_sig_bytes).is_err(),
        "wrong signature should NOT verify"
    );

    println!("RedPallas fixtures generated and validated.");
}

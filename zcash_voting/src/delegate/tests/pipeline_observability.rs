use super::*;
use std::sync::Arc;

struct NoPirTransport;
impl pir_client::Transport for NoPirTransport {
    fn get<'a>(&'a self, _: &'a str) -> pir_client::TransportFuture<'a> {
        panic!("cached proof must not contact PIR")
    }
    fn post<'a>(&'a self, _: &'a str, _: Vec<u8>) -> pir_client::TransportFuture<'a> {
        panic!("cached proof must not contact PIR")
    }
}

#[test]
fn pipeline_cached_proof_reports_reused_at_both_boundaries() {
    let (pipeline, _wallet) = super::pipeline_fixture::pipeline_with_wallet();
    let db = pipeline.voting_db();
    let eligibility =
        pipeline.eligibility_with_report(Some(crate::ObservabilityOptions::default()));
    assert!(eligibility.result.is_ok());
    assert_eq!(
        eligibility.observability.unwrap().outcome,
        crate::ObservationOutcome::Succeeded
    );
    let keystone =
        pipeline.keystone_request_with_report(0, Some(crate::ObservabilityOptions::default()));
    assert!(keystone.result.is_ok(), "{:?}", keystone.result);
    assert_eq!(
        keystone.observability.unwrap().outcome,
        crate::ObservationOutcome::Succeeded
    );
    {
        let conn = db.conn();
        // Seed the durable proof boundary, as in the proof-coordination fixtures.
        // This test exercises reuse and never asks the cryptographic prover to run.
        crate::storage::queries::store_proof(
            &conn,
            pipeline.round_id(),
            &db.wallet_id(),
            0,
            &[0xA1; 96],
        )
        .unwrap();
    }
    let pir = crate::pir::PirFleet::new(
        &[],
        crate::config::PirLayout {
            pir_depth: pir_types::COMPILED_PIR_LAYOUT.pir_depth as u32,
            tier0_layers: pir_types::COMPILED_PIR_LAYOUT.tier0_layers as u32,
            tier1_layers: pir_types::COMPILED_PIR_LAYOUT.tier1_layers as u32,
            poly_len: pir_types::DEFAULT_YPIR_POLY_LEN as u32,
        },
        Arc::new(NoPirTransport),
    )
    .unwrap();
    let report = pipeline.ensure_proof_with_report(
        0,
        &pir,
        &crate::types::NoopProgressReporter,
        Some(crate::ObservabilityOptions::default()),
    );
    assert_eq!(report.result.unwrap(), DelegationProofStatus::Reused);
    let diagnostics = report.observability.unwrap();
    assert_eq!(diagnostics.outcome, crate::ObservationOutcome::Reused);
    let stage = diagnostics
        .records
        .iter()
        .find(|record| record.stage.as_ref() == "delegation::ensure_proof")
        .unwrap();
    assert_eq!(stage.outcome, crate::ObservationOutcome::Reused);
    assert_eq!(stage.attribution.bundle_index, Some(0));
}

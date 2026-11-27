//! Matching a served ballot against the ballot attestation in resolved config.

use super::fixtures::{
    full_ballot, resolved_config, trusted_key, untrusted_key, Attestation, ROUND_ID,
};

use crate::ballot::BallotAuthenticationError;
use crate::config::{decide_config_switch, ConfigSwitchKind, ResolvedVotingConfigSummary};

fn attested_by_trusted_key() -> Attestation {
    Attestation::Signed {
        ballot: full_ballot(),
        signer: trusted_key(),
    }
}

#[test]
fn a_ballot_matching_its_attestation_authenticates() {
    let config = resolved_config(attested_by_trusted_key());
    let round = &config.authenticated_rounds[0];
    assert_eq!(
        round.ballot_digest.as_deref(),
        Some(full_ballot().digest().to_hex().as_str())
    );

    let authenticated = config.authenticate_ballot(ROUND_ID, full_ballot()).unwrap();
    assert_eq!(authenticated.round_id(), ROUND_ID);
    assert_eq!(authenticated.ballot(), &full_ballot());
    assert_eq!(authenticated.digest(), full_ballot().digest());
    assert_eq!(authenticated.proposal_ids(), vec![1, 2]);
}

#[test]
fn a_ballot_differing_from_its_attestation_is_rejected() {
    let config = resolved_config(attested_by_trusted_key());
    let mut served = full_ballot();
    served.proposals[0].options.swap(0, 1);

    let error = config
        .authenticate_ballot(ROUND_ID, served.clone())
        .unwrap_err();
    assert_eq!(
        error,
        BallotAuthenticationError::DigestMismatch {
            round_id: ROUND_ID.to_string(),
            attested: full_ballot().digest().to_hex(),
            served: served.digest().to_hex(),
        }
    );
}

#[test]
fn a_round_without_a_ballot_attestation_still_authenticates_but_its_ballot_does_not() {
    let config = resolved_config(Attestation::Absent);
    assert_eq!(config.authenticated_rounds.len(), 1);
    assert_eq!(config.authenticated_rounds[0].ballot_digest, None);

    assert_eq!(
        config
            .authenticate_ballot(ROUND_ID, full_ballot())
            .unwrap_err(),
        BallotAuthenticationError::BallotNotAttested {
            round_id: ROUND_ID.to_string(),
        }
    );
}

#[test]
fn an_attestation_by_an_untrusted_key_skips_the_round() {
    let config = resolved_config(Attestation::Signed {
        ballot: full_ballot(),
        signer: untrusted_key(),
    });
    assert!(config.authenticated_rounds.is_empty());
    assert_eq!(config.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn an_attestation_with_a_malformed_digest_skips_the_round() {
    let config = resolved_config(Attestation::MalformedDigest);
    assert!(config.authenticated_rounds.is_empty());
    assert_eq!(config.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn an_unknown_round_is_rejected() {
    let config = resolved_config(attested_by_trusted_key());
    let other_round = "02".repeat(32);
    assert_eq!(
        config
            .authenticate_ballot(&other_round, full_ballot())
            .unwrap_err(),
        BallotAuthenticationError::RoundNotAuthenticated {
            round_id: other_round,
        }
    );
}

#[test]
fn authenticated_rounds_persisted_before_ballot_attestation_still_deserialize() {
    let round: crate::config::AuthenticatedRound = serde_json::from_value(serde_json::json!({
        "round_id": ROUND_ID,
        "ea_pk": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
    }))
    .unwrap();
    assert_eq!(round.ballot_digest, None);
}

#[test]
fn adding_a_ballot_attestation_to_a_live_round_is_not_a_config_change() {
    let before = ResolvedVotingConfigSummary::from(&resolved_config(Attestation::Absent));
    let after = ResolvedVotingConfigSummary::from(&resolved_config(attested_by_trusted_key()));
    assert_eq!(
        decide_config_switch(Some(before), after).kind,
        ConfigSwitchKind::Unchanged
    );
}

use super::*;

#[test]
fn malformed_batch_lists_identify_the_protocol_field() {
    for (diagnostic, field) in [
        (
            parse_csv_u32("1,,2").unwrap_err().to_string(),
            "proposal_ids",
        ),
        (
            parse_csv_u64("1,,2", "cast_vote_batch vote_commitment_leaf_indices")
                .unwrap_err()
                .to_string(),
            "vote_commitment_leaf_indices",
        ),
        (
            parse_csv_strings(
                &format!("{},,", "11".repeat(32)),
                "cast_vote_batch van_nullifiers",
            )
            .unwrap_err()
            .to_string(),
            "van_nullifiers",
        ),
    ] {
        assert!(
            diagnostic.contains(field),
            "{diagnostic:?} must identify {field}"
        );
    }
}

//! The canonical ballot encoding against the published cross-language vectors.

use serde::Deserialize;

use super::fixtures::full_ballot;
use crate::ballot::Ballot;
use crate::config::PirLayout;
use crate::round_auth::RoundAuthPayloadV3;

const VECTORS: &str = include_str!("../../../test-vectors/ballot_digest_v1.json");

#[derive(Deserialize)]
struct Vectors {
    ballot_digest_v1: Vec<DigestVector>,
    round_auth_v3: Vec<RoundAuthV3Vector>,
}

#[derive(Deserialize)]
struct DigestVector {
    name: String,
    ballot: Ballot,
    encoding_hex: String,
    digest_hex: String,
}

#[derive(Deserialize)]
struct RoundAuthV3Vector {
    round_id_hex: String,
    ea_pk_hex: String,
    pir_layout: PirLayout,
    ballot_digest_hex: String,
    payload_hex: String,
}

fn vectors() -> Vectors {
    serde_json::from_str(VECTORS).expect("vector file parses")
}

#[test]
fn encoding_and_digest_match_every_published_vector() {
    let vectors = vectors();
    assert!(!vectors.ballot_digest_v1.is_empty());
    for vector in vectors.ballot_digest_v1 {
        assert_eq!(
            hex::encode(vector.ballot.canonical_encoding()),
            vector.encoding_hex,
            "encoding of vector {}",
            vector.name
        );
        assert_eq!(
            vector.ballot.digest().to_hex(),
            vector.digest_hex,
            "digest of vector {}",
            vector.name
        );
    }
}

#[test]
fn round_auth_v3_payload_matches_published_vector() {
    for vector in vectors().round_auth_v3 {
        let payload = RoundAuthPayloadV3::new(
            hex::decode(vector.round_id_hex)
                .unwrap()
                .try_into()
                .unwrap(),
            hex::decode(vector.ea_pk_hex).unwrap().try_into().unwrap(),
            vector.pir_layout,
            hex::decode(vector.ballot_digest_hex)
                .unwrap()
                .try_into()
                .unwrap(),
        );
        assert_eq!(hex::encode(payload.to_bytes()), vector.payload_hex);
    }
}

/// A named edit to one displayed ballot field.
type BallotEdit = (&'static str, fn(&mut Ballot));

#[test]
fn every_displayed_field_changes_the_digest() {
    let original = full_ballot().digest();
    let edits: Vec<BallotEdit> = vec![
        ("round title", |b| b.title.push('!')),
        ("round description", |b| b.description.push('!')),
        ("discussion url", |b| b.discussion_url.push('x')),
        ("proposal id", |b| b.proposals[1].id = 3),
        ("proposal title", |b| b.proposals[0].title.push('!')),
        ("proposal description", |b| {
            b.proposals[0].description.push('!')
        }),
        ("zip number", |b| b.proposals[0].zip_number.push('0')),
        ("forum url", |b| b.proposals[0].forum_url.push('x')),
        ("option label", |b| {
            b.proposals[0].options[1].label.push('!')
        }),
        ("option description", |b| {
            b.proposals[0].options[1].description.push('!')
        }),
        ("option index", |b| b.proposals[0].options[2].index = 5),
        ("option order", |b| b.proposals[0].options.swap(0, 1)),
        ("proposal order", |b| b.proposals.swap(0, 1)),
        ("dropped option", |b| {
            b.proposals[0].options.pop();
        }),
    ];
    for (field, edit) in edits {
        let mut edited = full_ballot();
        edit(&mut edited);
        assert_ne!(
            edited.digest(),
            original,
            "editing the {field} must change the digest"
        );
    }
}

#[test]
fn length_prefixes_keep_adjacent_fields_apart() {
    let mut first = full_ballot();
    first.title = "ab".to_string();
    first.description = "c".to_string();
    let mut second = full_ballot();
    second.title = "a".to_string();
    second.description = "bc".to_string();
    assert_ne!(first.digest(), second.digest());
}

#[test]
fn absent_text_fields_deserialize_as_empty() {
    let ballot: Ballot = serde_json::from_value(serde_json::json!({
        "proposals": [{
            "id": 1,
            "title": "Q",
            "options": [{"index": 0, "label": "Yes"}, {"index": 1, "label": "No"}]
        }]
    }))
    .unwrap();
    let vectors = vectors();
    let empty_metadata = vectors
        .ballot_digest_v1
        .iter()
        .find(|vector| vector.name == "empty_round_metadata_single_proposal")
        .unwrap();
    assert_eq!(ballot.digest().to_hex(), empty_metadata.digest_hex);
}

#[test]
fn the_vote_servers_round_object_deserializes_with_zero_values_omitted() {
    // Shape of `round` in `GET /shielded-vote/v1/round/{round_id}`: Go's
    // encoding/json with `omitempty`, so `index: 0` and empty strings are
    // absent, and non-ballot fields are present.
    let served: Ballot = serde_json::from_value(serde_json::json!({
        "vote_round_id": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
        "snapshot_height": 3000000,
        "status": 1,
        "title": "Round 1",
        "description": "Two questions.",
        "discussion_url": "https://forum.example.org/t/1",
        "proposals": [
            {
                "id": 1,
                "title": "Adopt ZIP-1",
                "description": "First question.",
                "zip_number": "ZIP-1",
                "forum_url": "https://forum.example.org/t/2",
                "options": [
                    {"label": "Support", "description": "In favor"},
                    {"index": 1, "label": "Oppose"},
                    {"index": 2, "label": "Abstain"}
                ]
            },
            {
                "id": 2,
                "title": "Second",
                "options": [{"label": "A"}, {"index": 1, "label": "B"}]
            }
        ]
    }))
    .unwrap();
    assert_eq!(served, full_ballot());
}

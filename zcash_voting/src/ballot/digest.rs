//! Canonical ballot encoding (`zcash-shielded-vote:ballot:v1`).
//!
//! ```text
//! domain
//!   || str(title) || str(description) || str(discussion_url)
//!   || u64(proposal count)
//!   || for each proposal:
//!        u32(id) || str(title) || str(description) || str(zip_number)
//!        || str(forum_url) || u64(option count)
//!        || for each option: u32(index) || str(label) || str(description)
//!
//! str(s) = u64(byte length of s) || UTF-8 bytes of s
//! ```
//!
//! Integers are little-endian. Every variable-length item is prefixed with its
//! length or count, so no two distinct ballots share an encoding.

use sha2::{Digest, Sha256};

use super::{Ballot, BallotDigest};

const BALLOT_DOMAIN_TAG_V1: &[u8] = b"zcash-shielded-vote:ballot:v1";

impl Ballot {
    /// Returns the canonical bytes hashed by [`Ballot::digest`].
    pub fn canonical_encoding(&self) -> Vec<u8> {
        let mut encoding = BALLOT_DOMAIN_TAG_V1.to_vec();
        push_str(&mut encoding, &self.title);
        push_str(&mut encoding, &self.description);
        push_str(&mut encoding, &self.discussion_url);
        push_count(&mut encoding, self.proposals.len());
        for proposal in &self.proposals {
            encoding.extend_from_slice(&proposal.id.to_le_bytes());
            push_str(&mut encoding, &proposal.title);
            push_str(&mut encoding, &proposal.description);
            push_str(&mut encoding, &proposal.zip_number);
            push_str(&mut encoding, &proposal.forum_url);
            push_count(&mut encoding, proposal.options.len());
            for option in &proposal.options {
                encoding.extend_from_slice(&option.index.to_le_bytes());
                push_str(&mut encoding, &option.label);
                push_str(&mut encoding, &option.description);
            }
        }
        encoding
    }

    /// SHA-256 of [`Ballot::canonical_encoding`].
    pub fn digest(&self) -> BallotDigest {
        BallotDigest(Sha256::digest(self.canonical_encoding()).into())
    }
}

fn push_count(encoding: &mut Vec<u8>, count: usize) {
    // usize is at most 64 bits on every supported target.
    encoding.extend_from_slice(&(count as u64).to_le_bytes());
}

fn push_str(encoding: &mut Vec<u8>, text: &str) {
    push_count(encoding, text.len());
    encoding.extend_from_slice(text.as_bytes());
}

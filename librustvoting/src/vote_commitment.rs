use crate::types::{
    validate_encrypted_shares, validate_vote_decision, EncryptedShare, SharePayload,
    VoteCommitmentBundle, VotingError,
};

/// Build payloads for helper server (one per share).
///
/// Each payload contains the encrypted share data plus metadata the helper
/// needs to construct `MsgRevealShare`: the shares_hash (from the vote
/// commitment), proposal_id, vote_decision, and the VC tree position.
///
/// - `enc_shares`: Encrypted shares from `VoteCommitmentBundle.enc_shares`.
/// - `commitment`: The vote commitment bundle (provides shares_hash + proposal_id).
/// - `vote_decision`: The voter's choice (0=support, 1=oppose, 2=skip).
/// - `vc_tree_position`: Position of the Vote Commitment leaf in the VC tree,
///   known after the cast-vote TX is confirmed on chain.
pub fn build_share_payloads(
    enc_shares: &[EncryptedShare],
    commitment: &VoteCommitmentBundle,
    vote_decision: u32,
    vc_tree_position: u64,
) -> Result<Vec<SharePayload>, VotingError> {
    validate_encrypted_shares(enc_shares)?;
    validate_vote_decision(vote_decision)?;

    let all_enc_shares: Vec<EncryptedShare> = enc_shares.to_vec();

    let mut payloads = Vec::with_capacity(enc_shares.len());
    for share in enc_shares {
        payloads.push(SharePayload {
            shares_hash: commitment.shares_hash.clone(),
            proposal_id: commitment.proposal_id,
            vote_decision,
            enc_share: share.clone(),
            tree_position: vc_tree_position,
            all_enc_shares: all_enc_shares.clone(),
        });
    }

    Ok(payloads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_enc_shares() -> Vec<EncryptedShare> {
        vec![
            EncryptedShare {
                c1: vec![0xC1; 32],
                c2: vec![0xC2; 32],
                share_index: 0,
                plaintext_value: 1,
                randomness: vec![0u8; 32],
            },
            EncryptedShare {
                c1: vec![0xC1; 32],
                c2: vec![0xC2; 32],
                share_index: 1,
                plaintext_value: 4,
                randomness: vec![0u8; 32],
            },
        ]
    }

    fn mock_commitment() -> VoteCommitmentBundle {
        VoteCommitmentBundle {
            van_nullifier: vec![0xAA; 32],
            vote_authority_note_new: vec![0xBB; 32],
            vote_commitment: vec![0xCC; 32],
            proposal_id: 1,
            proof: vec![0xAB; 256],
            enc_shares: vec![],
            anchor_height: 0,
            vote_round_id: String::new(),
            shares_hash: vec![0xDD; 32],
        }
    }

    #[test]
    fn test_build_share_payloads() {
        let commitment = mock_commitment();
        let result = build_share_payloads(&mock_enc_shares(), &commitment, 1, 42).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].proposal_id, 1);
        assert_eq!(result[0].vote_decision, 1);
        assert_eq!(result[0].tree_position, 42);
        assert_eq!(result[0].shares_hash, commitment.shares_hash);
        assert_eq!(result[0].enc_share.share_index, 0);
        assert_eq!(result[1].enc_share.share_index, 1);
    }
}

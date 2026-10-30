#[allow(unused_imports)]
pub(crate) use crate::backend::orchard;
use orchard::bundle::BundleVersion;
use orchard::note::NoteVersion;
use zcash_protocol::consensus::{BlockHeight, BranchId, Parameters};

use crate::types::VotingError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VotingShieldedProtocol {
    Ironwood,
}

impl VotingShieldedProtocol {
    pub(crate) fn for_branch_id(branch_id: BranchId) -> Result<Self, VotingError> {
        if matches!(branch_id, BranchId::Nu6_3 | BranchId::Nu7) {
            return Ok(Self::Ironwood);
        }

        Err(VotingError::InvalidInput {
            message: "zcash voting only supports Ironwood / NU6.3 or later shielded voting notes"
                .to_string(),
        })
    }

    pub(crate) fn for_height<P: Parameters>(
        params: &P,
        height: BlockHeight,
    ) -> Result<Self, VotingError> {
        Self::for_branch_id(BranchId::for_height(params, height))
    }

    pub(crate) fn bundle_version(self) -> BundleVersion {
        match self {
            Self::Ironwood => BundleVersion::ironwood_v3(),
        }
    }

    pub(crate) fn note_version(self) -> NoteVersion {
        match self {
            Self::Ironwood => NoteVersion::V3,
        }
    }

    pub(crate) fn pool(self) -> &'static str {
        match self {
            Self::Ironwood => "ironwood",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ironwood => "Ironwood",
        }
    }
}

// The protocol enum is crate-private, so a `tests/` sibling cannot name it. A
// wording change to the facade can move these assertions outward.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nu6_3_and_nu7_ironwood_branches() {
        for branch_id in [BranchId::Nu6_3, BranchId::Nu7] {
            let protocol = VotingShieldedProtocol::for_branch_id(branch_id).unwrap();
            assert_eq!(protocol, VotingShieldedProtocol::Ironwood);
            assert_eq!(protocol.bundle_version(), BundleVersion::ironwood_v3());
            assert_eq!(protocol.note_version(), NoteVersion::V3);
        }

        for branch_id in [BranchId::Nu6_2, BranchId::Sapling] {
            let err = VotingShieldedProtocol::for_branch_id(branch_id).unwrap_err();
            assert!(
                matches!(err, VotingError::InvalidInput { .. }),
                "unexpected error for {branch_id:?}: {err:?}"
            );
        }
    }
}

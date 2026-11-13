#[allow(unused_imports)]
pub(crate) use crate::backend::orchard;
use orchard::bundle::BundleVersion;
use orchard::note::NoteVersion;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};

use crate::types::VotingError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VotingShieldedProtocol {
    Ironwood,
}

impl VotingShieldedProtocol {
    /// Selects Ironwood V3 once NU6.3 is active on the snapshot's network.
    ///
    /// Later branch IDs do not change eligibility. This does not add support
    /// for changes to the note format, commitment tree, or voting circuit.
    pub(crate) fn for_height<P: Parameters>(params: &P, height: u64) -> Result<Self, VotingError> {
        let height = u32::try_from(height)
            .map(BlockHeight::from_u32)
            .map_err(|_| VotingError::InvalidInput {
                message: format!("snapshot height {height} does not fit in u32"),
            })?;
        if params.is_nu_active(NetworkUpgrade::Nu6_3, height) {
            return Ok(Self::Ironwood);
        }

        Err(VotingError::InvalidInput {
            message: format!(
                "snapshot height {height} is before NU6.3/Ironwood activation or activation is not configured for {:?}",
                params.network_type()
            ),
        })
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

#[cfg(test)]
mod tests {
    mod branch_selection;
}

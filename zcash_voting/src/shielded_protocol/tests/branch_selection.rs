use super::super::*;
use crate::types::Network;
use zcash_protocol::consensus::{BranchId, NetworkType};

/// Eligibility includes activation and remains limited to the existing V3 profile.
#[test]
fn eligibility_follows_ironwood_activation_on_each_network() {
    for network in [Network::Mainnet, Network::Testnet, Network::Regtest] {
        let activation = u64::from(u32::from(
            network.activation_height(NetworkUpgrade::Nu6_3).unwrap(),
        ));
        assert!(matches!(
            VotingShieldedProtocol::for_height(&network, activation - 1),
            Err(VotingError::InvalidInput { message })
                if message.contains("before NU6.3/Ironwood activation")
        ));
        for height in [activation, activation + 1, u64::from(u32::MAX)] {
            let protocol = VotingShieldedProtocol::for_height(&network, height).unwrap();
            assert_eq!(protocol, VotingShieldedProtocol::Ironwood);
            assert_eq!(protocol.bundle_version(), BundleVersion::ironwood_v3());
            assert_eq!(protocol.note_version(), NoteVersion::V3);
        }
    }
}

/// Every compiled upgrade boundary retains the old snapshot eligibility result.
#[test]
fn known_upgrade_boundaries_keep_the_same_eligibility() {
    for network in [Network::Mainnet, Network::Testnet, Network::Regtest] {
        for upgrade in [
            NetworkUpgrade::Overwinter,
            NetworkUpgrade::Sapling,
            NetworkUpgrade::Blossom,
            NetworkUpgrade::Heartwood,
            NetworkUpgrade::Canopy,
            NetworkUpgrade::Nu5,
            NetworkUpgrade::Nu6,
            NetworkUpgrade::Nu6_1,
            NetworkUpgrade::Nu6_2,
            NetworkUpgrade::Nu6_3,
            NetworkUpgrade::Nu7,
        ] {
            if let Some(activation) = network.activation_height(upgrade) {
                let activation = u32::from(activation);
                for height in [activation - 1, activation, activation + 1] {
                    let was_eligible = matches!(
                        BranchId::for_height(&network, BlockHeight::from_u32(height)),
                        BranchId::Nu6_3 | BranchId::Nu7
                    );
                    assert_eq!(
                        VotingShieldedProtocol::for_height(&network, u64::from(height)).is_ok(),
                        was_eligible,
                        "{network:?} at {height}"
                    );
                }
            }
        }
    }
}

/// A parameter source that exposes only the capability voting actually needs.
#[derive(Clone)]
struct IronwoodActivation(Option<BlockHeight>);

impl Parameters for IronwoodActivation {
    /// Identifies this synthetic network as regtest.
    fn network_type(&self) -> NetworkType {
        NetworkType::Regtest
    }

    /// Fails the test if selection tries to discover any other upgrade.
    fn activation_height(&self, upgrade: NetworkUpgrade) -> Option<BlockHeight> {
        assert_eq!(upgrade, NetworkUpgrade::Nu6_3);
        self.0
    }
}

/// Later upgrade metadata is unnecessary, but missing Ironwood activation fails closed.
#[test]
fn eligibility_does_not_consult_later_upgrade_schedule() {
    let height = u64::from(u32::MAX);
    assert!(VotingShieldedProtocol::for_height(
        &IronwoodActivation(Some(BlockHeight::from_u32(10))),
        height,
    )
    .is_ok());
    assert!(matches!(
        VotingShieldedProtocol::for_height(&IronwoodActivation(None), height),
        Err(VotingError::InvalidInput { message })
            if message.contains("activation is not configured")
    ));
}

/// Out-of-range heights remain distinct from an inactive shielded pool.
#[test]
fn overflowing_snapshot_height_is_rejected() {
    assert!(matches!(
        VotingShieldedProtocol::for_height(&Network::Regtest, u64::from(u32::MAX) + 1),
        Err(VotingError::InvalidInput { message }) if message.contains("does not fit in u32")
    ));
}

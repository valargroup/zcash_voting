import ComposableArchitecture
import Foundation
import VotingModels

extension VotingCryptoClient: DependencyKey {
    public static var liveValue: Self {
        Self(
            generateHotkey: {
                // Stub: return a hardcoded key pair
                VotingHotkey(
                    secretKey: Data(repeating: 0xAA, count: 32),
                    publicKey: Data(repeating: 0xBB, count: 32),
                    address: "zs1voting7qk4hs9xd3nfw8yj6m2r0ekrl...a8e2"
                )
            },
            constructDelegationAction: { _, _, _ in
                try await Task.sleep(for: .milliseconds(100))
                return DelegationAction(
                    actionBytes: Data(repeating: 0x01, count: 64),
                    rk: Data(repeating: 0x02, count: 32),
                    sighash: Data(repeating: 0x03, count: 32)
                )
            },
            buildDelegationWitness: { _, _, _ in
                try await Task.sleep(for: .milliseconds(200))
                return Data(repeating: 0x04, count: 128)
            },
            generateDelegationProof: { _ in
                // Stub: emit progress over ~4s, then yield mock proof
                AsyncThrowingStream { continuation in
                    Task {
                        for step in 1...8 {
                            try await Task.sleep(for: .milliseconds(500))
                            continuation.yield(.progress(Double(step) / 8.0))
                        }
                        continuation.yield(.completed(Data(repeating: 0x05, count: 64)))
                        continuation.finish()
                    }
                }
            },
            decomposeWeight: { weight in
                // Binary decomposition into power-of-2 shares
                var shares: [UInt64] = []
                var remaining = weight
                var bit: UInt64 = 1
                while remaining > 0 {
                    if remaining & 1 == 1 {
                        shares.append(bit)
                    }
                    remaining >>= 1
                    bit <<= 1
                }
                return shares
            },
            encryptShares: { shares, _ in
                try await Task.sleep(for: .milliseconds(100))
                return shares.enumerated().map { index, value in
                    EncryptedShare(
                        c1: Data(repeating: UInt8(index & 0xFF), count: 32),
                        c2: Data(repeating: UInt8((index + 1) & 0xFF), count: 32),
                        shareIndex: UInt32(index),
                        plaintextValue: value
                    )
                }
            },
            buildVoteCommitment: { proposalId, _, _, _ in
                // Stub: emit progress over ~2s, then yield mock bundle
                AsyncThrowingStream { continuation in
                    Task {
                        for step in 1...4 {
                            try await Task.sleep(for: .milliseconds(100))
                            continuation.yield(.progress(Double(step) / 4.0))
                        }
                        let bundle = VoteCommitmentBundle(
                            vanNullifier: Data(repeating: 0x10, count: 32),
                            voteAuthorityNoteNew: Data(repeating: 0x11, count: 32),
                            voteCommitment: Data(repeating: 0x12, count: 32),
                            proposalId: proposalId,
                            proof: Data(repeating: 0x13, count: 64),
                            voteRoundId: Data(repeating: 0, count: 32),
                            voteCommTreeAnchorHeight: 0
                        )
                        continuation.yield(.completed(bundle.proof))
                        continuation.finish()
                    }
                }
            },
            buildSharePayloads: { encShares, commitment in
                try await Task.sleep(for: .milliseconds(50))
                return encShares.map { share in
                    SharePayload(
                        sharesHash: Data(repeating: 0x20, count: 32),
                        proposalId: commitment.proposalId,
                        voteDecision: 0,
                        encShare: share,
                        shareIndex: share.shareIndex,
                        treePosition: UInt64(share.shareIndex) * 100
                    )
                }
            }
        )
    }
}

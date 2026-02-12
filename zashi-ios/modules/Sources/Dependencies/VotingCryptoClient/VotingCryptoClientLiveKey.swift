import ComposableArchitecture
import Foundation
import VotingModels
import ZcashVotingFFI

extension VotingCryptoClient: DependencyKey {
    public static var liveValue: Self {
        Self(
            generateHotkey: {
                let hotkey = try ZcashVotingFFI.generateHotkey()
                return VotingHotkey(
                    secretKey: hotkey.secretKey,
                    publicKey: hotkey.publicKey,
                    address: hotkey.address
                )
            },
            constructDelegationAction: { hotkey, notes, params in
                let ffiHotkey = ZcashVotingFFI.VotingHotkey(
                    secretKey: hotkey.secretKey,
                    publicKey: hotkey.publicKey,
                    address: hotkey.address
                )
                let ffiNotes = notes.map {
                    ZcashVotingFFI.NoteInfo(
                        commitment: $0.commitment,
                        nullifier: $0.nullifier,
                        value: $0.value,
                        position: $0.position
                    )
                }
                let ffiParams = ZcashVotingFFI.VotingRoundParams(
                    voteRoundId: params.voteRoundId.hexString,
                    snapshotHeight: params.snapshotHeight,
                    eaPk: params.eaPK,
                    ncRoot: params.ncRoot,
                    nullifierImtRoot: params.nullifierIMTRoot
                )
                let result = try ZcashVotingFFI.constructDelegationAction(
                    hotkey: ffiHotkey,
                    notes: ffiNotes,
                    params: ffiParams
                )
                return DelegationAction(
                    actionBytes: result.actionBytes,
                    rk: result.rk,
                    sighash: result.sighash
                )
            },
            buildDelegationWitness: { action, inclusionProofs, exclusionProofs in
                let ffiAction = ZcashVotingFFI.DelegationAction(
                    actionBytes: action.actionBytes,
                    rk: action.rk,
                    sighash: action.sighash
                )
                return try ZcashVotingFFI.buildDelegationWitness(
                    action: ffiAction,
                    inclusionProofs: inclusionProofs,
                    exclusionProofs: exclusionProofs
                )
            },
            generateDelegationProof: { witness in
                // Progress simulation stays in Swift; the actual proof call goes to Rust.
                // When real ZKP generation lands (~4-8s), we'll replace this with
                // callback-based progress or polling.
                AsyncThrowingStream { continuation in
                    Task {
                        for step in 1...7 {
                            try await Task.sleep(for: .milliseconds(500))
                            continuation.yield(.progress(Double(step) / 8.0))
                        }
                        let result = try ZcashVotingFFI.generateDelegationProof(witness: witness)
                        guard result.success else {
                            continuation.finish(throwing: VotingCryptoError.proofFailed(
                                result.error ?? "unknown"
                            ))
                            return
                        }
                        continuation.yield(.completed(result.proof))
                        continuation.finish()
                    }
                }
            },
            decomposeWeight: { weight in
                ZcashVotingFFI.decomposeWeight(weight: weight)
            },
            encryptShares: { shares, eaPK in
                let ffiShares = try ZcashVotingFFI.encryptShares(shares: shares, eaPk: eaPK)
                return ffiShares.map {
                    EncryptedShare(
                        c1: $0.c1,
                        c2: $0.c2,
                        shareIndex: $0.shareIndex,
                        plaintextValue: $0.plaintextValue
                    )
                }
            },
            buildVoteCommitment: { proposalId, choice, encShares, vanWitness in
                // Progress simulation stays in Swift; actual proof call goes to Rust.
                AsyncThrowingStream { continuation in
                    Task {
                        for step in 1...3 {
                            try await Task.sleep(for: .milliseconds(100))
                            continuation.yield(.progress(Double(step) / 4.0))
                        }
                        let ffiShares = encShares.map {
                            ZcashVotingFFI.EncryptedShare(
                                c1: $0.c1,
                                c2: $0.c2,
                                shareIndex: $0.shareIndex,
                                plaintextValue: $0.plaintextValue
                            )
                        }
                        let result = try ZcashVotingFFI.buildVoteCommitment(
                            proposalId: String(proposalId),
                            choice: choice.ffiValue,
                            encShares: ffiShares,
                            vanWitness: vanWitness
                        )
                        // Map back to VotingModels type (includes extra fields for chain submission)
                        let bundle = VoteCommitmentBundle(
                            vanNullifier: result.vanNullifier,
                            voteAuthorityNoteNew: result.voteAuthorityNoteNew,
                            voteCommitment: result.voteCommitment,
                            proposalId: proposalId,
                            proof: result.proof,
                            voteRoundId: Data(repeating: 0, count: 32),
                            voteCommTreeAnchorHeight: 0
                        )
                        continuation.yield(.completed(bundle.proof))
                        continuation.finish()
                    }
                }
            },
            buildSharePayloads: { encShares, commitment in
                let ffiShares = encShares.map {
                    ZcashVotingFFI.EncryptedShare(
                        c1: $0.c1,
                        c2: $0.c2,
                        shareIndex: $0.shareIndex,
                        plaintextValue: $0.plaintextValue
                    )
                }
                let ffiCommitment = ZcashVotingFFI.VoteCommitmentBundle(
                    vanNullifier: commitment.vanNullifier,
                    voteAuthorityNoteNew: commitment.voteAuthorityNoteNew,
                    voteCommitment: commitment.voteCommitment,
                    proposalId: String(commitment.proposalId),
                    proof: commitment.proof
                )
                let ffiPayloads = try ZcashVotingFFI.buildSharePayloads(
                    encShares: ffiShares,
                    commitment: ffiCommitment
                )
                return ffiPayloads.map {
                    SharePayload(
                        sharesHash: $0.sharesHash,
                        proposalId: commitment.proposalId,
                        voteDecision: $0.voteDecision,
                        encShare: EncryptedShare(
                            c1: $0.encShare.c1,
                            c2: $0.encShare.c2,
                            shareIndex: $0.encShare.shareIndex,
                            plaintextValue: $0.encShare.plaintextValue
                        ),
                        shareIndex: $0.encShare.shareIndex,
                        treePosition: $0.treePosition
                    )
                }
            }
        )
    }
}

// MARK: - Helpers

enum VotingCryptoError: Error {
    case proofFailed(String)
}

private extension VoteChoice {
    var ffiValue: UInt32 {
        switch self {
        case .support: return 0
        case .oppose: return 1
        case .skip: return 2
        }
    }
}

private extension Data {
    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}

import ComposableArchitecture
import Foundation
import VotingModels

extension DependencyValues {
    public var votingCrypto: VotingCryptoClient {
        get { self[VotingCryptoClient.self] }
        set { self[VotingCryptoClient.self] = newValue }
    }
}

@DependencyClient
public struct VotingCryptoClient {
    public var generateHotkey: @Sendable () async throws -> VotingHotkey
    public var constructDelegationAction: @Sendable (
        _ hotkey: VotingHotkey,
        _ notes: [NoteInfo],
        _ params: VotingRoundParams
    ) async throws -> DelegationAction
    public var buildDelegationWitness: @Sendable (
        _ action: DelegationAction,
        _ inclusionProofs: [Data],
        _ exclusionProofs: [Data]
    ) async throws -> Data
    public var generateDelegationProof: @Sendable (_ witness: Data) -> AsyncThrowingStream<ProofEvent, Error>
        = { _ in AsyncThrowingStream { $0.finish() } }
    public var decomposeWeight: @Sendable (_ weight: UInt64) -> [UInt64] = { _ in [] }
    public var encryptShares: @Sendable (
        _ shares: [UInt64],
        _ eaPK: Data
    ) async throws -> [EncryptedShare]
    public var buildVoteCommitment: @Sendable (
        _ proposalId: String,
        _ choice: VoteChoice,
        _ encShares: [EncryptedShare],
        _ vanWitness: Data
    ) -> AsyncThrowingStream<ProofEvent, Error>
        = { _, _, _, _ in AsyncThrowingStream { $0.finish() } }
    public var buildSharePayloads: @Sendable (
        _ encShares: [EncryptedShare],
        _ commitment: VoteCommitmentBundle
    ) async throws -> [SharePayload]
}

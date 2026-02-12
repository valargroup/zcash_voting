import ComposableArchitecture
import Foundation
import VotingModels

extension DependencyValues {
    public var votingStorage: VotingStorageClient {
        get { self[VotingStorageClient.self] }
        set { self[VotingStorageClient.self] = newValue }
    }
}

@DependencyClient
public struct VotingStorageClient {
    public var storeHotkey: @Sendable (_ roundId: String, _ hotkey: VotingHotkey) async throws -> Void
    public var loadHotkey: @Sendable (_ roundId: String) async -> VotingHotkey?
    public var storeDelegation: @Sendable (_ roundId: String, _ registration: DelegationRegistration) async throws -> Void
    public var loadDelegation: @Sendable (_ roundId: String) async -> DelegationRegistration?
    public var storeSession: @Sendable (_ session: VotingSession) async throws -> Void
    public var loadSession: @Sendable (_ roundId: String) async -> VotingSession?
    public var clearRound: @Sendable (_ roundId: String) async -> Void
}

import ComposableArchitecture
import Foundation
import VotingModels

private actor VotingStore {
    var hotkeys: [String: VotingHotkey] = [:]
    var delegations: [String: DelegationRegistration] = [:]
    var sessions: [String: VotingSession] = [:]

    func storeHotkey(roundId: String, hotkey: VotingHotkey) {
        hotkeys[roundId] = hotkey
    }

    func loadHotkey(roundId: String) -> VotingHotkey? {
        hotkeys[roundId]
    }

    func storeDelegation(roundId: String, registration: DelegationRegistration) {
        delegations[roundId] = registration
    }

    func loadDelegation(roundId: String) -> DelegationRegistration? {
        delegations[roundId]
    }

    func storeSession(_ session: VotingSession) {
        sessions[session.voteRoundId] = session
    }

    func loadSession(roundId: String) -> VotingSession? {
        sessions[roundId]
    }

    func clearRound(roundId: String) {
        hotkeys.removeValue(forKey: roundId)
        delegations.removeValue(forKey: roundId)
        sessions.removeValue(forKey: roundId)
    }
}

extension VotingStorageClient: DependencyKey {
    public static var liveValue: Self {
        let store = VotingStore()

        return Self(
            storeHotkey: { roundId, hotkey in
                await store.storeHotkey(roundId: roundId, hotkey: hotkey)
            },
            loadHotkey: { roundId in
                await store.loadHotkey(roundId: roundId)
            },
            storeDelegation: { roundId, registration in
                await store.storeDelegation(roundId: roundId, registration: registration)
            },
            loadDelegation: { roundId in
                await store.loadDelegation(roundId: roundId)
            },
            storeSession: { session in
                await store.storeSession(session)
            },
            loadSession: { roundId in
                await store.loadSession(roundId: roundId)
            },
            clearRound: { roundId in
                await store.clearRound(roundId: roundId)
            }
        )
    }
}

import Foundation

// MARK: - Session & Round

public struct VotingSession: Equatable, Sendable {
    public let voteRoundId: String
    public let snapshotHeight: UInt64
    public let snapshotBlockhash: Data
    public let proposalsHash: Data
    public let voteEndTime: Date
    public let eaPK: Data
    public let vkZkp1: Data
    public let vkZkp2: Data
    public let vkZkp3: Data
    public let ncRoot: Data
    public let nullifierIMTRoot: Data
    public let proposals: [Proposal]
    public let status: SessionStatus

    public init(
        voteRoundId: String,
        snapshotHeight: UInt64,
        snapshotBlockhash: Data,
        proposalsHash: Data,
        voteEndTime: Date,
        eaPK: Data,
        vkZkp1: Data,
        vkZkp2: Data,
        vkZkp3: Data,
        ncRoot: Data,
        nullifierIMTRoot: Data,
        proposals: [Proposal],
        status: SessionStatus
    ) {
        self.voteRoundId = voteRoundId
        self.snapshotHeight = snapshotHeight
        self.snapshotBlockhash = snapshotBlockhash
        self.proposalsHash = proposalsHash
        self.voteEndTime = voteEndTime
        self.eaPK = eaPK
        self.vkZkp1 = vkZkp1
        self.vkZkp2 = vkZkp2
        self.vkZkp3 = vkZkp3
        self.ncRoot = ncRoot
        self.nullifierIMTRoot = nullifierIMTRoot
        self.proposals = proposals
        self.status = status
    }
}

public enum SessionStatus: Equatable, Sendable {
    case active
    case tallying
    case finalized
}

public struct VotingRoundParams: Equatable, Sendable {
    public let voteRoundId: String
    public let snapshotHeight: UInt64
    public let eaPK: Data
    public let ncRoot: Data
    public let nullifierIMTRoot: Data

    public init(
        voteRoundId: String,
        snapshotHeight: UInt64,
        eaPK: Data,
        ncRoot: Data,
        nullifierIMTRoot: Data
    ) {
        self.voteRoundId = voteRoundId
        self.snapshotHeight = snapshotHeight
        self.eaPK = eaPK
        self.ncRoot = ncRoot
        self.nullifierIMTRoot = nullifierIMTRoot
    }
}

// MARK: - Hotkey

public struct VotingHotkey: Equatable, Sendable {
    public let secretKey: Data
    public let publicKey: Data
    public let address: String

    public init(secretKey: Data, publicKey: Data, address: String) {
        self.secretKey = secretKey
        self.publicKey = publicKey
        self.address = address
    }
}

// MARK: - Delegation

public struct DelegationAction: Equatable, Sendable {
    public let actionBytes: Data
    public let rk: Data
    public let sighash: Data

    public init(actionBytes: Data, rk: Data, sighash: Data) {
        self.actionBytes = actionBytes
        self.rk = rk
        self.sighash = sighash
    }
}

public struct DelegationRegistration: Equatable, Sendable {
    public let rk: Data
    public let spendAuthSig: Data
    public let proof: Data

    public init(rk: Data, spendAuthSig: Data, proof: Data) {
        self.rk = rk
        self.spendAuthSig = spendAuthSig
        self.proof = proof
    }
}

// MARK: - Voting

public struct EncryptedShare: Equatable, Sendable {
    public let c1: Data
    public let c2: Data
    public let shareIndex: UInt32
    public let plaintextValue: UInt64

    public init(c1: Data, c2: Data, shareIndex: UInt32, plaintextValue: UInt64) {
        self.c1 = c1
        self.c2 = c2
        self.shareIndex = shareIndex
        self.plaintextValue = plaintextValue
    }
}

public struct VoteCommitmentBundle: Equatable, Sendable {
    public let vanNullifier: Data
    public let voteAuthorityNoteNew: Data
    public let voteCommitment: Data
    public let proposalId: String
    public let proof: Data

    public init(vanNullifier: Data, voteAuthorityNoteNew: Data, voteCommitment: Data, proposalId: String, proof: Data) {
        self.vanNullifier = vanNullifier
        self.voteAuthorityNoteNew = voteAuthorityNoteNew
        self.voteCommitment = voteCommitment
        self.proposalId = proposalId
        self.proof = proof
    }
}

public struct SharePayload: Equatable, Sendable {
    public let sharesHash: Data
    public let proposalId: String
    public let voteDecision: UInt32
    public let encShare: EncryptedShare
    public let shareIndex: UInt32
    public let treePosition: UInt64

    public init(sharesHash: Data, proposalId: String, voteDecision: UInt32, encShare: EncryptedShare, shareIndex: UInt32, treePosition: UInt64) {
        self.sharesHash = sharesHash
        self.proposalId = proposalId
        self.voteDecision = voteDecision
        self.encShare = encShare
        self.shareIndex = shareIndex
        self.treePosition = treePosition
    }
}

// MARK: - Tree & Transactions

public struct CommitmentTreeState: Equatable, Sendable {
    public let nextIndex: UInt64
    public let root: Data
    public let height: UInt64

    public init(nextIndex: UInt64, root: Data, height: UInt64) {
        self.nextIndex = nextIndex
        self.root = root
        self.height = height
    }
}

public struct TxResult: Equatable, Sendable {
    public let txHash: String
    public let code: UInt32

    public init(txHash: String, code: UInt32) {
        self.txHash = txHash
        self.code = code
    }
}

public struct TallyResult: Equatable, Sendable {
    public struct Entry: Equatable, Sendable {
        public let decision: UInt32
        public let amount: UInt64

        public init(decision: UInt32, amount: UInt64) {
            self.decision = decision
            self.amount = amount
        }
    }

    public let entries: [Entry]

    public init(entries: [Entry]) {
        self.entries = entries
    }
}

// MARK: - Notes

public struct NoteInfo: Equatable, Sendable {
    public let commitment: Data
    public let nullifier: Data
    public let value: UInt64
    public let position: UInt64

    public init(commitment: Data, nullifier: Data, value: UInt64, position: UInt64) {
        self.commitment = commitment
        self.nullifier = nullifier
        self.value = value
        self.position = position
    }
}

// MARK: - Proof Events

public enum ProofEvent: Equatable, Sendable {
    case progress(Double)
    case completed(Data)
}

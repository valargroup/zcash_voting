public enum VoteChoice: Equatable, Hashable, Codable {
    case support
    case oppose
    case skip

    public var label: String {
        switch self {
        case .support: return "Support"
        case .oppose: return "Oppose"
        case .skip: return "Skipped"
        }
    }
}

import Foundation

public struct Proposal: Equatable, Identifiable, Sendable {
    public let id: String
    public let title: String
    public let description: String
    public let zipNumber: String?
    public let forumURL: URL?

    public init(
        id: String,
        title: String,
        description: String,
        zipNumber: String? = nil,
        forumURL: URL? = nil
    ) {
        self.id = id
        self.title = title
        self.description = description
        self.zipNumber = zipNumber
        self.forumURL = forumURL
    }
}

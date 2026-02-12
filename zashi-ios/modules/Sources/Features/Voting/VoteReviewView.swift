import SwiftUI
import ComposableArchitecture
import Generated
import UIComponents
import VotingModels

struct VoteReviewView: View {
    @Environment(\.colorScheme) var colorScheme

    let store: StoreOf<Voting>

    var body: some View {
        WithPerceptionTracking {
            VStack(spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Text("Review Your Votes")
                            .zFont(.semiBold, size: 22, style: Design.Text.primary)
                            .padding(.top, 16)

                        // Voting weight summary
                        HStack {
                            Text("Voting weight:")
                                .zFont(.regular, size: 14, style: Design.Text.secondary)
                            Spacer()
                            Text("\(store.votingWeightZECString) ZEC")
                                .zFont(.semiBold, size: 14, style: Design.Text.primary)
                        }
                        .padding(12)
                        .background(Color.secondary.opacity(0.06))
                        .clipShape(RoundedRectangle(cornerRadius: 10))

                        // Votes table
                        ForEach(store.votingRound.proposals) { proposal in
                            let vote = store.votes[proposal.id]
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(proposal.title)
                                        .zFont(.medium, size: 14, style: Design.Text.primary)
                                        .lineLimit(1)
                                    if let zip = proposal.zipNumber {
                                        Text(zip)
                                            .font(.system(size: 11, weight: .medium, design: .monospaced))
                                            .foregroundStyle(.secondary)
                                    }
                                }

                                Spacer()

                                VoteChip(choice: vote)
                            }
                            .padding(12)
                            .background(Design.Surfaces.bgPrimary.color(colorScheme))
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                            .overlay(
                                RoundedRectangle(cornerRadius: 12)
                                    .stroke(Design.Surfaces.strokeSecondary.color(colorScheme), lineWidth: 1)
                            )
                            .onTapGesture {
                                store.send(.editVote(proposalId: proposal.id))
                            }
                        }

                        // Summary
                        HStack(spacing: 16) {
                            voteSummaryItem(
                                count: store.votes.values.filter { $0 == .support }.count,
                                label: "Support",
                                color: .green
                            )
                            voteSummaryItem(
                                count: store.votes.values.filter { $0 == .oppose }.count,
                                label: "Oppose",
                                color: .red
                            )
                            voteSummaryItem(
                                count: store.votes.values.filter { $0 == .skip }.count,
                                label: "Skipped",
                                color: .gray
                            )
                        }
                        .padding(.top, 8)

                        Spacer().frame(height: 80)
                    }
                    .screenHorizontalPadding()
                }

                // Submit button
                VStack {
                    ZashiButton("Submit Votes", type: .primary) {
                        store.send(.submitVotesTapped)
                    }
                    .padding(.horizontal, 24)
                    .padding(.vertical, 12)
                    .background {
                        LinearGradient(
                            colors: [
                                Design.Surfaces.bgPrimary.color(colorScheme).opacity(0),
                                Design.Surfaces.bgPrimary.color(colorScheme),
                            ],
                            startPoint: .top,
                            endPoint: UnitPoint(x: 0.5, y: 0.3)
                        )
                    }
                }
            }
            .navigationTitle("Review")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .navigationBarLeading) {
                    Button {
                        store.send(.goBack)
                    } label: {
                        Image(systemName: "chevron.left")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func voteSummaryItem(count: Int, label: String, color: Color) -> some View {
        VStack(spacing: 4) {
            Text("\(count)")
                .zFont(.semiBold, size: 20, style: Design.Text.primary)
            Text(label)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(color)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
        .background(color.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }
}

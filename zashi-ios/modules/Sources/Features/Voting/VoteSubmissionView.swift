import SwiftUI
import ComposableArchitecture
import Generated
import UIComponents
import VotingModels

struct VoteSubmissionView: View {
    let store: StoreOf<Voting>

    var body: some View {
        WithPerceptionTracking {
            VStack(spacing: 32) {
                Spacer()

                // Progress indicator
                switch store.submissionStatus {
                case .idle:
                    EmptyView()
                case .submitting(let index, let total):
                    VStack(spacing: 20) {
                        ProgressView()
                            .scaleEffect(1.5)

                        Text("Submitting votes...")
                            .zFont(.semiBold, size: 18, style: Design.Text.primary)

                        Text("Processing proposal \(index + 1) of \(total)")
                            .zFont(.regular, size: 14, style: Design.Text.secondary)

                        ProgressView(value: Double(index + 1), total: Double(total))
                            .tint(.green)
                            .padding(.horizontal, 40)
                    }
                case .complete:
                    completionView()
                case .failed(let error):
                    VStack(spacing: 16) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 48))
                            .foregroundStyle(.orange)

                        Text("Submission Failed")
                            .zFont(.semiBold, size: 18, style: Design.Text.primary)

                        Text(error)
                            .zFont(.regular, size: 14, style: Design.Text.secondary)
                            .multilineTextAlignment(.center)
                    }
                }

                Spacer()
            }
            .navigationBarBackButtonHidden(true)
        }
    }

    @ViewBuilder
    private func completionView() -> some View {
        VStack(spacing: 20) {
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 64))
                .foregroundStyle(.green)

            Text("Votes Submitted!")
                .zFont(.semiBold, size: 22, style: Design.Text.primary)

            Text("Your \(store.votingWeightZECString) ZEC voting weight has been applied to \(store.totalProposals) proposals.")
                .zFont(.regular, size: 15, style: Design.Text.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)

            ZashiButton("Done", type: .primary) {
                store.send(.doneTapped)
            }
            .padding(.horizontal, 40)
            .padding(.top, 16)
        }
    }
}

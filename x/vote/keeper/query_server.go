package keeper

import (
	"context"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/z-cale/zally/x/vote/types"
)

var _ types.QueryServer = queryServer{}

type queryServer struct {
	types.UnimplementedQueryServer
	k Keeper
}

// NewQueryServerImpl returns an implementation of the vote QueryServer interface.
func NewQueryServerImpl(keeper Keeper) types.QueryServer {
	return &queryServer{k: keeper}
}

// CommitmentTreeAtHeight returns the commitment tree root at a specific anchor height.
// Phase 5 will fully implement this with proper gRPC-gateway REST endpoints.
func (qs queryServer) CommitmentTreeAtHeight(_ context.Context, req *types.QueryCommitmentTreeRequest) (*types.QueryCommitmentTreeResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}

	// TODO(Phase 5): Implement query logic using keeper.GetCommitmentRootAtHeight.
	return &types.QueryCommitmentTreeResponse{
		Tree: &types.CommitmentTreeState{
			Height: req.Height,
		},
	}, nil
}

// LatestCommitmentTree returns the latest commitment tree state.
func (qs queryServer) LatestCommitmentTree(_ context.Context, req *types.QueryLatestTreeRequest) (*types.QueryLatestTreeResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}

	// TODO(Phase 5): Implement query logic using keeper.GetCommitmentTreeState.
	return &types.QueryLatestTreeResponse{
		Tree: &types.CommitmentTreeState{},
	}, nil
}

// VoteRound returns information about a specific vote round.
func (qs queryServer) VoteRound(_ context.Context, req *types.QueryVoteRoundRequest) (*types.QueryVoteRoundResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}
	if len(req.VoteRoundId) == 0 {
		return nil, status.Error(codes.InvalidArgument, "vote_round_id is required")
	}

	// TODO(Phase 5): Implement query logic using keeper.GetVoteRound.
	return &types.QueryVoteRoundResponse{
		Round: &types.VoteRound{},
	}, nil
}

// ProposalTally returns the accumulated tally for a proposal within a vote round.
func (qs queryServer) ProposalTally(_ context.Context, req *types.QueryProposalTallyRequest) (*types.QueryProposalTallyResponse, error) {
	if req == nil {
		return nil, status.Error(codes.InvalidArgument, "empty request")
	}
	if len(req.VoteRoundId) == 0 {
		return nil, status.Error(codes.InvalidArgument, "vote_round_id is required")
	}

	// TODO(Phase 5): Implement query logic using keeper.GetTally.
	return &types.QueryProposalTallyResponse{
		Tally: make(map[uint32]uint64),
	}, nil
}

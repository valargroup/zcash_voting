package keeper

import (
	"context"

	"github.com/z-cale/zally/x/vote/types"
)

var _ types.MsgServer = msgServer{}

type msgServer struct {
	types.UnimplementedMsgServer
	k Keeper
}

// NewMsgServerImpl returns an implementation of the vote MsgServer interface.
func NewMsgServerImpl(keeper Keeper) types.MsgServer {
	return &msgServer{k: keeper}
}

// SetupVoteRound handles MsgSetupVoteRound.
// Phase 4 will implement: compute vote_round_id, store VoteRound, emit event.
func (ms msgServer) SetupVoteRound(_ context.Context, _ *types.MsgSetupVoteRound) (*types.MsgSetupVoteRoundResponse, error) {
	// TODO(Phase 4): Implement keeper logic.
	return &types.MsgSetupVoteRoundResponse{}, nil
}

// RegisterDelegation handles MsgRegisterDelegation (ZKP #1).
// Phase 4 will implement: record nullifiers, append to commitment tree, emit event.
func (ms msgServer) RegisterDelegation(_ context.Context, _ *types.MsgRegisterDelegation) (*types.MsgRegisterDelegationResponse, error) {
	// TODO(Phase 4): Implement keeper logic.
	return &types.MsgRegisterDelegationResponse{}, nil
}

// CreateVoteCommitment handles MsgCreateVoteCommitment (ZKP #2).
// Phase 4 will implement: validate anchor, record nullifier, append to tree, emit event.
func (ms msgServer) CreateVoteCommitment(_ context.Context, _ *types.MsgCreateVoteCommitment) (*types.MsgCreateVoteCommitmentResponse, error) {
	// TODO(Phase 4): Implement keeper logic.
	return &types.MsgCreateVoteCommitmentResponse{}, nil
}

// RevealVoteShare handles MsgRevealVoteShare (ZKP #3).
// Phase 4 will implement: record nullifier, accumulate tally, emit event.
func (ms msgServer) RevealVoteShare(_ context.Context, _ *types.MsgRevealVoteShare) (*types.MsgRevealVoteShareResponse, error) {
	// TODO(Phase 4): Implement keeper logic.
	return &types.MsgRevealVoteShareResponse{}, nil
}

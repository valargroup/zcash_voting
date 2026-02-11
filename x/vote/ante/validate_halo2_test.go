//go:build halo2

package ante_test

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/z-cale/zally/crypto/redpallas"
	"github.com/z-cale/zally/crypto/zkp/halo2"
	"github.com/z-cale/zally/x/vote/ante"
	"github.com/z-cale/zally/x/vote/types"
)

// repoRoot returns the absolute path to the repository root by walking up
// from this test file's location (x/vote/ante/).
func repoRoot(t *testing.T) string {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	require.True(t, ok, "runtime.Caller failed")
	// thisFile = .../x/vote/ante/validate_halo2_test.go → go up 4 levels
	return filepath.Join(filepath.Dir(thisFile), "..", "..", "..")
}

// mustReadFixture reads a binary fixture from crypto/zkp/testdata/.
func mustReadFixture(t *testing.T, name string) []byte {
	t.Helper()
	path := filepath.Join(repoRoot(t), "crypto", "zkp", "testdata", name)
	data, err := os.ReadFile(path)
	require.NoError(t, err, "failed to read fixture %s", path)
	return data
}

// TestHalo2DelegationValidProof runs the full ante validation pipeline with a
// real Halo2 toy proof. The MsgRegisterDelegation.Proof carries the real proof
// bytes and Rk carries the 32-byte public input.
func TestHalo2DelegationValidProof(t *testing.T) {
	proof := mustReadFixture(t, "toy_valid_proof.bin")
	publicInput := mustReadFixture(t, "toy_valid_input.bin")

	// Build a MsgRegisterDelegation with the real proof and public input as Rk.
	msg := &types.MsgRegisterDelegation{
		Rk:                  publicInput, // 32-byte toy circuit public input
		SpendAuthSig:        make([]byte, 64),
		SignedNoteNullifier: make([]byte, 32),
		CmxNew:              make([]byte, 32),
		EncMemo:             make([]byte, 64),
		GovComm:             make([]byte, 32),
		GovNullifiers: [][]byte{
			make([]byte, 32),
		},
		Proof:       proof,
		VoteRoundId: testRoundID,
	}

	// Use the real Halo2 verifier but mock the signature verifier
	// (RedPallas is not under test here).
	opts := ante.ValidateOpts{
		SigVerifier: redpallas.NewMockVerifier(),
		ZKPVerifier: halo2.NewVerifier(),
		SigHash:     testSigHash,
	}

	// Create a test suite for the keeper/context setup, then run through
	// the full ValidateVoteTx pipeline.
	s := new(ValidateTestSuite)
	s.SetT(t)
	s.SetupTest()
	s.setupActiveRound()

	err := ante.ValidateVoteTx(s.ctx, msg, s.keeper, opts)
	require.NoError(t, err, "valid Halo2 toy proof should pass the ante handler")
}

// TestHalo2DelegationWrongInput verifies that a real Halo2 proof fails when
// paired with the wrong public input (i.e. the full pipeline returns
// ErrInvalidProof).
func TestHalo2DelegationWrongInput(t *testing.T) {
	proof := mustReadFixture(t, "toy_valid_proof.bin")
	wrongInput := mustReadFixture(t, "toy_wrong_input.bin")

	msg := &types.MsgRegisterDelegation{
		Rk:                  wrongInput, // wrong public input
		SpendAuthSig:        make([]byte, 64),
		SignedNoteNullifier: make([]byte, 32),
		CmxNew:              make([]byte, 32),
		EncMemo:             make([]byte, 64),
		GovComm:             make([]byte, 32),
		GovNullifiers: [][]byte{
			make([]byte, 32),
		},
		Proof:       proof,
		VoteRoundId: testRoundID,
	}

	opts := ante.ValidateOpts{
		SigVerifier: redpallas.NewMockVerifier(),
		ZKPVerifier: halo2.NewVerifier(),
		SigHash:     testSigHash,
	}

	s := new(ValidateTestSuite)
	s.SetT(t)
	s.SetupTest()
	s.setupActiveRound()

	err := ante.ValidateVoteTx(s.ctx, msg, s.keeper, opts)
	require.Error(t, err, "wrong public input should fail verification")
	require.ErrorIs(t, err, types.ErrInvalidProof, "should wrap ErrInvalidProof")
}

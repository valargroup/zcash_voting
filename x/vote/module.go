package vote

import (
	"context"
	"encoding/json"

	"cosmossdk.io/core/appmodule"
	"cosmossdk.io/core/store"
	"cosmossdk.io/depinject"
	"cosmossdk.io/log"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/types/module"

	"github.com/z-cale/zally/x/vote/keeper"
	modulev1 "github.com/z-cale/zally/x/vote/module/v1"
	"github.com/z-cale/zally/x/vote/types"
)

// Compile-time interface assertions.
var (
	_ appmodule.AppModule     = AppModule{}
	_ appmodule.HasEndBlocker = AppModule{}
	_ module.HasName          = AppModule{}
	_ module.HasServices      = AppModule{}
)

// ----------------------------------------------------------------------------
// Depinject registration
// ----------------------------------------------------------------------------

func init() {
	appmodule.Register(
		&modulev1.Module{},
		appmodule.Provide(ProvideModule),
	)
}

// ModuleInputs defines the inputs needed to create the vote module.
type ModuleInputs struct {
	depinject.In

	StoreService store.KVStoreService
	Cdc          codec.Codec
	Logger       log.Logger
	Config       *modulev1.Module
}

// ModuleOutputs defines the outputs produced by the vote module.
type ModuleOutputs struct {
	depinject.Out

	Module appmodule.AppModule
	Keeper keeper.Keeper
}

// ProvideModule is called by depinject to construct the vote module and keeper.
func ProvideModule(in ModuleInputs) ModuleOutputs {
	k := keeper.NewKeeper(
		in.StoreService,
		in.Config.Authority,
		in.Logger,
	)

	m := NewAppModule(k, in.Cdc)

	return ModuleOutputs{
		Module: m,
		Keeper: k,
	}
}

// ----------------------------------------------------------------------------
// AppModule implementation
// ----------------------------------------------------------------------------

// AppModule implements the Cosmos SDK AppModule interface for the vote module.
type AppModule struct {
	keeper keeper.Keeper
	cdc    codec.Codec
}

// NewAppModule creates a new AppModule.
func NewAppModule(keeper keeper.Keeper, cdc codec.Codec) AppModule {
	return AppModule{keeper: keeper, cdc: cdc}
}

// IsOnePerModuleType implements depinject.OnePerModuleType.
func (AppModule) IsOnePerModuleType() {}

// IsAppModule implements appmodule.AppModule.
func (AppModule) IsAppModule() {}

// Name returns the module name.
func (AppModule) Name() string {
	return types.ModuleName
}

// RegisterServices registers the module's gRPC services with the app.
// NOTE: Only the QueryServer is registered here. The MsgServer is NOT registered
// because vote transactions bypass the standard Cosmos Tx envelope and
// MsgServiceRouter entirely (see Phase 5). Registering MsgServer would require
// cosmos.msg.v1.signer annotations which don't apply to ZKP-authenticated messages.
func (am AppModule) RegisterServices(cfg module.Configurator) {
	types.RegisterQueryServer(cfg.QueryServer(), keeper.NewQueryServerImpl(am.keeper))
}

// EndBlock is called at the end of each block.
// Phase 4: will compute the commitment tree root and store it keyed by block height.
func (am AppModule) EndBlock(_ context.Context) error {
	// TODO(Phase 4): Compute commitment tree root from all leaves appended in this block.
	// Store the root keyed by block height for anchor verification.
	return nil
}

// DefaultGenesis returns the default genesis state as raw JSON bytes.
func (am AppModule) DefaultGenesis(_ codec.JSONCodec) json.RawMessage {
	return json.RawMessage(`{}`)
}

// ValidateGenesis performs genesis state validation.
func (am AppModule) ValidateGenesis(_ codec.JSONCodec, _ client.TxEncodingConfig, _ json.RawMessage) error {
	return nil
}

// InitGenesis initializes the module state from genesis.
func (am AppModule) InitGenesis(_ context.Context, _ codec.JSONCodec, _ json.RawMessage) {
	// No-op for Phase 2. Vote module has no initial genesis state.
}

// ExportGenesis exports the module state as genesis.
func (am AppModule) ExportGenesis(_ context.Context, _ codec.JSONCodec) json.RawMessage {
	return json.RawMessage(`{}`)
}

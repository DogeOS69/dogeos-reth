//! DogeOS-owned type boundary for the standalone Reth node.
//!
//! The protocol-facing primitives, chainspec, Engine payload family, and generic transaction
//! storage binding are all selected at this boundary.

use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_reth_primitives::DogeosPrimitives;
use reth_node_types::NodeTypes;

mod args;
pub use args::DogeosRollupArgs;
mod payload;
pub use payload::DogeosPayloadBuilderBuilder;
mod engine;
pub use engine::DogeosEngineValidatorBuilder;
mod consensus;
pub use consensus::DogeosConsensusBuilder;
mod execution;
pub use execution::DogeosExecutorBuilder;
mod pool;
pub use pool::DogeosPoolBuilder;
mod network;
pub use network::DogeosNetworkBuilder;
mod wire_import;
pub use wire_import::{DogeosScrollWireEngineImporter, ScrollWireImportError};
mod rpc;
pub use rpc::{DogeosEthApiBuilder, DogeosPendingEnvBuilder};

use reth_node_builder::{
    Node, NodeAdapter,
    components::{BasicPayloadServiceBuilder, ComponentsBuilder, NodeComponentsBuilder},
};
use reth_rpc_builder::RethRpcModule;

/// Standard RPC and Engine API add-ons for a fully assembled DogeOS node.
pub type DogeosAddOns<Node> =
    reth_node_builder::rpc::RpcAddOns<Node, DogeosEthApiBuilder, DogeosEngineValidatorBuilder>;

/// Concrete component builder used by the DogeOS node preset.
pub type DogeosComponentsBuilder<Node> = ComponentsBuilder<
    Node,
    DogeosPoolBuilder,
    BasicPayloadServiceBuilder<DogeosPayloadBuilderBuilder>,
    DogeosNetworkBuilder,
    DogeosExecutorBuilder,
    DogeosConsensusBuilder,
>;

/// Concrete components and adapter produced by the DogeOS node preset.
pub type DogeosComponents<Node> =
    <DogeosComponentsBuilder<Node> as NodeComponentsBuilder<Node>>::Components;
pub type DogeosNodeAdapter<Node> = NodeAdapter<Node, DogeosComponents<Node>>;
pub type DogeosEthApi<Node> = reth_rpc::EthApi<
    Node,
    dogeos_reth_rpc::DogeosRpcConverter<<Node as reth_node_builder::FullNodeTypes>::Provider>,
>;

impl DogeosNodeTypes {
    pub const fn new(args: DogeosRollupArgs) -> Self {
        Self { args }
    }

    /// Complete Reth 2 component graph using DogeOS-owned execution and policy components.
    pub fn components<Node>() -> DogeosComponentsBuilder<Node>
    where
        Node: reth_node_builder::FullNodeTypes<Types = Self>,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(DogeosPoolBuilder::default())
            .executor(DogeosExecutorBuilder)
            .payload(BasicPayloadServiceBuilder::new(
                DogeosPayloadBuilderBuilder::default(),
            ))
            .network(DogeosNetworkBuilder::default())
            .consensus(DogeosConsensusBuilder)
    }

    /// Complete RPC/Engine add-on graph using DogeOS conversion and payload validation.
    pub fn add_ons<Node>() -> DogeosAddOns<DogeosNodeAdapter<Node>>
    where
        Node: reth_node_builder::FullNodeTypes<Types = Self>,
        DogeosEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<
                DogeosNodeAdapter<Node>,
                EthApi = DogeosEthApi<DogeosNodeAdapter<Node>>,
            >,
        DogeosEthApi<DogeosNodeAdapter<Node>>: reth_rpc_eth_api::helpers::EthTransactions<Error = reth_rpc_eth_types::EthApiError>
            + reth_rpc_eth_api::helpers::TraceExt
            + reth_rpc_eth_api::RpcNodeCore<Provider = Node::Provider>,
        <DogeosEthApi<DogeosNodeAdapter<Node>> as reth_rpc_eth_api::RpcNodeCore>::Provider:
            reth_chainspec::ChainSpecProvider<ChainSpec = DogeosChainSpec>,
    {
        Self::add_ons_with_sequencer(None)
    }

    fn add_ons_with_sequencer<Node>(
        sequencer_url: Option<String>,
    ) -> DogeosAddOns<DogeosNodeAdapter<Node>>
    where
        Node: reth_node_builder::FullNodeTypes<Types = Self>,
        DogeosEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<
                DogeosNodeAdapter<Node>,
                EthApi = DogeosEthApi<DogeosNodeAdapter<Node>>,
            >,
        DogeosEthApi<DogeosNodeAdapter<Node>>: reth_rpc_eth_api::helpers::EthTransactions<Error = reth_rpc_eth_types::EthApiError>
            + reth_rpc_eth_api::helpers::TraceExt
            + reth_rpc_eth_api::RpcNodeCore<Provider = Node::Provider>,
        <DogeosEthApi<DogeosNodeAdapter<Node>> as reth_rpc_eth_api::RpcNodeCore>::Provider:
            reth_chainspec::ChainSpecProvider<ChainSpec = DogeosChainSpec>,
    {
        DogeosAddOns::default().extend_rpc_modules(move |ctx| {
            let forwarder_url = sequencer_url
                .as_deref()
                .map(reqwest::Url::parse)
                .transpose()?
                .or_else(|| ctx.config().rpc.rpc_forwarder.clone());
            if let Some(url) = forwarder_url {
                let sequencer = dogeos_reth_rpc::SequencerClient::with_http_client(
                    url.as_str(),
                    reqwest::Client::new(),
                )?;
                let forwarder = dogeos_reth_rpc::DogeosRawTransactionForwarder::new(
                    ctx.registry.eth_api().clone(),
                    sequencer,
                    ctx.registry.tasks().clone(),
                    !ctx.config().txpool.no_local_transactions_propagation,
                );
                ctx.modules.add_or_replace_if_module_configured(
                    RethRpcModule::Eth,
                    forwarder.into_rpc()?,
                )?;
            }

            let witness_api = dogeos_reth_rpc::DogeosDebugWitnessApi::new(ctx.registry.debug_api());
            ctx.modules.add_or_replace_if_module_configured(
                RethRpcModule::Debug,
                witness_api.into_rpc()?,
            )?;
            Ok(())
        })
    }
}

/// Reth 2's generic body storage bound to the inherited Scroll transaction envelope.
pub type DogeosStorage = reth_storage_api::EthStorage<
    <DogeosPrimitives as reth_primitives_traits::NodePrimitives>::SignedTx,
>;

/// Node type and runtime policy configuration owned by DogeOS.
#[derive(Clone, Debug, Default)]
pub struct DogeosNodeTypes {
    /// Scroll-compatible runtime settings supplied by the CLI.
    pub args: DogeosRollupArgs,
}

impl NodeTypes for DogeosNodeTypes {
    type Primitives = DogeosPrimitives;
    type ChainSpec = DogeosChainSpec;
    type Storage = DogeosStorage;
    type Payload = DogeosEngineTypes;
}

impl<N> Node<N> for DogeosNodeTypes
where
    N: reth_node_builder::FullNodeTypes<Types = Self>,
    DogeosEthApiBuilder: reth_node_builder::rpc::EthApiBuilder<
            DogeosNodeAdapter<N>,
            EthApi = DogeosEthApi<DogeosNodeAdapter<N>>,
        >,
    DogeosEthApi<DogeosNodeAdapter<N>>: reth_rpc_eth_api::helpers::EthTransactions<Error = reth_rpc_eth_types::EthApiError>
        + reth_rpc_eth_api::helpers::TraceExt
        + reth_rpc_eth_api::RpcNodeCore<Provider = N::Provider>,
    <DogeosEthApi<DogeosNodeAdapter<N>> as reth_rpc_eth_api::RpcNodeCore>::Provider:
        reth_chainspec::ChainSpecProvider<ChainSpec = DogeosChainSpec>,
{
    type ComponentsBuilder = DogeosComponentsBuilder<N>;
    type AddOns = DogeosAddOns<DogeosNodeAdapter<N>>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(DogeosPoolBuilder::default())
            .executor(DogeosExecutorBuilder)
            .payload(BasicPayloadServiceBuilder::new(
                DogeosPayloadBuilderBuilder {
                    block_da_size_limit: Some(self.args.payload_size_limit),
                    ..Default::default()
                },
            ))
            .network(DogeosNetworkBuilder::default())
            .consensus(DogeosConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        DogeosNodeTypes::add_ons_with_sequencer(self.args.sequencer.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requires_node_types<T: NodeTypes>() {}

    #[test]
    fn dogeos_node_types_satisfy_reths_public_contract() {
        requires_node_types::<DogeosNodeTypes>();
    }

    fn requires_protocol_types<
        T: NodeTypes<
                Primitives = DogeosPrimitives,
                ChainSpec = DogeosChainSpec,
                Storage = DogeosStorage,
                Payload = DogeosEngineTypes,
            >,
    >() {
    }

    #[test]
    fn node_boundary_uses_dogeos_protocol_types() {
        requires_protocol_types::<DogeosNodeTypes>();
    }

    #[test]
    fn rollup_args_preserve_current_defaults() {
        let node = DogeosNodeTypes::default();
        assert_eq!(node.args, DogeosRollupArgs::default());
        assert_eq!(
            node.args.payload_size_limit,
            payload::DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT
        );
    }
}

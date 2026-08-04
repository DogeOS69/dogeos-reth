//! DogeOS-owned type boundary for the standalone Reth node.
//!
//! The protocol-facing primitives, chainspec, Engine payload family, and generic transaction
//! storage binding are all selected at this boundary.

use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_engine::{DogeosEngineTypes, ScrollPayloadAttributes};
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
mod storage;
pub use storage::DogeosStorage;
mod wire_import;
pub use wire_import::{DogeosScrollWireEngineImporter, ScrollWireImportError};
mod rpc;
use rpc::ScrollWireRuntime;
pub use rpc::{DogeosEthApiBuilder, DogeosPendingEnvBuilder};

use reth_node_builder::{
    DebugNode, Node, NodeAdapter,
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
    pub fn new(args: DogeosRollupArgs) -> Self {
        Self {
            args,
            scroll_wire: ScrollWireRuntime::default(),
        }
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
        dogeos_reth_rpc::DogeosRpcConverter<Node::Provider>: reth_rpc_convert::RpcConvert<
                Primitives = DogeosPrimitives,
                Evm = dogeos_reth_evm::ScrollEvmConfig,
                Error = reth_rpc_eth_types::EthApiError,
                Network = dogeos_rpc_types::Scroll,
            >,
    {
        Self::add_ons_with_policy(
            None,
            dogeos_reth_rpc::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE,
            payload::DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT,
            ScrollWireRuntime::default(),
            None,
        )
    }

    fn add_ons_with_policy<Node>(
        sequencer_url: Option<String>,
        min_suggested_priority_fee: u64,
        payload_size_limit: u64,
        scroll_wire: ScrollWireRuntime,
        scroll_wire_signer: Option<alloy_primitives::Address>,
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
        dogeos_reth_rpc::DogeosRpcConverter<Node::Provider>: reth_rpc_convert::RpcConvert<
                Primitives = DogeosPrimitives,
                Evm = dogeos_reth_evm::ScrollEvmConfig,
                Error = reth_rpc_eth_types::EthApiError,
                Network = dogeos_rpc_types::Scroll,
            >,
    {
        DogeosAddOns::new(
            DogeosEthApiBuilder::new(scroll_wire, scroll_wire_signer),
            DogeosEngineValidatorBuilder,
            Default::default(),
            Default::default(),
            Default::default(),
        )
        .extend_rpc_modules(move |ctx| {
            let priority_fee_api = dogeos_reth_rpc::DogeosPriorityFeeApi::new(
                ctx.registry.eth_api().clone(),
                ctx.registry.eth_api().gas_oracle().config().max_price,
                min_suggested_priority_fee,
                payload_size_limit,
            );
            ctx.modules.add_or_replace_if_module_configured(
                RethRpcModule::Eth,
                priority_fee_api.into_rpc()?,
            )?;

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

/// Node type and runtime policy configuration owned by DogeOS.
#[derive(Clone, Debug, Default)]
pub struct DogeosNodeTypes {
    /// Scroll-compatible runtime settings supplied by the CLI.
    pub args: DogeosRollupArgs,
    scroll_wire: ScrollWireRuntime,
}

/// Adapts Reth's Ethereum local miner attributes to the Scroll-compatible Engine API shape.
#[derive(Debug)]
struct DogeosLocalPayloadAttributesBuilder {
    inner: reth_engine_local::LocalPayloadAttributesBuilder<DogeosChainSpec>,
}

impl reth_node_builder::PayloadAttributesBuilder<ScrollPayloadAttributes, alloy_consensus::Header>
    for DogeosLocalPayloadAttributesBuilder
{
    fn build(
        &self,
        parent: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
    ) -> ScrollPayloadAttributes {
        let mut payload_attributes =
            reth_node_builder::PayloadAttributesBuilder::build(&self.inner, parent);
        // Scroll activates the Shanghai EVM rules but does not include EIP-4895 withdrawals in
        // Engine attributes or Storage V2 block bodies.
        payload_attributes.withdrawals = None;

        ScrollPayloadAttributes {
            payload_attributes,
            ..Default::default()
        }
    }
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
    dogeos_reth_rpc::DogeosRpcConverter<N::Provider>: reth_rpc_convert::RpcConvert<
            Primitives = DogeosPrimitives,
            Evm = dogeos_reth_evm::ScrollEvmConfig,
            Error = reth_rpc_eth_types::EthApiError,
            Network = dogeos_rpc_types::Scroll,
        >,
{
    type ComponentsBuilder = DogeosComponentsBuilder<N>;
    type AddOns = DogeosAddOns<DogeosNodeAdapter<N>>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        let network = if self.args.enable_scroll_wire {
            let (network, manager) = DogeosNetworkBuilder::new()
                .with_scroll_wire(dogeos_scroll_wire::ScrollWireConfig::new(true));
            self.scroll_wire.install(manager);
            network
        } else {
            DogeosNetworkBuilder::new()
        };
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
            .network(network)
            .consensus(DogeosConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        DogeosNodeTypes::add_ons_with_policy(
            self.args.sequencer.clone(),
            self.args.min_suggested_priority_fee,
            self.args.payload_size_limit,
            self.scroll_wire.clone(),
            self.args.scroll_wire_signer,
        )
    }
}

impl<N> DebugNode<N> for DogeosNodeTypes
where
    N: reth_node_builder::FullNodeComponents<Types = Self>,
    Self: Node<N>,
{
    type RpcBlock = alloy_rpc_types_eth::Block<dogeos_reth_primitives::ScrollTransactionSigned>;

    fn rpc_to_primitive_block(
        rpc_block: Self::RpcBlock,
    ) -> reth_primitives_traits::BlockTy<Self::Primitives> {
        rpc_block.into_consensus()
    }

    fn local_payload_attributes_builder(
        chain_spec: &Self::ChainSpec,
    ) -> impl reth_node_builder::PayloadAttributesBuilder<
        <Self::Payload as reth_node_builder::PayloadTypes>::PayloadAttributes,
        <Self::Primitives as reth_primitives_traits::NodePrimitives>::BlockHeader,
    > {
        DogeosLocalPayloadAttributesBuilder {
            inner: reth_engine_local::LocalPayloadAttributesBuilder::new(std::sync::Arc::new(
                chain_spec.clone(),
            )),
        }
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
        assert_eq!(
            node.args.min_suggested_priority_fee,
            dogeos_reth_rpc::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE
        );
        assert!(node.args.enable_scroll_wire);
        assert_eq!(node.args.scroll_wire_signer, None);
    }

    #[test]
    fn scroll_wire_signer_is_required_outside_dev() {
        let args = DogeosRollupArgs::default();
        assert!(
            args.validate_for_chain(&dogeos_chainspec::DOGEOS_DEV)
                .is_ok()
        );
        assert!(
            args.validate_for_chain(&dogeos_chainspec::DOGEOS_MAINNET)
                .is_err()
        );
        assert!(
            args.validate_for_chain(&dogeos_chainspec::DOGEOS_CHIKYU)
                .is_err()
        );

        let disabled = DogeosRollupArgs {
            enable_scroll_wire: false,
            ..Default::default()
        };
        assert!(
            disabled
                .validate_for_chain(&dogeos_chainspec::DOGEOS_MAINNET)
                .is_ok()
        );

        let authenticated = DogeosRollupArgs {
            scroll_wire_signer: Some(alloy_primitives::Address::ZERO),
            ..Default::default()
        };
        assert!(
            authenticated
                .validate_for_chain(&dogeos_chainspec::DOGEOS_MAINNET)
                .is_ok()
        );
    }

    #[test]
    fn local_miner_builds_scroll_payload_attributes() {
        let builder = DogeosLocalPayloadAttributesBuilder {
            inner: reth_engine_local::LocalPayloadAttributesBuilder::new(
                dogeos_chainspec::DOGEOS_DEV.clone(),
            ),
        };
        let parent = reth_primitives_traits::SealedHeader::seal_slow(alloy_consensus::Header {
            timestamp: 1,
            ..Default::default()
        });

        let attributes = reth_node_builder::PayloadAttributesBuilder::build(&builder, &parent);

        assert!(attributes.payload_attributes.timestamp >= 2);
        assert!(attributes.payload_attributes.withdrawals.is_none());
        assert!(!attributes.no_tx_pool);
        assert!(attributes.transactions.is_none());
        assert!(attributes.block_data_hint.is_empty());
        assert!(attributes.gas_limit.is_none());
    }
}

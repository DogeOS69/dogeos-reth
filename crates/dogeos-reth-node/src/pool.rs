use crate::DogeosNodeTypes;
use dogeos_reth_txpool::{
    DogeosPooledTransaction, DogeosTransactionPool, DogeosTransactionValidator,
};
use reth_evm::ConfigureEvm;
use reth_node_builder::{
    BuilderContext, FullNodeTypes,
    components::{PoolBuilder, PoolBuilderConfigOverrides},
};
use reth_provider::CanonStateSubscriptions;
use reth_transaction_pool::{
    CoinbaseTipOrdering, TransactionValidationTaskExecutor, blobstore::DiskFileBlobStore,
};

/// Builds the DogeOS transaction pool and its canonical-chain maintenance tasks.
#[derive(Debug, Clone, Default)]
pub struct DogeosPoolBuilder {
    pub pool_config_overrides: PoolBuilderConfigOverrides,
}

impl DogeosPoolBuilder {
    pub fn with_pool_config_overrides(mut self, overrides: PoolBuilderConfigOverrides) -> Self {
        self.pool_config_overrides = overrides;
        self
    }
}

impl<Node, Evm> PoolBuilder<Node, Evm> for DogeosPoolBuilder
where
    Node: FullNodeTypes<Types = DogeosNodeTypes>,
    Evm: ConfigureEvm<Primitives = dogeos_reth_primitives::DogeosPrimitives> + Clone + 'static,
{
    type Pool =
        DogeosTransactionPool<Node::Provider, DiskFileBlobStore, DogeosPooledTransaction, Evm>;

    async fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        evm_config: Evm,
    ) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.config().datadir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;
        let pool_config = self.pool_config_overrides.clone().apply(ctx.pool_config());

        let validator =
            TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone(), evm_config)
                .no_eip4844()
                .kzg_settings(ctx.kzg_settings()?)
                .with_local_transactions_config(pool_config.local_transactions_config.clone())
                .with_max_tx_input_bytes(ctx.chain_spec().config.max_tx_payload_bytes_per_block)
                .with_additional_tasks(
                    self.pool_config_overrides
                        .additional_validation_tasks
                        .unwrap_or(ctx.config().txpool.additional_validation_tasks),
                )
                .build_with_tasks(ctx.task_executor().clone(), blob_store.clone())
                .map(|validator| {
                    DogeosTransactionValidator::new(validator)
                        .require_l1_data_gas_fee(!ctx.config().dev.dev)
                        .require_l1_data_fee_buffer(
                            ctx.chain_spec().config.l1_data_fee_buffer_check,
                        )
                });

        let pool = reth_transaction_pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            pool_config,
        );
        let transactions_path = data_dir.txpool_transactions();
        let backup_config =
            reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(
                transactions_path,
            );
        ctx.task_executor()
            .spawn_critical_with_graceful_shutdown_signal("local transactions backup task", {
                let pool = pool.clone();
                move |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool,
                        backup_config,
                    )
                }
            });

        ctx.task_executor().spawn_critical_task(
            "txpool maintenance task",
            reth_transaction_pool::maintain::maintain_transaction_pool_future(
                ctx.provider().clone(),
                pool.clone(),
                ctx.provider().canonical_state_stream(),
                ctx.task_executor().clone(),
                reth_transaction_pool::maintain::MaintainPoolConfig {
                    max_tx_lifetime: pool.config().max_queued_lifetime,
                    ..Default::default()
                },
            ),
        );
        tracing::info!(target: "reth::cli", "DogeOS transaction pool initialized");
        Ok(pool)
    }
}

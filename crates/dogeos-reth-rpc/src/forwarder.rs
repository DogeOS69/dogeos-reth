use crate::{SequencerClient, SequencerClientError};
use alloy_primitives::{B256, Bytes};
use jsonrpsee::{RpcModule, types::ErrorObjectOwned};
use reth_rpc_eth_api::helpers::EthTransactions;
use reth_rpc_eth_types::EthApiError;
use reth_tasks::Runtime;

/// Scroll-compatible raw-transaction forwarding policy.
///
/// Transactions enter the local pool before forwarding. When local transactions are propagated,
/// forwarding is fire-and-forget; otherwise the sequencer response is awaited and its failure is
/// returned to the caller.
#[derive(Clone, Debug)]
pub struct DogeosRawTransactionForwarder<Eth> {
    eth_api: Eth,
    sequencer: SequencerClient,
    runtime: Runtime,
    propagate_local_transactions: bool,
}

impl<Eth> DogeosRawTransactionForwarder<Eth> {
    pub const fn new(
        eth_api: Eth,
        sequencer: SequencerClient,
        runtime: Runtime,
        propagate_local_transactions: bool,
    ) -> Self {
        Self {
            eth_api,
            sequencer,
            runtime,
            propagate_local_transactions,
        }
    }
}

impl<Eth> DogeosRawTransactionForwarder<Eth>
where
    Eth: EthTransactions<Error = EthApiError> + Clone + Send + Sync + 'static,
{
    pub async fn send_raw_transaction(&self, raw: Bytes) -> Result<B256, EthApiError> {
        let local_hash = self.eth_api.send_raw_transaction(raw.clone()).await?;
        let sequencer = self.sequencer.clone();

        if self.propagate_local_transactions {
            self.runtime.spawn_task(async move {
                match sequencer.forward_raw_transaction(&raw).await {
                    Ok(sequencer_hash) => tracing::debug!(
                        target: "dogeos::rpc::eth",
                        %local_hash,
                        %sequencer_hash,
                        "successfully forwarded transaction to sequencer"
                    ),
                    Err(error) => tracing::warn!(
                        target: "dogeos::rpc::eth",
                        %error,
                        %local_hash,
                        "failed to forward transaction to sequencer; transaction remains in the local pool"
                    ),
                }
            });
        } else {
            let sequencer_hash =
                sequencer
                    .forward_raw_transaction(&raw)
                    .await
                    .map_err(|error| match error {
                        SequencerClientError::Rpc(error) => EthApiError::other(error),
                    })?;
            tracing::debug!(
                target: "dogeos::rpc::eth",
                %local_hash,
                %sequencer_hash,
                "successfully forwarded transaction to sequencer"
            );
        }

        Ok(local_hash)
    }

    /// Builds the `eth_sendRawTransaction` replacement method.
    pub fn into_rpc(self) -> Result<RpcModule<Self>, jsonrpsee::core::RegisterMethodError> {
        let mut module = RpcModule::new(self);
        module.register_async_method("eth_sendRawTransaction", |params, api, _| async move {
            let raw = params.one::<Bytes>()?;
            api.send_raw_transaction(raw)
                .await
                .map_err(ErrorObjectOwned::from)
        })?;
        Ok(module)
    }
}

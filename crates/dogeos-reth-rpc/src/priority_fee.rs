use alloy_consensus::{BlockHeader, Transaction, TxReceipt};
use alloy_eips::{BlockNumberOrTag, eip2718::Encodable2718};
use alloy_primitives::{U64, U256};
use alloy_rpc_types_eth::FeeHistory;
use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_evm::ScrollBaseFeeProvider;
use jsonrpsee::{RpcModule, types::ErrorObjectOwned};
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::BlockBody;
use reth_revm::database::StateProviderDatabase;
use reth_rpc_eth_api::{RpcNodeCore, RpcNodeCoreExt, helpers::EthFees};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};

/// Default minimum tip returned while the latest block still has capacity.
pub const DEFAULT_MIN_SUGGESTED_PRIORITY_FEE: u64 = 100;

fn is_at_capacity(
    gas_used: u64,
    gas_limit: u64,
    max_tx_gas_used: u64,
    total_payload_size: u64,
    max_tx_payload_size: u64,
    payload_size_limit: u64,
) -> bool {
    gas_used.saturating_add(max_tx_gas_used) > gas_limit
        || total_payload_size.saturating_add(max_tx_payload_size) > payload_size_limit
}

fn capacity_tip(mut tips: Vec<u128>, minimum: U256, maximum: Option<U256>) -> U256 {
    if tips.is_empty() {
        return minimum;
    }
    tips.sort_unstable();
    let median = if tips.len() % 2 == 1 {
        tips[tips.len() / 2]
    } else {
        (tips[tips.len() / 2 - 1] + tips[tips.len() / 2]) / 2
    };
    let median = U256::from(median);
    let mut suggestion = (median + median / U256::from(10)).max(minimum);
    if let Some(maximum) = maximum {
        suggestion = suggestion.min(maximum);
    }
    suggestion
}

/// Scroll-compatible priority-fee policy installed over Reth's generic Ethereum method.
#[derive(Clone, Debug)]
pub struct DogeosPriorityFeeApi<Eth: RpcNodeCore> {
    eth: Eth,
    max_price: Option<U256>,
    min_suggested_priority_fee: U256,
    payload_size_limit: u64,
}

impl<Eth: RpcNodeCore> DogeosPriorityFeeApi<Eth> {
    pub fn new(
        eth: Eth,
        max_price: Option<U256>,
        min_suggested_priority_fee: u64,
        payload_size_limit: u64,
    ) -> Self {
        Self {
            eth,
            max_price,
            min_suggested_priority_fee: U256::from(min_suggested_priority_fee),
            payload_size_limit,
        }
    }

    pub fn into_rpc(self) -> Result<RpcModule<Self>, jsonrpsee::core::RegisterMethodError>
    where
        Self: Clone + Send + Sync + 'static,
        Eth: EthFees + RpcNodeCoreExt + Send + Sync + 'static,
        Eth::Provider: ChainSpecProvider<ChainSpec = DogeosChainSpec> + StateProviderFactory,
        Eth::Error: Into<ErrorObjectOwned>,
    {
        let mut module = RpcModule::new(self);
        module.register_async_method("eth_maxPriorityFeePerGas", |_, api, _| async move {
            api.suggested_priority_fee().await
        })?;
        module.register_async_method("eth_feeHistory", |params, api, _| async move {
            let mut params = params.sequence();
            let block_count = params.next::<U64>()?;
            let newest_block = params.next::<BlockNumberOrTag>()?;
            let reward_percentiles = params.optional_next::<Vec<f64>>()?;
            api.fee_history(block_count.to(), newest_block, reward_percentiles)
                .await
        })?;
        Ok(module)
    }
}

impl<Eth> DogeosPriorityFeeApi<Eth>
where
    Eth: RpcNodeCoreExt,
{
    async fn priority_fee_for_block(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<(U256, bool), ErrorObjectOwned> {
        let header = self
            .eth
            .provider()
            .sealed_header_by_number_or_tag(block)
            .map_err(|error| ErrorObjectOwned::from(EthApiError::from(error)))?
            .ok_or_else(|| ErrorObjectOwned::from(EthApiError::HeaderNotFound(block.into())))?;

        let Some((block, receipts)) = self
            .eth
            .cache()
            .get_block_and_receipts(header.hash())
            .await
            .map_err(|error| ErrorObjectOwned::from(EthApiError::from(error)))?
        else {
            return Ok((self.min_suggested_priority_fee, false));
        };

        let mut max_tx_gas_used = 0u64;
        let mut last_cumulative_gas = 0u64;
        for receipt in receipts.iter() {
            let cumulative_gas = receipt.cumulative_gas_used();
            max_tx_gas_used =
                max_tx_gas_used.max(cumulative_gas.saturating_sub(last_cumulative_gas));
            last_cumulative_gas = cumulative_gas;
        }

        let (total_payload_size, max_tx_payload_size) =
            block
                .body()
                .transactions()
                .iter()
                .fold((0u64, 0u64), |(total, max), transaction| {
                    let len = transaction.encode_2718_len() as u64;
                    (total.saturating_add(len), max.max(len))
                });

        let at_capacity = is_at_capacity(
            header.gas_used(),
            header.gas_limit(),
            max_tx_gas_used,
            total_payload_size,
            max_tx_payload_size,
            self.payload_size_limit,
        );
        if !at_capacity {
            return Ok((self.min_suggested_priority_fee, false));
        }

        let base_fee = block.base_fee_per_gas();
        let tips = block
            .body()
            .transactions()
            .iter()
            .filter_map(|transaction| {
                base_fee
                    .map(|base_fee| transaction.effective_tip_per_gas(base_fee))
                    .unwrap_or_else(|| Some(transaction.priority_fee_or_price()))
            })
            .collect::<Vec<_>>();
        Ok((
            capacity_tip(tips, self.min_suggested_priority_fee, self.max_price),
            true,
        ))
    }

    async fn suggested_priority_fee(&self) -> Result<U256, ErrorObjectOwned> {
        self.priority_fee_for_block(BlockNumberOrTag::Latest)
            .await
            .map(|(tip, _)| tip)
    }
}

impl<Eth> DogeosPriorityFeeApi<Eth>
where
    Eth: EthFees + RpcNodeCoreExt,
    Eth::Provider: ChainSpecProvider<ChainSpec = DogeosChainSpec> + StateProviderFactory,
    Eth::Error: Into<ErrorObjectOwned>,
{
    async fn fee_history(
        &self,
        block_count: u64,
        mut newest_block: BlockNumberOrTag,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory, ErrorObjectOwned> {
        let mut history = EthFees::fee_history(
            &self.eth,
            block_count,
            newest_block,
            reward_percentiles.clone(),
        )
        .await
        .map_err(Into::into)?;

        if block_count == 0 {
            return Ok(history);
        }
        if newest_block.is_pending() {
            newest_block = BlockNumberOrTag::Latest;
        }

        let last_header = self
            .eth
            .provider()
            .sealed_header_by_number_or_tag(newest_block)
            .map_err(|error| ErrorObjectOwned::from(EthApiError::from(error)))?
            .ok_or_else(|| {
                ErrorObjectOwned::from(EthApiError::HeaderNotFound(newest_block.into()))
            })?;
        let next_base_fee = {
            let state = self
                .eth
                .provider()
                .state_by_block_id(last_header.hash().into())
                .map_err(|error| ErrorObjectOwned::from(EthApiError::from(error)))?;
            let mut state = StateProviderDatabase::new(state.as_ref());
            ScrollBaseFeeProvider::new(self.eth.provider().chain_spec())
                .next_block_base_fee(&mut state, last_header.header(), last_header.timestamp())
                .map_err(|error| ErrorObjectOwned::from(EthApiError::Internal(error.into())))?
        };
        if let Some(next) = history.base_fee_per_gas.last_mut() {
            *next = next_base_fee as u128;
        }

        if let Some(percentiles) = reward_percentiles {
            let (suggestion, at_capacity) = self.priority_fee_for_block(newest_block).await?;
            if !at_capacity {
                history.reward = Some(vec![
                    vec![suggestion.to::<u128>(); percentiles.len()];
                    history.gas_used_ratio.len()
                ]);
            }
        }

        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_bytes_can_mark_a_block_at_capacity() {
        assert!(is_at_capacity(0, 20_000_000, 0, 90, 20, 100));
        assert!(!is_at_capacity(0, 20_000_000, 0, 80, 20, 100));
    }

    #[test]
    fn capacity_tip_uses_median_plus_ten_percent() {
        assert_eq!(
            capacity_tip(vec![100, 300, 200], U256::from(100), None),
            U256::from(220)
        );
        assert_eq!(
            capacity_tip(vec![100, 300], U256::from(250), None),
            U256::from(250)
        );
        assert_eq!(
            capacity_tip(vec![1_000], U256::from(100), Some(U256::from(500))),
            U256::from(500)
        );
    }
}

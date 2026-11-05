use crate::DogeosPriorityFeeApi;
use alloy_consensus::SignableTransaction;
use alloy_eips::{BlockId, eip2718::Encodable2718};
use alloy_primitives::{Address, Signature, TxKind, U64, U256, b256};
use alloy_rpc_types_eth::TransactionRequest;
use dogeos_chainspec::DogeosChainSpec;
use dogeos_protocol_types::{ScrollTxEnvelope, ScrollTypedTransaction};
use dogeos_reth_evm::{
    ScrollBaseFeeProvider, ScrollTransactionIntoTxEnv, spec_id_at_timestamp_and_number,
};
use dogeos_rpc_types::ScrollTransactionRequest;
use jsonrpsee::{
    RpcModule,
    types::{ErrorObjectOwned, error::INVALID_PARAMS_CODE},
};
use reth_chainspec::ChainSpecProvider;
use reth_errors::RethError;
use reth_evm::FromTxWithEncoded;
use reth_revm::database::StateProviderDatabase;
use reth_rpc_eth_api::{
    EthApiTypes, RpcNodeCoreExt,
    helpers::{LoadPendingBlock, LoadState, estimate::EstimateCall},
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use revm::context::{Block, TxEnv};
use revm_scroll::{
    ScrollSpecId, ScrollTransaction,
    l1block::{L1BlockInfo, L1FeeError},
};
use serde::Serialize;

/// Margin added to the L1 data fee for the oracle price moving between quote and inclusion.
///
/// Replaying ten days of testnet oracle updates through the mainnet update policy, the fee per
/// byte rose by at most 22% over 60 seconds in 99% of windows.
pub const L1_DATA_FEE_BUFFER_PERCENT: u64 = 25;

/// Placeholder signature used to price an unsigned request. Its `r` and `s` are random bytes, so
/// they do not compress, and their first byte is non-zero, so they encode at full length.
const PLACEHOLDER_SIGNATURE_R: U256 = U256::from_be_bytes(
    b256!("c8264941029a5ee8e7f78f11eb580a31962338367297a8c28802035d80b38b6d").0,
);
const PLACEHOLDER_SIGNATURE_S: U256 = U256::from_be_bytes(
    b256!("da6ca367a96c07acd5a9b49cb12912e41058d2316a0f9ab3f92486aac5f39213").0,
);

/// Result of `dogeos_estimateFee`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DogeosFeeEstimate {
    /// Execution gas, as `eth_estimateGas` returns it.
    pub gas_limit: U64,
    /// Base fee of the block after the one the estimate is made at.
    pub base_fee_per_gas: U256,
    /// Tip the transaction is priced with.
    pub max_priority_fee_per_gas: U256,
    /// Fee cap the transaction is priced with; the gas price for legacy transactions.
    pub max_fee_per_gas: U256,
    /// L1 data fee of the transaction at the block the estimate is made at.
    pub l1_data_fee: U256,
    /// `l1_data_fee` plus [`L1_DATA_FEE_BUFFER_PERCENT`].
    pub l1_data_fee_buffered: U256,
    /// `gas_limit * max_fee_per_gas + l1_data_fee_buffered`.
    pub total_fee: U256,
}

/// Serves `dogeos_estimateFee` and Scroll's `scroll_estimateL1DataFee`.
#[derive(Clone, Debug)]
pub struct DogeosEstimateFeeApi<Eth: reth_rpc_eth_api::RpcNodeCore> {
    priority_fee: DogeosPriorityFeeApi<Eth>,
}

impl<Eth: reth_rpc_eth_api::RpcNodeCore> DogeosEstimateFeeApi<Eth> {
    /// Suggests tips with the same policy as `eth_maxPriorityFeePerGas`.
    pub const fn new(priority_fee: DogeosPriorityFeeApi<Eth>) -> Self {
        Self { priority_fee }
    }
}

impl<Eth> DogeosEstimateFeeApi<Eth>
where
    Eth: EstimateCall
        + LoadPendingBlock
        + LoadState
        + RpcNodeCoreExt
        + EthApiTypes<NetworkTypes = dogeos_rpc_types::Scroll>
        + Clone
        + Send
        + Sync
        + 'static,
    Eth::Provider: ChainSpecProvider<ChainSpec = DogeosChainSpec> + StateProviderFactory,
    Eth::Error: Into<ErrorObjectOwned>,
{
    pub fn into_rpc(self) -> Result<RpcModule<Self>, jsonrpsee::core::RegisterMethodError> {
        let mut module = RpcModule::new(self);
        module.register_async_method("dogeos_estimateFee", |params, api, _| async move {
            let mut params = params.sequence();
            let request = params.next::<ScrollTransactionRequest>()?;
            let block = params.optional_next::<BlockId>()?.unwrap_or_default();
            api.estimate_fee(request, block).await
        })?;
        // Scroll l2geth's method: same name, parameters, default block and result type.
        module.register_async_method("scroll_estimateL1DataFee", |params, api, _| async move {
            let mut params = params.sequence();
            let request = params.next::<ScrollTransactionRequest>()?;
            let block = params
                .optional_next::<BlockId>()?
                .unwrap_or_else(BlockId::pending);
            api.estimate_fee(request, block)
                .await
                .map(|estimate| U64::from(estimate.l1_data_fee.saturating_to::<u64>()))
        })?;
        Ok(module)
    }

    async fn estimate_fee(
        &self,
        request: ScrollTransactionRequest,
        block: BlockId,
    ) -> Result<DogeosFeeEstimate, ErrorObjectOwned> {
        let eth = &self.priority_fee.eth;
        let gas_limit = EstimateCall::estimate_gas_at(eth, request.clone(), block, None)
            .await
            .map_err(Into::into)?
            .saturating_to::<u64>();

        let (evm_env, state_block) = eth.evm_env_at(block).await.map_err(Into::into)?;
        let chain_spec = eth.provider().chain_spec();
        let header = eth
            .provider()
            .sealed_header_by_id(state_block)
            .map_err(|error| ErrorObjectOwned::from(EthApiError::from(error)))?
            .ok_or_else(|| ErrorObjectOwned::from(EthApiError::HeaderNotFound(state_block)))?;
        let state = eth
            .state_at_block_id(state_block)
            .await
            .map_err(Into::into)?;
        let mut state = StateProviderDatabase::new(state);
        let base_fee = ScrollBaseFeeProvider::new(chain_spec.clone())
            .predict_next_block_base_fee(&mut state, header.header())
            .map_err(|error| ErrorObjectOwned::from(EthApiError::Internal(error.into())))?;
        let timestamp = evm_env.block_env.timestamp().saturating_to::<u64>();
        let number = evm_env.block_env.number().saturating_to::<u64>();
        let spec = spec_id_at_timestamp_and_number(timestamp, number, chain_spec.as_ref());
        let l1_block_info = L1BlockInfo::try_fetch(&mut state, spec)
            .map_err(|error| ErrorObjectOwned::from(EthApiError::Internal(error.into())))?;

        let mut request = request.into_inner();
        let base_fee = U256::from(base_fee);
        let suggested_tip = self.priority_fee.suggested_priority_fee().await?;
        let (max_fee, tip) = fill_fees(&mut request, base_fee, suggested_tip);
        if request.nonce.is_none() {
            let from = request.from.unwrap_or_default();
            let nonce = LoadState::transaction_count(eth, from, Some(block))
                .await
                .map_err(Into::into)?;
            request.nonce = Some(nonce.saturating_to());
        }
        request.gas = Some(gas_limit);
        request.chain_id.get_or_insert(evm_env.cfg_env.chain_id);
        request.to.get_or_insert(TxKind::Create);

        let request: ScrollTransactionRequest = request.into();
        let transaction = request.build_typed_tx().map_err(|error| {
            ErrorObjectOwned::owned(INVALID_PARAMS_CODE, error.to_string(), None::<()>)
        })?;
        let l1_data_fee = l1_data_fee(&l1_block_info, spec, &placeholder_signed(transaction))
            .map_err(|error| {
                ErrorObjectOwned::from(EthApiError::Internal(RethError::other(error)))
            })?;

        Ok(fee_estimate(gas_limit, base_fee, tip, max_fee, l1_data_fee))
    }
}

/// Fills the request's missing fee fields and returns its fee cap and tip. Legacy and EIP-2930
/// requests get `base_fee + tip` as gas price; others a cap of `2 * base_fee + tip`, the default
/// ethers and viem use, which leaves room for the base fee to rise before inclusion.
fn fill_fees(
    request: &mut TransactionRequest,
    base_fee: U256,
    suggested_tip: U256,
) -> (U256, U256) {
    if request.gas_price.is_some() || matches!(request.transaction_type, Some(0 | 1)) {
        let gas_price = request
            .gas_price
            .map(U256::from)
            .unwrap_or(base_fee + suggested_tip);
        request.gas_price = Some(gas_price.saturating_to());
        (gas_price, gas_price.saturating_sub(base_fee))
    } else {
        let tip = request
            .max_priority_fee_per_gas
            .map(U256::from)
            .unwrap_or(suggested_tip);
        let max_fee = request
            .max_fee_per_gas
            .map(U256::from)
            .unwrap_or(base_fee * U256::from(2) + tip);
        let tip = tip.min(max_fee);
        request.max_fee_per_gas = Some(max_fee.saturating_to());
        request.max_priority_fee_per_gas = Some(tip.saturating_to());
        (max_fee, tip)
    }
}

/// Prices `transaction` the way block execution sets the receipt's L1 fee: the same transaction
/// environment conversion supplies the encoded bytes and compression inputs.
fn l1_data_fee(
    l1_block_info: &L1BlockInfo,
    spec: ScrollSpecId,
    transaction: &ScrollTxEnvelope,
) -> Result<U256, L1FeeError> {
    let ScrollTransaction {
        rlp_bytes,
        compression_ratio,
        compressed_size,
        ..
    } = ScrollTransactionIntoTxEnv::<TxEnv>::from_encoded_tx(
        transaction,
        Address::ZERO,
        transaction.encoded_2718().into(),
    )
    .into();
    l1_block_info.try_calculate_tx_l1_cost(
        &rlp_bytes.unwrap_or_default(),
        spec,
        compression_ratio,
        compressed_size,
    )
}

/// Signs `transaction` with the placeholder signature.
fn placeholder_signed(transaction: ScrollTypedTransaction) -> ScrollTxEnvelope {
    let signature = Signature::new(PLACEHOLDER_SIGNATURE_R, PLACEHOLDER_SIGNATURE_S, true);
    transaction.into_signed(signature).into()
}

fn fee_estimate(
    gas_limit: u64,
    base_fee_per_gas: U256,
    max_priority_fee_per_gas: U256,
    max_fee_per_gas: U256,
    l1_data_fee: U256,
) -> DogeosFeeEstimate {
    let l1_data_fee_buffered = l1_data_fee
        .saturating_mul(U256::from(100 + L1_DATA_FEE_BUFFER_PERCENT))
        .wrapping_div(U256::from(100));
    DogeosFeeEstimate {
        gas_limit: U64::from(gas_limit),
        base_fee_per_gas,
        max_priority_fee_per_gas,
        max_fee_per_gas,
        l1_data_fee,
        l1_data_fee_buffered,
        total_fee: U256::from(gas_limit)
            .saturating_mul(max_fee_per_gas)
            .saturating_add(l1_data_fee_buffered),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{TxEip1559, TxEip2930, TxEip7702, TxLegacy};
    use alloy_eips::{eip2930::AccessListItem, eip7702::Authorization};
    use alloy_primitives::{Bytes, keccak256};

    fn noise(seed: u64, len: usize) -> Vec<u8> {
        (0..len.div_ceil(32) as u64)
            .flat_map(|i| keccak256([seed.to_be_bytes(), i.to_be_bytes()].concat()).0)
            .take(len)
            .collect()
    }

    /// Bytecode-like input: repeated short patterns mixed with incompressible constants.
    fn bytecode(len: usize) -> Bytes {
        let mut code = Vec::with_capacity(len);
        let mut i = 0u64;
        while code.len() < len {
            code.extend_from_slice(&[0x60, 0x80, 0x60, 0x40, 0x52, 0x34, 0x80, 0x15]);
            code.push(0x7f);
            code.extend(noise(i, 32));
            i += 1;
        }
        code.truncate(len);
        code.into()
    }

    fn corpus() -> Vec<ScrollTypedTransaction> {
        let to = Address::repeat_byte(0x11);
        let approve: Bytes = [
            &[0x09, 0x5e, 0xa7, 0xb3][..],
            &[0; 12],
            to.as_slice(),
            &[0xff; 32],
        ]
        .concat()
        .into();
        let eip1559 = |nonce, to: TxKind, input: Bytes| {
            ScrollTypedTransaction::Eip1559(TxEip1559 {
                chain_id: 6_281_971,
                nonce,
                gas_limit: 21_000,
                max_fee_per_gas: 2_000_000_000,
                max_priority_fee_per_gas: 100,
                to,
                input,
                ..Default::default()
            })
        };
        let authorization = Authorization {
            chain_id: U256::from(6_281_971),
            address: Address::repeat_byte(0xde),
            nonce: 1,
        }
        .into_signed(Signature::new(
            U256::from_be_slice(&noise(1, 32)),
            U256::from_be_slice(&noise(2, 32)),
            false,
        ));
        vec![
            eip1559(0, TxKind::Call(to), Bytes::new()),
            eip1559(1_000_000, TxKind::Call(to), approve.clone()),
            eip1559(3, TxKind::Create, bytecode(500)),
            eip1559(3, TxKind::Create, bytecode(5_000)),
            eip1559(3, TxKind::Create, bytecode(24_000)),
            ScrollTypedTransaction::Legacy(TxLegacy {
                chain_id: Some(6_281_971),
                nonce: 7,
                gas_price: 2_000_000_000,
                gas_limit: 50_000,
                to: TxKind::Call(to),
                input: approve,
                ..Default::default()
            }),
            ScrollTypedTransaction::Eip2930(TxEip2930 {
                chain_id: 6_281_971,
                gas_limit: 50_000,
                to: TxKind::Call(to),
                access_list: vec![AccessListItem {
                    address: to,
                    storage_keys: vec![keccak256([1]), keccak256([2])],
                }]
                .into(),
                ..Default::default()
            }),
            ScrollTypedTransaction::Eip7702(TxEip7702 {
                chain_id: 6_281_971,
                gas_limit: 100_000,
                to,
                authorization_list: vec![authorization],
                ..Default::default()
            }),
        ]
    }

    fn compressed_size(transaction: &ScrollTxEnvelope) -> usize {
        ScrollTransaction::from(ScrollTransactionIntoTxEnv::<TxEnv>::from_encoded_tx(
            transaction,
            Address::ZERO,
            transaction.encoded_2718().into(),
        ))
        .compressed_size
        .unwrap()
    }

    #[test]
    fn fills_missing_fee_fields() {
        let base_fee = U256::from(1_000);
        let tip = U256::from(100);

        let mut request = TransactionRequest::default();
        assert_eq!(
            fill_fees(&mut request, base_fee, tip),
            (U256::from(2_100), tip)
        );
        assert_eq!(request.max_fee_per_gas, Some(2_100));
        assert_eq!(request.max_priority_fee_per_gas, Some(100));
        assert_eq!(request.gas_price, None);

        let mut request = TransactionRequest {
            transaction_type: Some(0),
            ..Default::default()
        };
        assert_eq!(
            fill_fees(&mut request, base_fee, tip),
            (U256::from(1_100), tip)
        );
        assert_eq!(request.gas_price, Some(1_100));
        assert_eq!(request.max_fee_per_gas, None);

        // Fields the caller set are kept, and the tip never exceeds the cap.
        let mut request = TransactionRequest {
            max_fee_per_gas: Some(1_050),
            ..Default::default()
        };
        assert_eq!(
            fill_fees(&mut request, base_fee, tip),
            (U256::from(1_050), U256::from(100))
        );
        let mut request = TransactionRequest {
            max_fee_per_gas: Some(50),
            ..Default::default()
        };
        assert_eq!(
            fill_fees(&mut request, base_fee, tip),
            (U256::from(50), U256::from(50))
        );
        let mut request = TransactionRequest {
            gas_price: Some(900),
            ..Default::default()
        };
        assert_eq!(
            fill_fees(&mut request, base_fee, tip),
            (U256::from(900), U256::ZERO)
        );
    }

    #[test]
    fn total_fee_adds_the_buffered_l1_fee() {
        let estimate = fee_estimate(
            21_000,
            U256::from(1_000),
            U256::from(100),
            U256::from(2_100),
            U256::from(1_000_001),
        );
        assert_eq!(estimate.l1_data_fee_buffered, U256::from(1_250_001));
        assert_eq!(estimate.total_fee, U256::from(21_000 * 2_100 + 1_250_001));
        assert_eq!(
            serde_json::to_value(&estimate).unwrap(),
            serde_json::json!({
                "gasLimit": "0x5208",
                "baseFeePerGas": "0x3e8",
                "maxPriorityFeePerGas": "0x64",
                "maxFeePerGas": "0x834",
                "l1DataFee": "0xf4241",
                "l1DataFeeBuffered": "0x1312d1",
                "totalFee": "0x2b3fc71",
            })
        );
    }

    /// The placeholder encodes at least as long and compresses to at least as many bytes as real
    /// signatures, so the quote is never below the fee charged, and at most a byte above it.
    #[test]
    fn placeholder_signature_bounds_real_signatures() {
        for transaction in corpus() {
            let placeholder = placeholder_signed(transaction.clone());
            let placeholder_size = compressed_size(&placeholder);
            for i in 0..1_000u64 {
                // Seeds disjoint from the corpus, so a signature never repeats corpus bytes.
                let seed = (1 << 32) + 2 * i;
                let signature = Signature::new(
                    U256::from_be_slice(&noise(seed, 32)),
                    U256::from_be_slice(&noise(seed + 1, 32)),
                    i % 2 == 0,
                );
                let signed: ScrollTxEnvelope = transaction.clone().into_signed(signature).into();
                assert!(signed.encode_2718_len() <= placeholder.encode_2718_len());
                let size = compressed_size(&signed);
                assert!(
                    size <= placeholder_size && placeholder_size <= size + 1,
                    "type {} signature {i}: {size} vs placeholder {placeholder_size}",
                    placeholder.tx_type() as u8,
                );
            }
        }
    }
}

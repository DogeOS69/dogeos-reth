use alloy_consensus::{
    SignableTransaction,
    error::ValueError,
    transaction::{Recovered, TxHashRef},
};
use alloy_primitives::{Address, Signature};
use alloy_rpc_types_eth::TransactionInfo;
use dogeos_protocol_types::{ScrollAdditionalInfo, ScrollTransactionInfo, ScrollTxEnvelope};
use dogeos_reth_evm::{ScrollEvmConfig, ScrollTransactionIntoTxEnv};
use dogeos_reth_primitives::ScrollReceipt;
use dogeos_rpc_types::{Scroll, ScrollRpcTransaction, ScrollTransactionRequest};
use reth_errors::ProviderError;
use reth_evm::EvmEnvFor;
use reth_rpc_convert::{
    EthTxEnvError, RpcConverter, TryIntoTxEnv, TxInfoMapper,
    transaction::{RpcTxConverter, SimTxConverter, TxEnvConverter},
};
use reth_storage_api::ReceiptProvider;
use revm::context::TxEnv;
use std::{convert::Infallible, fmt};

/// Adds receipt-derived Scroll transaction metadata during RPC conversion.
pub struct ScrollTxInfoMapper<Provider>(Provider);

impl<Provider: Clone> Clone for ScrollTxInfoMapper<Provider> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<Provider> fmt::Debug for ScrollTxInfoMapper<Provider> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScrollTxInfoMapper").finish()
    }
}

impl<Provider> ScrollTxInfoMapper<Provider> {
    pub const fn new(provider: Provider) -> Self {
        Self(provider)
    }
}

impl<Provider> TxInfoMapper<ScrollTxEnvelope> for ScrollTxInfoMapper<Provider>
where
    Provider: ReceiptProvider<Receipt = ScrollReceipt>,
{
    type Out = ScrollTransactionInfo;
    type Err = ProviderError;

    fn try_map(
        &self,
        tx: &ScrollTxEnvelope,
        tx_info: TransactionInfo,
    ) -> Result<Self::Out, Self::Err> {
        let additional_info = if tx.is_l1_message() {
            None
        } else {
            self.0
                .receipt_by_hash(*tx.tx_hash())?
                .map(|receipt| ScrollAdditionalInfo {
                    l1_fee: receipt.l1_fee(),
                })
        }
        .unwrap_or_default();
        Ok(ScrollTransactionInfo::new(tx_info, additional_info))
    }
}

/// Converts a consensus Scroll transaction into its RPC response representation.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollRpcTxConverter;

impl RpcTxConverter<ScrollTxEnvelope, ScrollRpcTransaction, ScrollTransactionInfo>
    for ScrollRpcTxConverter
{
    type Err = Infallible;

    fn convert_rpc_tx(
        &self,
        tx: ScrollTxEnvelope,
        signer: Address,
        tx_info: ScrollTransactionInfo,
    ) -> Result<ScrollRpcTransaction, Self::Err> {
        Ok(ScrollRpcTransaction::from_transaction(
            Recovered::new_unchecked(tx, signer),
            tx_info,
        ))
    }
}

/// Builds a dummy-signed transaction for `eth_simulateV1`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollSimTxConverter;

impl SimTxConverter<ScrollTransactionRequest, ScrollTxEnvelope> for ScrollSimTxConverter {
    type Err = ValueError<ScrollTransactionRequest>;

    fn convert_sim_tx(
        &self,
        request: ScrollTransactionRequest,
    ) -> Result<ScrollTxEnvelope, Self::Err> {
        let transaction = request
            .build_typed_tx()
            .map_err(|request| ValueError::new(request, "required fields missing"))?;
        let signature = Signature::new(Default::default(), Default::default(), false);
        Ok(transaction.into_signed(signature).into())
    }
}

/// Converts RPC call requests into the transaction environment used by Scroll REVM.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollTxEnvConverter;

impl TxEnvConverter<ScrollTransactionRequest, ScrollEvmConfig> for ScrollTxEnvConverter {
    type Error = EthTxEnvError;

    fn convert_tx_env(
        &self,
        request: ScrollTransactionRequest,
        evm_env: &EvmEnvFor<ScrollEvmConfig>,
    ) -> Result<ScrollTransactionIntoTxEnv<TxEnv>, Self::Error> {
        let tx_env = request.into_inner().try_into_tx_env(evm_env)?;
        Ok(ScrollTransactionIntoTxEnv::new(tx_env, None, None, None))
    }
}

/// Fully assembled conversion pipeline for the DogeOS `eth_` RPC API.
pub type DogeosRpcConverter<Provider> = RpcConverter<
    Scroll,
    ScrollEvmConfig,
    crate::ScrollReceiptConverter,
    (),
    ScrollTxInfoMapper<Provider>,
    ScrollSimTxConverter,
    ScrollRpcTxConverter,
    ScrollTxEnvConverter,
>;

/// Builds the conversion pipeline used by the DogeOS node RPC component.
pub fn dogeos_rpc_converter<Provider>(provider: Provider) -> DogeosRpcConverter<Provider> {
    RpcConverter::new(crate::ScrollReceiptConverter)
        .with_mapper(ScrollTxInfoMapper::new(provider))
        .with_sim_tx_converter(ScrollSimTxConverter)
        .with_rpc_tx_converter(ScrollRpcTxConverter)
        .with_tx_env_converter(ScrollTxEnvConverter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_primitives::U256;

    #[test]
    fn rpc_converter_uses_the_supplied_recovered_signer() {
        let transaction =
            ScrollTxEnvelope::Legacy(TxLegacy::default().into_signed(Signature::test_signature()));
        let signer = Address::repeat_byte(9);
        let rpc = ScrollRpcTxConverter
            .convert_rpc_tx(transaction, signer, ScrollTransactionInfo::default())
            .unwrap();
        assert_eq!(rpc.inner.inner.signer(), signer);
    }

    #[test]
    fn simulate_converter_dummy_signs_a_complete_request() {
        let mut request = ScrollTransactionRequest::default()
            .transaction_type(0)
            .gas_limit(21_000)
            .nonce(0)
            .to(Address::repeat_byte(1));
        request.as_mut().chain_id = Some(1);
        request.as_mut().gas_price = Some(1);
        request.as_mut().value = Some(U256::ZERO);

        assert!(matches!(
            ScrollSimTxConverter.convert_sim_tx(request).unwrap(),
            ScrollTxEnvelope::Legacy(_)
        ));
    }
}

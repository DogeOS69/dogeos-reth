use alloy_consensus::{
    SignableTransaction,
    error::ValueError,
    transaction::{Recovered, TxHashRef},
};
use alloy_primitives::{Address, Signature};
use alloy_rpc_types_eth::TransactionInfo;
use dogeos_protocol_types::{ScrollAdditionalInfo, ScrollTransactionInfo, ScrollTxEnvelope};
use dogeos_reth_primitives::ScrollReceipt;
use dogeos_rpc_types::{ScrollRpcTransaction, ScrollTransactionRequest};
use reth_errors::ProviderError;
use reth_rpc_convert::TxInfoMapper;
use reth_rpc_convert::transaction::{RpcTxConverter, SimTxConverter};
use reth_storage_api::ReceiptProvider;
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

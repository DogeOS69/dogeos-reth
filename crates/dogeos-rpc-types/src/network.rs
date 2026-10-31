use crate::{
    ScrollRpcTransaction, ScrollTransactionReceipt, ScrollTransactionRequest,
    transaction::BLOB_REQUEST_UNSUPPORTED,
};
use alloc::{vec, vec::Vec};
use alloy_consensus::{TxType, error::ValueError};
use alloy_network::{
    BuildResult, Network, NetworkWallet, TransactionBuilder, TransactionBuilderError,
};
use alloy_primitives::{Address, Bytes, ChainId, TxKind, U256};
use alloy_rpc_types_eth::AccessList;
use dogeos_protocol_types::{
    ScrollReceiptEnvelope, ScrollTxEnvelope, ScrollTxType, ScrollTypedTransaction,
};

/// Alloy network marker for the inherited Scroll protocol exposed by DogeOS.
#[derive(Clone, Copy, Debug)]
pub struct Scroll;

impl Network for Scroll {
    type TxType = ScrollTxType;
    type TxEnvelope = ScrollTxEnvelope;
    type UnsignedTx = ScrollTypedTransaction;
    type ReceiptEnvelope = ScrollReceiptEnvelope;
    type Header = alloy_consensus::Header;
    type TransactionRequest = ScrollTransactionRequest;
    type TransactionResponse = ScrollRpcTransaction;
    type ReceiptResponse = ScrollTransactionReceipt;
    type HeaderResponse = alloy_rpc_types_eth::Header;
    type BlockResponse =
        alloy_rpc_types_eth::Block<Self::TransactionResponse, Self::HeaderResponse>;
}

impl TransactionBuilder<Scroll> for ScrollTransactionRequest {
    fn chain_id(&self) -> Option<ChainId> {
        self.as_ref().chain_id()
    }

    fn set_chain_id(&mut self, chain_id: ChainId) {
        self.as_mut().set_chain_id(chain_id);
    }

    fn nonce(&self) -> Option<u64> {
        self.as_ref().nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.as_mut().set_nonce(nonce);
    }

    fn take_nonce(&mut self) -> Option<u64> {
        self.as_mut().nonce.take()
    }

    fn input(&self) -> Option<&Bytes> {
        self.as_ref().input()
    }

    fn set_input<T: Into<Bytes>>(&mut self, input: T) {
        self.as_mut().set_input(input);
    }

    fn from(&self) -> Option<Address> {
        self.as_ref().from()
    }

    fn set_from(&mut self, from: Address) {
        self.as_mut().set_from(from);
    }

    fn kind(&self) -> Option<TxKind> {
        self.as_ref().kind()
    }

    fn clear_kind(&mut self) {
        self.as_mut().clear_kind();
    }

    fn set_kind(&mut self, kind: TxKind) {
        self.as_mut().set_kind(kind);
    }

    fn value(&self) -> Option<U256> {
        self.as_ref().value()
    }

    fn set_value(&mut self, value: U256) {
        self.as_mut().set_value(value);
    }

    fn gas_price(&self) -> Option<u128> {
        self.as_ref().gas_price()
    }

    fn set_gas_price(&mut self, gas_price: u128) {
        self.as_mut().set_gas_price(gas_price);
    }

    fn max_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_fee_per_gas()
    }

    fn set_max_fee_per_gas(&mut self, max_fee_per_gas: u128) {
        self.as_mut().set_max_fee_per_gas(max_fee_per_gas);
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_priority_fee_per_gas()
    }

    fn set_max_priority_fee_per_gas(&mut self, max_priority_fee_per_gas: u128) {
        self.as_mut()
            .set_max_priority_fee_per_gas(max_priority_fee_per_gas);
    }

    fn gas_limit(&self) -> Option<u64> {
        self.as_ref().gas_limit()
    }

    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.as_mut().set_gas_limit(gas_limit);
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.as_ref().access_list()
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.as_mut().set_access_list(access_list);
    }

    fn complete_type(&self, ty: ScrollTxType) -> Result<(), Vec<&'static str>> {
        if ty == ScrollTxType::L1Message {
            return Err(vec!["L1 message transactions cannot be submitted over RPC"]);
        }
        self.as_ref()
            .complete_type(TxType::try_from(u8::from(ty)).expect("known Ethereum type"))
    }

    fn can_submit(&self) -> bool {
        self.as_ref().can_submit()
    }

    fn can_build(&self) -> bool {
        self.as_ref().can_build()
    }

    fn output_tx_type(&self) -> ScrollTxType {
        map_tx_type(self.as_ref().preferred_type())
    }

    fn output_tx_type_checked(&self) -> Option<ScrollTxType> {
        self.as_ref().buildable_type().map(map_tx_type)
    }

    fn prep_for_submission(&mut self) {
        // Ethereum preparation would retype a blob request and drop its blob fields; keep them so
        // building rejects it.
        if self.is_blob_request() {
            return;
        }
        self.as_mut().prep_for_submission();
    }

    fn build_unsigned(self) -> BuildResult<ScrollTypedTransaction, Scroll> {
        if self.is_blob_request() {
            return Err(TransactionBuilderError::custom(ValueError::new_static(
                (),
                BLOB_REQUEST_UNSUPPORTED,
            ))
            .into_unbuilt(self));
        }
        if let Err((tx_type, missing)) = self.as_ref().missing_keys() {
            let tx_type = map_tx_type(tx_type);
            return Err(
                TransactionBuilderError::InvalidTransactionRequest(tx_type, missing)
                    .into_unbuilt(self),
            );
        }
        self.clone().build_typed_tx().map_err(|error| {
            let request = error.into_value();
            TransactionBuilderError::InvalidTransactionRequest(
                request.output_tx_type(),
                vec!["required fields"],
            )
            .into_unbuilt(request)
        })
    }

    async fn build<W: NetworkWallet<Scroll>>(
        self,
        wallet: &W,
    ) -> Result<ScrollTxEnvelope, TransactionBuilderError<Scroll>> {
        Ok(wallet.sign_request(self).await?)
    }
}

const fn map_tx_type(tx_type: TxType) -> ScrollTxType {
    match tx_type {
        TxType::Legacy => ScrollTxType::Legacy,
        TxType::Eip2930 => ScrollTxType::Eip2930,
        TxType::Eip1559 | TxType::Eip4844 => ScrollTxType::Eip1559,
        TxType::Eip7702 => ScrollTxType::Eip7702,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7594::BlobTransactionSidecarVariant;
    use alloy_primitives::B256;

    #[test]
    fn network_maps_blob_requests_to_scroll_eip1559() {
        let mut request = ScrollTransactionRequest::default().transaction_type(3);
        request.as_mut().max_fee_per_blob_gas = Some(1);
        assert_eq!(request.output_tx_type(), ScrollTxType::Eip1559);
    }

    fn eip1559_request() -> ScrollTransactionRequest {
        let mut request = ScrollTransactionRequest::default()
            .to(Address::repeat_byte(1))
            .gas_limit(21_000)
            .nonce(0)
            .max_fee_per_gas(10)
            .max_priority_fee_per_gas(1);
        request.as_mut().chain_id = Some(1);
        request
    }

    fn blob_requests() -> [ScrollTransactionRequest; 4] {
        let typed = eip1559_request().transaction_type(3);

        let mut blob_fee = eip1559_request();
        blob_fee.as_mut().max_fee_per_blob_gas = Some(1);

        let mut blob_hashes = eip1559_request();
        blob_hashes.as_mut().max_fee_per_blob_gas = Some(1);
        blob_hashes.as_mut().blob_versioned_hashes = Some(vec![B256::repeat_byte(1)]);

        let mut sidecar = eip1559_request();
        sidecar.as_mut().sidecar = Some(BlobTransactionSidecarVariant::Eip4844(Default::default()));

        [typed, blob_fee, blob_hashes, sidecar]
    }

    fn assert_blob_rejected(request: ScrollTransactionRequest) {
        let error = request.clone().build_unsigned().unwrap_err();
        assert_eq!(error.request, request);
        assert!(matches!(error.error, TransactionBuilderError::Custom(_)));
        assert_eq!(error.error.to_string(), BLOB_REQUEST_UNSUPPORTED);
    }

    #[test]
    fn network_rejects_blob_requests() {
        for request in blob_requests() {
            assert_blob_rejected(request);
        }
    }

    #[test]
    fn blob_check_precedes_missing_keys() {
        // Blob hashes without a sidecar make the request an incomplete EIP-4844 one, so the
        // missing-keys check alone would ask for the sidecar.
        let mut request = eip1559_request();
        request.as_mut().max_fee_per_blob_gas = Some(1);
        request.as_mut().blob_versioned_hashes = Some(vec![B256::repeat_byte(1)]);
        assert!(request.as_ref().missing_keys().is_err());
        assert_blob_rejected(request);
    }

    #[test]
    fn prepared_blob_requests_are_still_rejected() {
        for mut request in blob_requests() {
            let unprepared = request.clone();
            request.prep_for_submission();
            assert_eq!(request, unprepared);
            assert_blob_rejected(request);
        }
    }

    #[test]
    fn prepared_eip1559_request_still_builds() {
        let mut request = eip1559_request();
        request.prep_for_submission();
        assert_eq!(request.as_ref().transaction_type, Some(2));
        assert!(matches!(
            request.build_unsigned().unwrap(),
            ScrollTypedTransaction::Eip1559(_)
        ));
    }

    #[test]
    fn l1_messages_are_not_rpc_submission_types() {
        assert!(
            ScrollTransactionRequest::default()
                .complete_type(ScrollTxType::L1Message)
                .is_err()
        );
    }
}

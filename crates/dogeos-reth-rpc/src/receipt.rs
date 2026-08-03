use alloy_consensus::{Receipt, TxReceipt};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use dogeos_protocol_types::ScrollReceiptEnvelope;
use dogeos_reth_primitives::{DogeosPrimitives, ScrollReceipt};
use dogeos_rpc_types::{ScrollTransactionReceipt, ScrollTransactionReceiptFields};
use reth_rpc_convert::transaction::{ConvertReceiptInput, ReceiptConverter};
use reth_rpc_eth_types::{EthApiError, receipt::build_receipt};

/// Converts DogeOS primitive receipts into Scroll-compatible RPC receipts.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ScrollReceiptConverter;

impl ReceiptConverter<DogeosPrimitives> for ScrollReceiptConverter {
    type RpcReceipt = ScrollTransactionReceipt;
    type Error = EthApiError;

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, DogeosPrimitives>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        inputs
            .into_iter()
            .map(|input| Ok(ScrollReceiptBuilder::new(input).build()))
            .collect()
    }
}

/// Builds a Scroll RPC receipt from its Reth conversion context.
#[derive(Debug)]
pub struct ScrollReceiptBuilder {
    pub core_receipt: TransactionReceipt<ScrollReceiptEnvelope<Log>>,
    pub scroll_receipt_fields: ScrollTransactionReceiptFields,
}

impl ScrollReceiptBuilder {
    pub fn new(input: ConvertReceiptInput<'_, DogeosPrimitives>) -> Self {
        let scroll_receipt_fields = ScrollTransactionReceiptFields {
            l1_fee: Some(input.receipt.l1_fee().saturating_to()),
        };
        let core_receipt = build_receipt(input, None, |receipt, next_log_index, meta| {
            let map_logs = move |receipt: Receipt| {
                let Receipt {
                    status,
                    cumulative_gas_used,
                    logs,
                } = receipt;
                let logs = Log::collect_for_receipt(next_log_index, meta, logs);
                Receipt {
                    status,
                    cumulative_gas_used,
                    logs,
                }
            };
            match receipt {
                ScrollReceipt::Legacy(receipt) => {
                    ScrollReceiptEnvelope::Legacy(map_logs(receipt.inner).into_with_bloom())
                }
                ScrollReceipt::Eip2930(receipt) => {
                    ScrollReceiptEnvelope::Eip2930(map_logs(receipt.inner).into_with_bloom())
                }
                ScrollReceipt::Eip1559(receipt) => {
                    ScrollReceiptEnvelope::Eip1559(map_logs(receipt.inner).into_with_bloom())
                }
                ScrollReceipt::Eip7702(receipt) => {
                    ScrollReceiptEnvelope::Eip7702(map_logs(receipt.inner).into_with_bloom())
                }
                ScrollReceipt::L1Message(receipt) => {
                    ScrollReceiptEnvelope::L1Message(map_logs(receipt).into_with_bloom())
                }
            }
        });
        Self {
            core_receipt,
            scroll_receipt_fields,
        }
    }

    pub fn build(self) -> ScrollTransactionReceipt {
        ScrollTransactionReceipt {
            inner: self.core_receipt,
            l1_fee: self.scroll_receipt_fields.l1_fee,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Eip658Value, SignableTransaction, TxLegacy, transaction::Recovered};
    use alloy_primitives::{Address, B256, Signature, U256};
    use dogeos_protocol_types::ScrollTransactionReceipt as ConsensusScrollReceipt;
    use reth_primitives_traits::TransactionMeta;

    #[test]
    fn converter_preserves_l1_fee_and_block_metadata() {
        let transaction = dogeos_protocol_types::ScrollTxEnvelope::Legacy(
            TxLegacy::default().into_signed(Signature::test_signature()),
        );
        let receipt = ScrollReceipt::Legacy(ConsensusScrollReceipt::new(
            Receipt {
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 21_000,
                logs: Vec::new(),
            },
            U256::from(7),
        ));
        let input = ConvertReceiptInput {
            receipt,
            tx: Recovered::new_unchecked(&transaction, Address::repeat_byte(1)),
            gas_used: 21_000,
            next_log_index: 0,
            meta: TransactionMeta {
                tx_hash: B256::repeat_byte(2),
                block_hash: B256::repeat_byte(3),
                block_number: 4,
                index: 5,
                ..Default::default()
            },
        };

        let converted = ScrollReceiptConverter
            .convert_receipts(vec![input])
            .unwrap();
        assert_eq!(converted[0].l1_fee, Some(7));
        assert_eq!(converted[0].inner.block_number, Some(4));
        assert_eq!(converted[0].inner.transaction_index, Some(5));
    }
}

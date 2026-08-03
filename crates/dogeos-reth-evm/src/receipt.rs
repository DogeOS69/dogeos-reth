use crate::{ReceiptBuilderCtx, ScrollReceiptBuilder};
use alloy_consensus::{Eip658Value, Receipt};
use alloy_evm::Evm;
use dogeos_protocol_types::ScrollTransactionReceipt;
use dogeos_reth_primitives::{ScrollReceipt, ScrollTransactionSigned, ScrollTxType};

/// Builds DogeOS receipts from Scroll transaction execution results.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollRethReceiptBuilder;

impl ScrollReceiptBuilder for ScrollRethReceiptBuilder {
    type Transaction = ScrollTransactionSigned;
    type Receipt = ScrollReceipt;

    fn build_receipt<E: Evm>(&self, ctx: ReceiptBuilderCtx<E>) -> Self::Receipt {
        let inner = Receipt {
            status: Eip658Value::Eip658(ctx.result.is_success()),
            cumulative_gas_used: ctx.cumulative_gas_used,
            logs: ctx.result.into_logs(),
        };
        let with_l1_fee = |inner| ScrollTransactionReceipt::new(inner, ctx.l1_fee);

        match ScrollTxType::try_from(ctx.tx_type).expect("unexpected Scroll transaction type") {
            ScrollTxType::Legacy => ScrollReceipt::Legacy(with_l1_fee(inner)),
            ScrollTxType::Eip2930 => ScrollReceipt::Eip2930(with_l1_fee(inner)),
            ScrollTxType::Eip1559 => ScrollReceipt::Eip1559(with_l1_fee(inner)),
            ScrollTxType::Eip7702 => ScrollReceipt::Eip7702(with_l1_fee(inner)),
            ScrollTxType::L1Message => ScrollReceipt::L1Message(inner),
        }
    }
}

use alloy_consensus::BlockHeader;
use alloy_eips::Encodable2718;
use alloy_primitives::U256;
use dogeos_hardforks::DogeosHardforks;
use dogeos_protocol_types::ScrollTransaction;
use dogeos_reth_evm::{
    RethL1BlockInfo, compute_compressed_size, compute_compression_ratio,
    spec_id_at_timestamp_and_number,
};
use parking_lot::RwLock;
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::{
    BlockTy, GotExpected, SealedBlock, transaction::error::InvalidTransactionError,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use revm_scroll::l1block::L1BlockInfo;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const MAX_ROLLUP_FEE_PRE_TSUKI: U256 = U256::from_limbs([u64::MAX, 0, 0, 0]);
const MAX_ROLLUP_FEE_TSUKI: U256 = U256::from_limbs([u64::MAX, u32::MAX as u64, 0, 0]);

/// State cached from the latest canonical head for L1 data-fee validation.
#[derive(Debug, Default)]
pub struct DogeosL1BlockInfo {
    l1_block_info: RwLock<L1BlockInfo>,
    timestamp: AtomicU64,
    number: AtomicU64,
}

/// Adds DogeOS transaction types and state-dependent L1 fee checks to Reth's validator.
#[derive(Debug)]
pub struct DogeosTransactionValidator<Client, Tx, Evm> {
    inner: EthTransactionValidator<Client, Tx, Evm>,
    block_info: Arc<DogeosL1BlockInfo>,
    require_l1_data_gas_fee: bool,
    require_l1_data_fee_buffer: bool,
}

impl<Client, Tx, Evm> DogeosTransactionValidator<Client, Tx, Evm> {
    pub fn with_block_info(
        inner: EthTransactionValidator<Client, Tx, Evm>,
        block_info: DogeosL1BlockInfo,
    ) -> Self {
        Self {
            inner,
            block_info: Arc::new(block_info),
            require_l1_data_gas_fee: true,
            require_l1_data_fee_buffer: false,
        }
    }

    pub fn require_l1_data_gas_fee(self, require: bool) -> Self {
        Self {
            require_l1_data_gas_fee: require,
            ..self
        }
    }

    pub fn require_l1_data_fee_buffer(self, require: bool) -> Self {
        Self {
            require_l1_data_fee_buffer: require,
            ..self
        }
    }

    pub const fn requires_l1_data_gas_fee(&self) -> bool {
        self.require_l1_data_gas_fee
    }

    pub const fn requires_l1_data_fee_buffer(&self) -> bool {
        self.require_l1_data_fee_buffer
    }
}

impl<Client, Tx, Evm> DogeosTransactionValidator<Client, Tx, Evm>
where
    Client: ChainSpecProvider<ChainSpec: DogeosHardforks> + StateProviderFactory,
    Tx: EthPoolTransaction + ScrollTransaction,
    Evm: reth_evm::ConfigureEvm,
{
    pub fn new(inner: EthTransactionValidator<Client, Tx, Evm>) -> Self {
        Self::with_block_info(inner, DogeosL1BlockInfo::default())
    }

    pub fn chain_spec(&self) -> Arc<Client::ChainSpec> {
        self.inner.chain_spec()
    }

    pub const fn client(&self) -> &Client {
        self.inner.client()
    }

    pub fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        if transaction.is_eip4844() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::Eip4844Disabled.into(),
            );
        }
        if transaction.is_l1_message() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TxTypeNotSupported.into(),
            );
        }

        let outcome = self.inner.validate_one(origin, transaction);
        if !self.require_l1_data_gas_fee {
            return outcome;
        }

        if let TransactionValidationOutcome::Valid {
            balance,
            state_nonce,
            transaction: valid_transaction,
            propagate,
            bytecode_hash,
            authorities,
        } = outcome
        {
            let mut encoded = Vec::with_capacity(valid_transaction.transaction().encoded_length());
            valid_transaction
                .transaction()
                .clone_into_consensus()
                .encode_2718(&mut encoded);
            let compression = (
                compute_compression_ratio(valid_transaction.transaction().input()),
                compute_compressed_size(&encoded),
            );
            let timestamp = self.block_info.timestamp.load(Ordering::Relaxed);
            let number = self.block_info.number.load(Ordering::Relaxed);
            let l1_data_fee = match self.block_info.l1_block_info.write().l1_tx_data_fee(
                self.chain_spec(),
                timestamp,
                number,
                &encoded,
                Some(compression),
                false,
            ) {
                Ok(fee) => fee,
                Err(error) => {
                    return TransactionValidationOutcome::Error(
                        *valid_transaction.hash(),
                        Box::new(error),
                    );
                }
            };
            let maximum = if self.chain_spec().is_tsuki_active_at_timestamp(timestamp) {
                MAX_ROLLUP_FEE_TSUKI
            } else {
                MAX_ROLLUP_FEE_PRE_TSUKI
            };
            if l1_data_fee >= maximum {
                return TransactionValidationOutcome::Invalid(
                    valid_transaction.into_transaction(),
                    InvalidTransactionError::GasUintOverflow.into(),
                );
            }
            let mut required = valid_transaction
                .transaction()
                .cost()
                .saturating_add(l1_data_fee);
            if self.require_l1_data_fee_buffer {
                required = required.saturating_add(l1_data_fee);
            }
            if required > balance {
                return TransactionValidationOutcome::Invalid(
                    valid_transaction.into_transaction(),
                    InvalidTransactionError::InsufficientFunds(
                        GotExpected {
                            got: balance,
                            expected: required,
                        }
                        .into(),
                    )
                    .into(),
                );
            }
            return TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction: valid_transaction,
                propagate,
                bytecode_hash,
                authorities,
            };
        }
        outcome
    }

    fn update_l1_block_info(&self, block: &SealedBlock<BlockTy<Evm::Primitives>>) {
        let header = block.header();
        self.block_info
            .timestamp
            .store(header.timestamp(), Ordering::Relaxed);
        self.block_info
            .number
            .store(header.number(), Ordering::Relaxed);
        let Ok(provider) = self.client().state_by_block_hash(block.hash()) else {
            return;
        };
        let mut database = StateProviderDatabase::new(provider);
        let spec =
            spec_id_at_timestamp_and_number(header.timestamp(), header.number(), self.chain_spec());
        if let Ok(info) = L1BlockInfo::try_fetch(&mut database, spec) {
            *self.block_info.l1_block_info.write() = info;
        }
    }
}

impl<Client, Tx, Evm> TransactionValidator for DogeosTransactionValidator<Client, Tx, Evm>
where
    Client: ChainSpecProvider<ChainSpec: DogeosHardforks> + StateProviderFactory,
    Tx: EthPoolTransaction + ScrollTransaction,
    Evm: reth_evm::ConfigureEvm,
{
    type Transaction = Tx;
    type Block = BlockTy<Evm::Primitives>;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.validate_one(origin, transaction)
    }

    fn on_new_head_block(&self, block: &SealedBlock<Self::Block>) {
        self.inner.on_new_head_block(block);
        self.update_l1_block_info(block);
    }
}

//! DogeOS transaction-pool types and state-aware validation.

mod transaction;
pub use transaction::DogeosPooledTransaction;
mod validator;
pub use validator::{DogeosL1FeeError, DogeosL1FeeSnapshot, DogeosTransactionValidator};
mod code_witness;
pub use code_witness::{
    CodeWitnessValidationConfig, CodeWitnessValidationError, validate_code_witness,
};
mod executor;
pub use executor::{DogeosValidationExecutor, ValidationCapacityError};

use dogeos_reth_evm::ScrollEvmConfig;
use reth_transaction_pool::{CoinbaseTipOrdering, Pool};

pub type DogeosTransactionPool<
    Client,
    BlobStore,
    Transaction = DogeosPooledTransaction,
    Evm = ScrollEvmConfig,
> = Pool<
    DogeosValidationExecutor<DogeosTransactionValidator<Client, Transaction, Evm>>,
    CoinbaseTipOrdering<Transaction>,
    BlobStore,
>;

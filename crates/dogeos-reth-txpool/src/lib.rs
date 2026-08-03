//! DogeOS transaction-pool types and state-aware validation.

mod transaction;
pub use transaction::DogeosPooledTransaction;
mod validator;
pub use validator::{DogeosL1BlockInfo, DogeosTransactionValidator};

use dogeos_reth_evm::ScrollEvmConfig;
use reth_transaction_pool::{CoinbaseTipOrdering, Pool, TransactionValidationTaskExecutor};

pub type DogeosTransactionPool<
    Client,
    BlobStore,
    Transaction = DogeosPooledTransaction,
    Evm = ScrollEvmConfig,
> = Pool<
    TransactionValidationTaskExecutor<DogeosTransactionValidator<Client, Transaction, Evm>>,
    CoinbaseTipOrdering<Transaction>,
    BlobStore,
>;

//! DogeOS transaction-pool types and state-aware validation.

mod transaction;
pub use transaction::DogeosPooledTransaction;
mod validator;
pub use validator::{DogeosL1BlockInfo, DogeosTransactionValidator};

use reth_primitives_traits::SealedBlock;
use reth_transaction_pool::{
    PoolTransaction, TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Local admission work could not be scheduled. This never marks a transaction permanently bad.
#[derive(Debug, thiserror::Error)]
pub enum ValidationCapacityError {
    #[error("transaction admission capacity exhausted; retry later")]
    Busy,
    #[error("transaction admission worker stopped: {0}")]
    Worker(String),
}

/// Bounds both waiting and executing validations before accepting any expensive work.
///
/// Permits move into the blocking job, so cancellation of an RPC or peer request cannot release
/// capacity while its EVM execution is still running. Waiting requests are bounded too, unlike
/// merely bounding the worker channel while allowing arbitrary senders to wait for it.
#[derive(Debug)]
pub struct DogeosValidationExecutor<V> {
    validator: Arc<V>,
    inflight: Arc<Semaphore>,
    workers: Arc<Semaphore>,
}

impl<V> Clone for DogeosValidationExecutor<V> {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
            inflight: self.inflight.clone(),
            workers: self.workers.clone(),
        }
    }
}

impl<V> DogeosValidationExecutor<V> {
    /// Uses one validation worker plus the existing additional-validation-tasks setting.
    pub fn new(validator: V, max_inflight: usize, additional_validation_tasks: usize) -> Self {
        assert!(max_inflight > 0, "admission capacity must be nonzero");
        Self {
            validator: Arc::new(validator),
            inflight: Arc::new(Semaphore::new(max_inflight)),
            workers: Arc::new(Semaphore::new(
                additional_validation_tasks
                    .saturating_add(1)
                    .min(max_inflight),
            )),
        }
    }

    pub fn validator(&self) -> &V {
        &self.validator
    }
}

impl<V: TransactionValidator + 'static> TransactionValidator for DogeosValidationExecutor<V> {
    type Transaction = V::Transaction;
    type Block = V::Block;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        let hash = *transaction.hash();
        let Ok(inflight) = self.inflight.clone().try_acquire_owned() else {
            return TransactionValidationOutcome::Error(
                hash,
                Box::new(ValidationCapacityError::Busy),
            );
        };
        let Ok(worker) = self.workers.clone().acquire_owned().await else {
            return TransactionValidationOutcome::Error(
                hash,
                Box::new(ValidationCapacityError::Busy),
            );
        };
        let validator = self.validator.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let (_inflight, _worker) = (inflight, worker);
            runtime.block_on(validator.validate_transaction(origin, transaction))
        })
        .await
        .unwrap_or_else(|error| {
            TransactionValidationOutcome::Error(
                hash,
                Box::new(ValidationCapacityError::Worker(error.to_string())),
            )
        })
    }

    fn on_new_head_block(&self, block: &SealedBlock<Self::Block>) {
        self.validator.on_new_head_block(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DogeosPooledTransaction;
    use alloy_consensus::{Signed, TxLegacy, transaction::Recovered};
    use alloy_primitives::{Address, Signature};
    use dogeos_reth_primitives::{DogeosBlock, ScrollTransactionSigned};
    use tokio::sync::mpsc;

    #[derive(Debug)]
    struct ControlledValidator {
        started: mpsc::UnboundedSender<()>,
        release: Arc<Semaphore>,
    }

    impl TransactionValidator for ControlledValidator {
        type Transaction = DogeosPooledTransaction;
        type Block = DogeosBlock;

        async fn validate_transaction(
            &self,
            _origin: TransactionOrigin,
            tx: Self::Transaction,
        ) -> TransactionValidationOutcome<Self::Transaction> {
            self.started.send(()).unwrap();
            self.release.acquire().await.unwrap().forget();
            TransactionValidationOutcome::Error(*tx.hash(), Box::new(std::io::Error::other("done")))
        }
    }

    fn tx() -> DogeosPooledTransaction {
        let signed = ScrollTransactionSigned::Legacy(Signed::new_unhashed(
            TxLegacy::default(),
            Signature::test_signature(),
        ));
        DogeosPooledTransaction::new(Recovered::new_unchecked(signed, Address::ZERO), 0)
    }

    fn spawn_validation(
        executor: &DogeosValidationExecutor<ControlledValidator>,
    ) -> tokio::task::JoinHandle<TransactionValidationOutcome<DogeosPooledTransaction>> {
        let executor = executor.clone();
        tokio::spawn(async move {
            executor
                .validate_transaction(TransactionOrigin::External, tx())
                .await
        })
    }

    fn assert_busy(outcome: TransactionValidationOutcome<DogeosPooledTransaction>) {
        let TransactionValidationOutcome::Error(_, error) = outcome else {
            panic!("expected busy")
        };
        assert!(matches!(
            error.downcast_ref(),
            Some(ValidationCapacityError::Busy)
        ));
    }

    #[tokio::test]
    async fn cancellation_keeps_running_work_counted_and_waiting_work_is_bounded() {
        let (started, mut rx) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let executor = DogeosValidationExecutor::new(
            ControlledValidator {
                started,
                release: release.clone(),
            },
            2,
            0,
        );
        let first = spawn_validation(&executor);
        rx.recv().await.unwrap();
        let second = spawn_validation(&executor);
        // Wait for the second request to claim its waiting slot, without starting another worker.
        while executor.inflight.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        assert!(rx.try_recv().is_err());
        assert_busy(
            executor
                .validate_transaction(TransactionOrigin::External, tx())
                .await,
        );

        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_busy(
            executor
                .validate_transaction(TransactionOrigin::External, tx())
                .await,
        );
        second.abort();
        assert!(second.await.unwrap_err().is_cancelled());
        assert_eq!(executor.inflight.available_permits(), 1);

        let third = spawn_validation(&executor);
        release.add_permits(1);
        rx.recv().await.unwrap();
        release.add_permits(1);
        third.await.unwrap();
        assert_eq!(executor.inflight.available_permits(), 2);
        assert_eq!(executor.workers.available_permits(), 1);
    }

    #[tokio::test]
    async fn additional_validation_tasks_control_concurrency() {
        let (started, mut rx) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let executor = DogeosValidationExecutor::new(
            ControlledValidator {
                started,
                release: release.clone(),
            },
            3,
            1,
        );
        let first = spawn_validation(&executor);
        let second = spawn_validation(&executor);
        rx.recv().await.unwrap();
        rx.recv().await.unwrap();
        assert_eq!(executor.workers.available_permits(), 0);
        release.add_permits(2);
        first.await.unwrap();
        second.await.unwrap();
        assert_eq!(executor.inflight.available_permits(), 3);
    }
}

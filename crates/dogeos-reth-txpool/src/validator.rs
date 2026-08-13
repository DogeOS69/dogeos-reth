use alloy_consensus::BlockHeader;
use alloy_eips::Encodable2718;
use alloy_primitives::{B256, U256};
use dogeos_hardforks::DogeosHardforks;
use dogeos_protocol_types::ScrollTransaction;
use dogeos_reth_evm::{
    RethL1BlockInfo, compute_compressed_size, compute_compression_ratio,
    spec_id_at_timestamp_and_number,
};
use parking_lot::{Mutex, RwLock};
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::{
    BlockTy, GotExpected, SealedBlock, transaction::error::InvalidTransactionError,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{AccountReader, BlockReaderIdExt, StateProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_transaction_pool::{
    EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use revm_scroll::l1block::{L1_GAS_PRICE_ORACLE_ADDRESS, L1BlockInfo};
use std::sync::Arc;
use thiserror::Error;

const MAX_ROLLUP_FEE_PRE_TSUKI: U256 = U256::from_limbs([u64::MAX, 0, 0, 0]);
const MAX_ROLLUP_FEE_TSUKI: U256 = U256::from_limbs([u64::MAX, u32::MAX as u64, 0, 0]);

/// A complete L1 fee snapshot loaded from one sealed canonical head and its exact state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DogeosL1FeeSnapshot {
    head_hash: B256,
    timestamp: u64,
    number: u64,
    l1_block_info: L1BlockInfo,
}

impl DogeosL1FeeSnapshot {
    /// Loads a complete L1 fee snapshot from the latest sealed canonical header and its exact
    /// state-by-hash view.
    pub fn load_latest<Client>(client: &Client) -> Result<Self, DogeosL1FeeError>
    where
        Client:
            ChainSpecProvider<ChainSpec: DogeosHardforks> + BlockReaderIdExt + StateProviderFactory,
    {
        let header = client
            .latest_header()
            .map_err(|source| DogeosL1FeeError::LatestHeaderRead { source })?
            .ok_or(DogeosL1FeeError::LatestHeaderNotFound)?;

        Self::load_for_head(client, header.hash(), header.timestamp(), header.number())
    }

    fn load_for_head<Client>(
        client: &Client,
        head_hash: B256,
        timestamp: u64,
        number: u64,
    ) -> Result<Self, DogeosL1FeeError>
    where
        Client: ChainSpecProvider<ChainSpec: DogeosHardforks> + StateProviderFactory,
    {
        let provider = client.state_by_block_hash(head_hash).map_err(|source| {
            DogeosL1FeeError::ExactStateOpen {
                head_hash,
                number,
                source,
            }
        })?;

        if provider
            .basic_account(&L1_GAS_PRICE_ORACLE_ADDRESS)
            .map_err(|source| DogeosL1FeeError::OracleAccountRead {
                head_hash,
                number,
                source,
            })?
            .is_none()
        {
            return Err(DogeosL1FeeError::OracleAccountNotFound { head_hash, number });
        }

        let spec = spec_id_at_timestamp_and_number(timestamp, number, client.chain_spec());
        let mut database = StateProviderDatabase::new(provider);
        let l1_block_info = L1BlockInfo::try_fetch(&mut database, spec).map_err(|source| {
            DogeosL1FeeError::OracleStorageRead {
                head_hash,
                number,
                source,
            }
        })?;

        Ok(Self {
            head_hash,
            timestamp,
            number,
            l1_block_info,
        })
    }

    /// Hash of the sealed canonical head whose state produced this snapshot.
    pub const fn head_hash(&self) -> B256 {
        self.head_hash
    }

    /// Timestamp of the sealed canonical head whose state produced this snapshot.
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Number of the sealed canonical head whose state produced this snapshot.
    pub const fn number(&self) -> u64 {
        self.number
    }
}

#[derive(Debug)]
enum DogeosL1FeeCache {
    Ready(Arc<DogeosL1FeeSnapshot>),
    Unavailable(Arc<DogeosL1FeeError>),
    Disabled,
}

/// Errors encountered while hydrating or using DogeOS L1 fee state.
#[derive(Debug, Error)]
pub enum DogeosL1FeeError {
    /// Reading the latest canonical header failed.
    #[error("failed to read latest canonical header for DogeOS L1 fee hydration: {source}")]
    LatestHeaderRead {
        /// Provider error returned by the latest-header lookup.
        #[source]
        source: ProviderError,
    },
    /// The provider did not have a latest canonical header.
    #[error("latest canonical header not found for DogeOS L1 fee hydration")]
    LatestHeaderNotFound,
    /// State for the exact sealed canonical head could not be opened.
    #[error(
        "failed to open exact state for DogeOS L1 fee canonical head {head_hash} (block #{number}): {source}"
    )]
    ExactStateOpen {
        /// Sealed canonical head hash.
        head_hash: B256,
        /// Sealed canonical head number.
        number: u64,
        /// Provider error returned by the exact-state lookup.
        #[source]
        source: ProviderError,
    },
    /// Reading the gas-price-oracle account failed.
    #[error(
        "failed to read DogeOS L1 gas-price-oracle account at canonical head {head_hash} (block #{number}): {source}"
    )]
    OracleAccountRead {
        /// Sealed canonical head hash.
        head_hash: B256,
        /// Sealed canonical head number.
        number: u64,
        /// Provider error returned by the account lookup.
        #[source]
        source: ProviderError,
    },
    /// The gas-price-oracle account does not exist at the selected head.
    #[error(
        "DogeOS L1 gas-price-oracle account not found at canonical head {head_hash} (block #{number})"
    )]
    OracleAccountNotFound {
        /// Sealed canonical head hash.
        head_hash: B256,
        /// Sealed canonical head number.
        number: u64,
    },
    /// Reading gas-price-oracle storage failed.
    #[error(
        "failed to fetch DogeOS L1 gas-price-oracle storage at canonical head {head_hash} (block #{number}): {source}"
    )]
    OracleStorageRead {
        /// Sealed canonical head hash.
        head_hash: B256,
        /// Sealed canonical head number.
        number: u64,
        /// Provider error returned by the oracle storage lookup.
        #[source]
        source: ProviderError,
    },
    /// A prior canonical-head refresh failed, so the old snapshot cannot be used.
    #[error("{}", cache_unavailable_message(.source.as_ref()))]
    CacheUnavailable {
        /// Typed refresh failure retained by the unavailable cache state.
        #[source]
        source: Arc<DogeosL1FeeError>,
    },
}

fn cache_unavailable_message(source: &DogeosL1FeeError) -> String {
    source.head_context().map_or_else(
        || "DogeOS L1 fee state unavailable".to_string(),
        |(head_hash, number)| {
            format!(
                "DogeOS L1 fee state unavailable for canonical head {head_hash} (block #{number})"
            )
        },
    )
}

impl DogeosL1FeeError {
    fn head_context(&self) -> Option<(B256, u64)> {
        match self {
            Self::ExactStateOpen {
                head_hash, number, ..
            }
            | Self::OracleAccountRead {
                head_hash, number, ..
            }
            | Self::OracleAccountNotFound { head_hash, number }
            | Self::OracleStorageRead {
                head_hash, number, ..
            } => Some((*head_hash, *number)),
            Self::CacheUnavailable { source } => source.head_context(),
            Self::LatestHeaderRead { .. } | Self::LatestHeaderNotFound => None,
        }
    }
}

/// Adds DogeOS transaction types and state-dependent L1 fee checks to Reth's validator.
#[derive(Debug)]
pub struct DogeosTransactionValidator<Client, Tx, Evm> {
    inner: EthTransactionValidator<Client, Tx, Evm>,
    l1_fee_cache: RwLock<DogeosL1FeeCache>,
    l1_fee_refresh_lock: Mutex<()>,
    require_l1_data_fee_buffer: bool,
}

impl<Client, Tx, Evm> DogeosTransactionValidator<Client, Tx, Evm> {
    /// Constructs a production validator from a fully hydrated snapshot.
    pub fn new(
        inner: EthTransactionValidator<Client, Tx, Evm>,
        snapshot: DogeosL1FeeSnapshot,
        require_l1_data_fee_buffer: bool,
    ) -> Self {
        Self {
            inner,
            l1_fee_cache: RwLock::new(DogeosL1FeeCache::Ready(Arc::new(snapshot))),
            l1_fee_refresh_lock: Mutex::new(()),
            require_l1_data_fee_buffer,
        }
    }

    /// Constructs a development-mode validator with L1 fee checking explicitly disabled.
    pub const fn disabled(
        inner: EthTransactionValidator<Client, Tx, Evm>,
        require_l1_data_fee_buffer: bool,
    ) -> Self {
        Self {
            inner,
            l1_fee_cache: RwLock::new(DogeosL1FeeCache::Disabled),
            l1_fee_refresh_lock: Mutex::new(()),
            require_l1_data_fee_buffer,
        }
    }

    pub fn chain_spec(&self) -> Arc<Client::ChainSpec>
    where
        Client: ChainSpecProvider,
    {
        self.inner.chain_spec()
    }

    pub const fn client(&self) -> &Client {
        self.inner.client()
    }

    pub fn requires_l1_data_gas_fee(&self) -> bool {
        !matches!(*self.l1_fee_cache.read(), DogeosL1FeeCache::Disabled)
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

        let snapshot = match &*self.l1_fee_cache.read() {
            DogeosL1FeeCache::Ready(snapshot) => Some(Arc::clone(snapshot)),
            DogeosL1FeeCache::Unavailable(source) => {
                return TransactionValidationOutcome::Error(
                    *transaction.hash(),
                    Box::new(DogeosL1FeeError::CacheUnavailable {
                        source: Arc::clone(source),
                    }),
                );
            }
            DogeosL1FeeCache::Disabled => None,
        };

        let outcome = self.inner.validate_one(origin, transaction);
        let Some(snapshot) = snapshot else {
            return outcome;
        };

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
            let chain_spec = self.chain_spec();
            let l1_data_fee = match snapshot.l1_block_info.l1_tx_data_fee(
                chain_spec.as_ref(),
                snapshot.timestamp,
                snapshot.number,
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
            let maximum = if chain_spec.is_tsuki_active_at_timestamp(snapshot.timestamp) {
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

    /// Reconciles the fee cache with the provider's latest sealed canonical head.
    ///
    /// This is called by the dedicated canonical-notification task, including after notification
    /// lag, and by the ordinary Reth validator callback. Refreshes are serialized, and the target
    /// hash is verified again after exact-state I/O, so an older caller cannot publish over a
    /// newer canonical result.
    pub fn refresh_l1_fee_cache_from_latest(&self)
    where
        Client: BlockReaderIdExt,
    {
        self.refresh_l1_fee_cache_from_latest_with_hooks(|_| {}, |_| {});
    }

    fn refresh_l1_fee_cache_from_latest_with_hooks(
        &self,
        mut target_hook: impl FnMut(B256),
        mut before_publish_hook: impl FnMut(B256),
    ) where
        Client: BlockReaderIdExt,
    {
        let _refresh_guard = self.l1_fee_refresh_lock.lock();
        if matches!(*self.l1_fee_cache.read(), DogeosL1FeeCache::Disabled) {
            return;
        }

        loop {
            let target = match self
                .client()
                .latest_header()
                .map_err(|source| DogeosL1FeeError::LatestHeaderRead { source })
                .and_then(|header| header.ok_or(DogeosL1FeeError::LatestHeaderNotFound))
            {
                Ok(header) => header,
                Err(error) => {
                    self.publish_l1_fee_refresh(Err(error));
                    return;
                }
            };
            target_hook(target.hash());

            if matches!(
                &*self.l1_fee_cache.read(),
                DogeosL1FeeCache::Ready(snapshot) if snapshot.head_hash == target.hash()
            ) {
                return;
            }

            let snapshot = DogeosL1FeeSnapshot::load_for_head(
                self.client(),
                target.hash(),
                target.timestamp(),
                target.number(),
            );
            let verified_head = match self
                .client()
                .latest_header()
                .map_err(|source| DogeosL1FeeError::LatestHeaderRead { source })
                .and_then(|header| header.ok_or(DogeosL1FeeError::LatestHeaderNotFound))
            {
                Ok(header) => header,
                Err(error) => {
                    self.publish_l1_fee_refresh(Err(error));
                    return;
                }
            };

            if verified_head.hash() != target.hash() {
                continue;
            }

            before_publish_hook(target.hash());
            self.publish_l1_fee_refresh(snapshot);
            return;
        }
    }

    fn publish_l1_fee_refresh(&self, snapshot: Result<DogeosL1FeeSnapshot, DogeosL1FeeError>) {
        match snapshot {
            Ok(snapshot) => {
                *self.l1_fee_cache.write() = DogeosL1FeeCache::Ready(Arc::new(snapshot));
            }
            Err(error) => {
                let head = error.head_context();
                tracing::error!(
                    target: "reth::txpool",
                    head_hash = ?head.map(|(hash, _)| hash),
                    block_number = ?head.map(|(_, number)| number),
                    %error,
                    "failed to refresh DogeOS L1 fee state for canonical head"
                );
                *self.l1_fee_cache.write() = DogeosL1FeeCache::Unavailable(Arc::new(error));
            }
        }
    }
}

impl<Client, Tx, Evm> TransactionValidator for DogeosTransactionValidator<Client, Tx, Evm>
where
    Client: ChainSpecProvider<ChainSpec: DogeosHardforks> + BlockReaderIdExt + StateProviderFactory,
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
        self.refresh_l1_fee_cache_from_latest();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DogeosPooledTransaction;
    use alloy_consensus::{Signed, TxLegacy};
    use alloy_eips::Encodable2718;
    use alloy_primitives::{Address, Signature, TxKind};
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_DEV, DogeosChainSpec};
    use dogeos_reth_evm::ScrollEvmConfig;
    use dogeos_reth_primitives::{DogeosBlock, DogeosPrimitives, ScrollTransactionSigned};
    use reth_chainspec::{ChainInfo, EthChainSpec};
    use reth_primitives_traits::{Block as _, Recovered};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
    use reth_storage_api::{
        BlockHashReader, BlockIdReader, BlockNumReader, StateProviderBox,
        errors::provider::ProviderResult,
    };
    use reth_transaction_pool::{
        PoolTransaction, TransactionValidationOutcome, blobstore::InMemoryBlobStore,
        validate::EthTransactionValidatorBuilder,
    };
    use revm_scroll::l1block::{
        L1_BASE_FEE_SLOT, L1_BLOB_BASE_FEE_SLOT, L1_BLOB_SCALAR_SLOT, L1_COMMIT_SCALAR_SLOT,
        PENALTY_FACTOR_SLOT, PENALTY_THRESHOLD_SLOT,
    };
    use std::{collections::BTreeMap, sync::Arc};

    type Provider = MockEthProvider<DogeosPrimitives, DogeosChainSpec>;

    fn oracle_account(
        chain_spec: &DogeosChainSpec,
        overrides: impl IntoIterator<Item = (U256, U256)>,
    ) -> ExtendedAccount {
        let genesis_account = chain_spec
            .genesis()
            .alloc
            .get(&L1_GAS_PRICE_ORACLE_ADDRESS)
            .expect("test chain spec contains the L1 gas-price oracle");
        let mut storage = genesis_account
            .storage
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect::<BTreeMap<_, _>>();
        storage.extend(
            overrides
                .into_iter()
                .map(|(key, value)| (B256::from(key), value)),
        );

        let mut account = ExtendedAccount::new(
            genesis_account.nonce.unwrap_or_default(),
            genesis_account.balance,
        )
        .extend_storage(storage);
        if let Some(code) = genesis_account.code.clone() {
            account = account.with_bytecode(code);
        }
        account
    }

    fn provider(chain_spec: Arc<DogeosChainSpec>) -> Provider {
        let provider = MockEthProvider::<DogeosPrimitives>::new()
            .with_chain_spec(chain_spec.as_ref().clone())
            .with_genesis_block();
        provider.add_account(L1_GAS_PRICE_ORACLE_ADDRESS, oracle_account(&chain_spec, []));
        provider
    }

    fn add_oracle_head(
        provider: &Provider,
        parent_hash: B256,
        number: u64,
        timestamp: u64,
        l1_base_fee: U256,
    ) -> SealedBlock<DogeosBlock> {
        provider.add_account(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            oracle_account(&DOGEOS_CHIKYU, [(L1_BASE_FEE_SLOT, l1_base_fee)]),
        );
        let block: DogeosBlock = alloy_consensus::Block {
            header: alloy_consensus::Header {
                parent_hash,
                number,
                timestamp,
                gas_limit: 30_000_000,
                ..Default::default()
            },
            body: Default::default(),
        };
        let block = block.seal_slow();
        provider.add_block(block.hash(), block.clone().unseal());
        block
    }

    fn validator(
        provider: Provider,
        snapshot: DogeosL1FeeSnapshot,
    ) -> DogeosTransactionValidator<Provider, DogeosPooledTransaction, ScrollEvmConfig> {
        let chain_spec = provider.chain_spec();
        let inner =
            EthTransactionValidatorBuilder::new(provider, ScrollEvmConfig::dogeos(chain_spec))
                .no_eip4844()
                .build(InMemoryBlobStore::default());
        DogeosTransactionValidator::new(inner, snapshot, false)
    }

    fn transaction(sender: Address) -> DogeosPooledTransaction {
        let signed: ScrollTransactionSigned = Signed::new_unchecked(
            TxLegacy {
                chain_id: Some(DOGEOS_CHIKYU.chain().id()),
                gas_price: 1,
                gas_limit: 21_000,
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            },
            Signature::test_signature(),
            B256::repeat_byte(0x44),
        )
        .into();
        let encoded_length = signed.encode_2718_len();
        DogeosPooledTransaction::new(Recovered::new_unchecked(signed, sender), encoded_length)
    }

    fn cache_snapshot(
        validator: &DogeosTransactionValidator<Provider, DogeosPooledTransaction, ScrollEvmConfig>,
    ) -> DogeosL1FeeSnapshot {
        match &*validator.l1_fee_cache.read() {
            DogeosL1FeeCache::Ready(snapshot) => snapshot.as_ref().clone(),
            state => panic!("expected ready cache, got {state:?}"),
        }
    }

    fn assert_formula_fields(snapshot: &DogeosL1FeeSnapshot) {
        assert!(snapshot.l1_block_info.l1_blob_base_fee.is_some());
        assert!(snapshot.l1_block_info.l1_commit_scalar.is_some());
        assert!(snapshot.l1_block_info.l1_blob_scalar.is_some());
        assert!(snapshot.l1_block_info.penalty_threshold.is_some());
        assert!(snapshot.l1_block_info.penalty_factor.is_some());
    }

    #[test]
    fn hydrates_feynman_genesis_from_one_exact_head() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let latest = provider.latest_header().unwrap().unwrap();

        let snapshot = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();

        assert_eq!(snapshot.head_hash(), latest.hash());
        assert_eq!(snapshot.timestamp(), DOGEOS_CHIKYU.genesis().timestamp);
        assert_eq!(snapshot.number(), 0);
        assert!(!DOGEOS_CHIKYU.is_tsuki_active_at_timestamp(snapshot.timestamp()));
        assert_formula_fields(&snapshot);
    }

    #[test]
    fn hydrates_tsuki_genesis_from_one_exact_head() {
        let provider = provider(DOGEOS_DEV.clone());
        let latest = provider.latest_header().unwrap().unwrap();

        let snapshot = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();

        assert_eq!(snapshot.head_hash(), latest.hash());
        assert_eq!(snapshot.timestamp(), DOGEOS_DEV.genesis().timestamp);
        assert_eq!(snapshot.number(), 0);
        assert!(DOGEOS_DEV.is_tsuki_active_at_timestamp(snapshot.timestamp()));
        assert_formula_fields(&snapshot);
    }

    #[test]
    fn missing_latest_header_is_an_explicit_error() {
        let provider =
            NoopProvider::<DogeosChainSpec, DogeosPrimitives>::new(DOGEOS_CHIKYU.clone());

        let error = DogeosL1FeeSnapshot::load_latest(&provider).unwrap_err();

        assert!(matches!(error, DogeosL1FeeError::LatestHeaderNotFound));
    }

    #[test]
    fn missing_oracle_account_is_an_explicit_error() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let latest_hash = provider.latest_header().unwrap().unwrap().hash();
        provider
            .accounts
            .lock()
            .remove(&L1_GAS_PRICE_ORACLE_ADDRESS);

        let error = DogeosL1FeeSnapshot::load_latest(&provider).unwrap_err();

        assert!(matches!(
            error,
            DogeosL1FeeError::OracleAccountNotFound {
                head_hash,
                number: 0
            } if head_hash == latest_hash
        ));
    }

    #[test]
    fn canonical_head_refresh_replaces_complete_snapshot_together() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let initial = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let validator = validator(provider.clone(), initial.clone());
        let next_fee = U256::from(42);
        provider.add_account(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            oracle_account(
                &DOGEOS_CHIKYU,
                [
                    (L1_BASE_FEE_SLOT, next_fee),
                    (L1_BLOB_BASE_FEE_SLOT, U256::from(43)),
                    (L1_COMMIT_SCALAR_SLOT, U256::from(44)),
                    (L1_BLOB_SCALAR_SLOT, U256::from(45)),
                    (PENALTY_THRESHOLD_SLOT, U256::from(46)),
                    (PENALTY_FACTOR_SLOT, U256::from(47)),
                ],
            ),
        );
        let block: DogeosBlock = alloy_consensus::Block {
            header: alloy_consensus::Header {
                parent_hash: initial.head_hash(),
                number: 1,
                timestamp: initial.timestamp() + 1,
                gas_limit: 30_000_000,
                ..Default::default()
            },
            body: Default::default(),
        };
        let block = block.seal_slow();
        provider.add_block(block.hash(), block.clone().unseal());

        validator.on_new_head_block(&block);

        let refreshed = cache_snapshot(&validator);
        assert_eq!(refreshed.head_hash(), block.hash());
        assert_eq!(refreshed.number(), 1);
        assert_eq!(refreshed.timestamp(), initial.timestamp() + 1);
        assert_eq!(refreshed.l1_block_info.l1_base_fee, next_fee);
        assert_eq!(
            refreshed.l1_block_info.l1_blob_base_fee,
            Some(U256::from(43))
        );
        assert_eq!(
            refreshed.l1_block_info.l1_commit_scalar,
            Some(U256::from(44))
        );
        assert_eq!(refreshed.l1_block_info.l1_blob_scalar, Some(U256::from(45)));
        assert_eq!(
            refreshed.l1_block_info.penalty_threshold,
            Some(U256::from(46))
        );
        assert_eq!(refreshed.l1_block_info.penalty_factor, Some(U256::from(47)));
    }

    #[test]
    fn refresh_rechecks_latest_hash_before_publication() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let initial = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let validator = Arc::new(validator(provider.clone(), initial.clone()));

        let h1 = add_oracle_head(
            &provider,
            initial.head_hash(),
            1,
            initial.timestamp() + 1,
            U256::from(101),
        );
        let h1_hash = h1.hash();

        let (a_captured_tx, a_captured_rx) = std::sync::mpsc::channel();
        let (release_a_tx, release_a_rx) = std::sync::mpsc::channel();
        let validator_a = Arc::clone(&validator);
        let refresh_a = std::thread::spawn(move || {
            let mut h1_blocked = false;
            validator_a.refresh_l1_fee_cache_from_latest_with_hooks(
                |target| {
                    if target == h1_hash && !h1_blocked {
                        h1_blocked = true;
                        a_captured_tx.send(target).unwrap();
                        release_a_rx
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    }
                },
                |_| {},
            );
        });
        assert_eq!(
            a_captured_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            h1_hash
        );

        let h2 = add_oracle_head(
            &provider,
            h1_hash,
            2,
            initial.timestamp() + 2,
            U256::from(202),
        );
        let h2_hash = h2.hash();

        let (b_captured_tx, b_captured_rx) = std::sync::mpsc::channel();
        let (release_b_tx, release_b_rx) = std::sync::mpsc::channel();
        let validator_b = Arc::clone(&validator);
        let refresh_b = std::thread::spawn(move || {
            let mut blocked = false;
            validator_b.refresh_l1_fee_cache_from_latest_with_hooks(
                |target| {
                    if !blocked {
                        blocked = true;
                        b_captured_tx.send(target).unwrap();
                        release_b_rx
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    }
                },
                |_| {},
            );
        });

        release_a_tx.send(()).unwrap();
        assert_eq!(
            b_captured_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            h2_hash
        );
        refresh_a.join().unwrap();

        let observed_while_b_waits = cache_snapshot(&validator);
        release_b_tx.send(()).unwrap();
        refresh_b.join().unwrap();

        assert_eq!(observed_while_b_waits.head_hash(), h2_hash);
        assert_eq!(observed_while_b_waits.number(), 2);
        assert_eq!(
            observed_while_b_waits.l1_block_info.l1_base_fee,
            U256::from(202)
        );
        assert_eq!(cache_snapshot(&validator).head_hash(), h2_hash);
    }

    #[test]
    fn refresh_lock_is_held_from_head_read_through_publication() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let initial = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let validator = Arc::new(validator(provider.clone(), initial.clone()));
        let h1 = add_oracle_head(
            &provider,
            initial.head_hash(),
            1,
            initial.timestamp() + 1,
            U256::from(301),
        );
        let h1_hash = h1.hash();

        let (a_verified_tx, a_verified_rx) = std::sync::mpsc::channel();
        let (release_a_tx, release_a_rx) = std::sync::mpsc::channel();
        let validator_a = Arc::clone(&validator);
        let refresh_a = std::thread::spawn(move || {
            validator_a.refresh_l1_fee_cache_from_latest_with_hooks(
                |_| {},
                |target| {
                    if target == h1_hash {
                        a_verified_tx.send(target).unwrap();
                        release_a_rx
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    }
                },
            );
        });
        assert_eq!(
            a_verified_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            h1_hash
        );
        assert_eq!(cache_snapshot(&validator).head_hash(), initial.head_hash());

        let h2 = add_oracle_head(
            &provider,
            h1_hash,
            2,
            initial.timestamp() + 2,
            U256::from(302),
        );
        let h2_hash = h2.hash();
        let (b_started_tx, b_started_rx) = std::sync::mpsc::channel();
        let validator_b = Arc::clone(&validator);
        let refresh_b = std::thread::spawn(move || {
            b_started_tx.send(()).unwrap();
            validator_b.refresh_l1_fee_cache_from_latest();
        });
        b_started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();

        let lock_held_through_publication = validator.l1_fee_refresh_lock.try_lock().is_none();
        if lock_held_through_publication {
            release_a_tx.send(()).unwrap();
            refresh_a.join().unwrap();
            refresh_b.join().unwrap();
        } else {
            // This branch makes the stale-write failure deterministic if refresh serialization is
            // removed: B publishes H2 before A is allowed to publish its already-verified H1.
            refresh_b.join().unwrap();
            release_a_tx.send(()).unwrap();
            refresh_a.join().unwrap();
        }

        let final_snapshot = cache_snapshot(&validator);
        assert_eq!(final_snapshot.head_hash(), h2_hash);
        assert_eq!(final_snapshot.number(), 2);
        assert!(
            lock_held_through_publication,
            "refresh mutex was released before cache publication"
        );
    }

    #[test]
    fn failed_refresh_blocks_admission_and_later_refresh_recovers() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let sender = Address::repeat_byte(0x22);
        provider.add_account(sender, ExtendedAccount::new(0, U256::MAX));
        let initial = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let validator = validator(provider.clone(), initial.clone());
        let block: DogeosBlock = alloy_consensus::Block {
            header: alloy_consensus::Header {
                parent_hash: initial.head_hash(),
                number: 1,
                timestamp: initial.timestamp() + 1,
                gas_limit: 30_000_000,
                ..Default::default()
            },
            body: Default::default(),
        };
        let block = block.seal_slow();
        provider.add_block(block.hash(), block.clone().unseal());
        provider
            .accounts
            .lock()
            .remove(&L1_GAS_PRICE_ORACLE_ADDRESS);

        validator.on_new_head_block(&block);
        let candidate = transaction(sender);
        let expected_hash = *candidate.hash();
        let outcome = validator.validate_one(TransactionOrigin::External, candidate);

        match outcome {
            TransactionValidationOutcome::Error(hash, error) => {
                assert_eq!(hash, expected_hash);
                let message = error.to_string();
                assert!(message.starts_with(&format!(
                    "DogeOS L1 fee state unavailable for canonical head {} (block #1)",
                    block.hash()
                )));
                assert_eq!(message.matches(&block.hash().to_string()).count(), 1);
                assert!(!message.contains("oracle account not found"));
                let Some(DogeosL1FeeError::CacheUnavailable { source }) =
                    error.downcast_ref::<DogeosL1FeeError>()
                else {
                    panic!("expected typed unavailable error, got {error:?}");
                };
                assert!(matches!(
                    source.as_ref(),
                    DogeosL1FeeError::OracleAccountNotFound { head_hash, number: 1 }
                        if *head_hash == block.hash()
                ));
                assert_eq!(
                    std::error::Error::source(error.as_ref()).map(ToString::to_string),
                    Some(source.to_string())
                );
            }
            outcome => panic!("expected unavailable-cache error, got {outcome:?}"),
        }

        provider.add_account(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            oracle_account(&DOGEOS_CHIKYU, []),
        );
        validator.on_new_head_block(&block);

        assert!(
            validator
                .validate_one(TransactionOrigin::External, transaction(sender))
                .is_valid()
        );
        assert_eq!(cache_snapshot(&validator).head_hash(), block.hash());
    }

    #[test]
    fn disabled_validator_bypasses_only_l1_fee_state() {
        let provider = provider(DOGEOS_CHIKYU.clone());
        let sender = Address::repeat_byte(0x33);
        provider.add_account(sender, ExtendedAccount::new(0, U256::MAX));
        let chain_spec = provider.chain_spec();
        let inner =
            EthTransactionValidatorBuilder::new(provider, ScrollEvmConfig::dogeos(chain_spec))
                .no_eip4844()
                .build(InMemoryBlobStore::default());
        let validator = DogeosTransactionValidator::disabled(inner, true);

        assert!(!validator.requires_l1_data_gas_fee());
        assert!(validator.requires_l1_data_fee_buffer());
        assert!(
            validator
                .validate_one(TransactionOrigin::External, transaction(sender))
                .is_valid()
        );
    }

    #[derive(Debug)]
    struct StateOpenFailure {
        chain_spec: Arc<DogeosChainSpec>,
    }

    impl BlockHashReader for StateOpenFailure {
        fn block_hash(&self, _number: u64) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(&self, _start: u64, _end: u64) -> ProviderResult<Vec<B256>> {
            Ok(Vec::new())
        }
    }

    impl BlockNumReader for StateOpenFailure {
        fn chain_info(&self) -> ProviderResult<ChainInfo> {
            Ok(ChainInfo::default())
        }

        fn best_block_number(&self) -> ProviderResult<u64> {
            Ok(0)
        }

        fn last_block_number(&self) -> ProviderResult<u64> {
            Ok(0)
        }

        fn block_number(&self, _hash: B256) -> ProviderResult<Option<u64>> {
            Ok(None)
        }
    }

    impl BlockIdReader for StateOpenFailure {
        fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
            Ok(None)
        }

        fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
            Ok(None)
        }

        fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
            Ok(None)
        }
    }

    impl ChainSpecProvider for StateOpenFailure {
        type ChainSpec = DogeosChainSpec;

        fn chain_spec(&self) -> Arc<Self::ChainSpec> {
            Arc::clone(&self.chain_spec)
        }
    }

    impl StateProviderFactory for StateOpenFailure {
        fn latest(&self) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn state_by_block_number_or_tag(
            &self,
            _number_or_tag: alloy_eips::BlockNumberOrTag,
        ) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn history_by_block_number(&self, _block: u64) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn history_by_block_hash(&self, block: B256) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::StateForHashNotFound(block))
        }

        fn state_by_block_hash(&self, block: B256) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::StateForHashNotFound(block))
        }

        fn pending(&self) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn pending_state_by_hash(
            &self,
            _block_hash: B256,
        ) -> ProviderResult<Option<StateProviderBox>> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
            Err(ProviderError::UnsupportedProvider)
        }
    }

    #[test]
    fn exact_state_open_failure_keeps_head_context_and_source() {
        let provider = StateOpenFailure {
            chain_spec: DOGEOS_CHIKYU.clone(),
        };
        let head_hash = B256::repeat_byte(0xaa);

        let error = DogeosL1FeeSnapshot::load_for_head(&provider, head_hash, 10, 7).unwrap_err();

        assert!(matches!(
            error,
            DogeosL1FeeError::ExactStateOpen {
                head_hash: actual_hash,
                number: 7,
                source: ProviderError::StateForHashNotFound(source_hash),
            } if actual_hash == head_hash && source_hash == head_hash
        ));
    }
}

//! Bounded, state-dependent admission simulation. Payload building remains the final policy gate.

use crate::DogeosPooledTransaction;
use alloy_consensus::{BlockHeader, Header, Transaction};
use dogeos_reth_evm::{
    CodeWitnessHandle, CodeWitnessLimit, DEFAULT_MAX_CODE_WITNESS_BYTES, EvmExt,
    ScrollBaseFeeProvider, ScrollEvmConfig, ScrollNextBlockEnvAttributes,
};
use reth_evm::{ConfigureEvm, Evm};
use reth_revm::{context::result::EVMError, database::StateProviderDatabase, db::CacheDB};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_transaction_pool::PoolTransaction;

/// Local resource limits for transaction admission. The byte budget also applies to payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeWitnessValidationConfig {
    /// Maximum distinct bytecode bytes accessed by one transaction.
    pub max_code_witness_bytes: u64,
    /// Maximum number of interpreter steps in one simulation.
    pub max_steps: u64,
    /// Maximum queued and executing validations, acquired before spawning validation work.
    pub max_inflight: usize,
}

impl Default for CodeWitnessValidationConfig {
    fn default() -> Self {
        Self {
            max_code_witness_bytes: DEFAULT_MAX_CODE_WITNESS_BYTES,
            max_steps: 1_000_000,
            max_inflight: 64,
        }
    }
}

/// Admission failures distinguish proven code overflow from an incomplete simulation.
#[derive(Debug, thiserror::Error)]
pub enum CodeWitnessValidationError {
    /// This transaction alone exceeds the block's bytecode allowance in the simulated state.
    #[error("transaction code witness exceeds budget: {observed} bytes > {limit} bytes")]
    CodeBytes { observed: u64, limit: u64 },
    /// No conclusion about code size is possible after exhausting the execution allowance.
    #[error("code witness simulation exhausted its {limit} instruction step allowance")]
    ExecutionSteps { limit: u64 },
    /// State could not be opened or read. Callers should return a retryable infrastructure error.
    #[error("code witness simulation provider error: {0}")]
    Provider(#[from] ProviderError),
    /// A canonical head is required to select an exact state snapshot.
    #[error("code witness simulation requires a canonical head")]
    MissingHead,
    /// Ordinary EVM validation failure is not evidence of code overflow.
    #[error("code witness simulation is inconclusive: {0}")]
    Indeterminate(String),
}

/// Executes a pool transaction against the latest canonical parent's exact state, without writes.
///
/// Future nonces are allowed during simulation because their predecessors may still be in the pool.
/// The sender nonce is overridden only in a disposable cache so CREATE addresses use the original
/// transaction nonce. A successful simulation is state-dependent and must be checked again while
/// building the block. Reverts and ordinary EVM halts are valid simulation results.
pub fn validate_code_witness<Client>(
    client: &Client,
    evm_config: &ScrollEvmConfig,
    transaction: &DogeosPooledTransaction,
    config: CodeWitnessValidationConfig,
) -> Result<(), CodeWitnessValidationError>
where
    Client: BlockReaderIdExt<Header = Header> + StateProviderFactory,
{
    let parent = client
        .latest_header()?
        .ok_or(CodeWitnessValidationError::MissingHead)?;
    if parent.number() == u64::MAX {
        return Err(CodeWitnessValidationError::Indeterminate(
            "parent block number overflow".to_owned(),
        ));
    }
    let provider = client.state_by_block_hash(parent.hash())?;
    let mut database = CacheDB::new(StateProviderDatabase::new(provider));
    // Skipping the nonce check alone is insufficient: CREATE derives its address from the account
    // nonce. An isolated overlay lets future-nonce transactions execute at their own nonce.
    database.load_account(transaction.sender())?.info.nonce = transaction.nonce();
    let timestamp = parent.timestamp().checked_add(1).ok_or_else(|| {
        CodeWitnessValidationError::Indeterminate("parent timestamp overflow".to_owned())
    })?;
    // The base-fee calculator assumes post-Feynman headers have a base fee. Keep malformed
    // provider data a retryable error rather than panicking in a validation worker.
    if parent.base_fee_per_gas().is_none() {
        return Err(CodeWitnessValidationError::Indeterminate(
            "parent has no base fee".to_owned(),
        ));
    }
    let base_fee = ScrollBaseFeeProvider::new(evm_config.chain_spec().clone())
        .next_block_base_fee(&mut database, &*parent, timestamp)?;
    let attributes = ScrollNextBlockEnvAttributes {
        timestamp,
        suggested_fee_recipient: parent.beneficiary(),
        gas_limit: parent.gas_limit(),
        base_fee,
    };
    let env = evm_config
        .next_evm_env(&parent, &attributes)
        .unwrap_or_else(|error| match error {});
    let meter = CodeWitnessHandle::new(config.max_code_witness_bytes, config.max_steps);
    meter.begin_transaction();
    let mut evm = evm_config.evm_with_env_and_inspector(database, env, meter.inspector());
    evm.with_l1_data_fee_buffer_check(evm_config.chain_spec().config.l1_data_fee_buffer_check);
    // Underpriced transactions may legitimately wait in the pool for a lower base fee. Execute
    // their code without changing their gas-price inputs; this policy is not fee validation.
    evm.with_base_fee_check(false);
    let result = evm.transact(transaction.consensus_ref());
    match meter.exceeded() {
        Some(CodeWitnessLimit::CodeBytes) => {
            return Err(CodeWitnessValidationError::CodeBytes {
                observed: meter.transaction_code_bytes(),
                limit: config.max_code_witness_bytes,
            });
        }
        Some(CodeWitnessLimit::ExecutionSteps) => {
            return Err(CodeWitnessValidationError::ExecutionSteps {
                limit: config.max_steps,
            });
        }
        None => {}
    }
    result.map(|_| ()).map_err(|error| match error {
        EVMError::Database(error) => CodeWitnessValidationError::Provider(error),
        error => CodeWitnessValidationError::Indeterminate(error.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxLegacy};
    use alloy_eips::Encodable2718;
    use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};
    use dogeos_chainspec::{DOGEOS_DEV, DogeosChainSpec};
    use dogeos_reth_primitives::{DogeosPrimitives, ScrollTransactionSigned};
    use reth_chainspec::EthChainSpec;
    use reth_primitives_traits::Recovered;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider, NoopProvider};
    use reth_storage_api::AccountReader;

    type Provider = MockEthProvider<DogeosPrimitives, DogeosChainSpec>;
    const SENDER: Address = Address::repeat_byte(0xa1);
    const ENTRY: Address = Address::repeat_byte(0xa2);
    const TARGET: Address = Address::repeat_byte(0xa3);

    fn provider(entry_code: Vec<u8>) -> Provider {
        let provider = MockEthProvider::<DogeosPrimitives>::new()
            .with_chain_spec(DOGEOS_DEV.as_ref().clone())
            .with_genesis_block();
        for (address, account) in &DOGEOS_DEV.genesis().alloc {
            let mut stored =
                ExtendedAccount::new(account.nonce.unwrap_or_default(), account.balance)
                    .extend_storage(
                        account
                            .storage
                            .clone()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(key, value)| (key, U256::from_be_bytes(value.0))),
                    );
            if let Some(code) = account.code.clone() {
                stored = stored.with_bytecode(code);
            }
            provider.add_account(*address, stored);
        }
        provider.add_account(SENDER, ExtendedAccount::new(0, U256::MAX));
        provider.add_account(
            ENTRY,
            ExtendedAccount::new(1, U256::ZERO).with_bytecode(entry_code.into()),
        );
        provider.add_account(
            TARGET,
            ExtendedAccount::new(1, U256::ZERO).with_bytecode(Bytes::from(vec![0; 1024])),
        );
        provider
    }

    fn transaction(nonce: u64) -> DogeosPooledTransaction {
        transaction_with(nonce, TxKind::Call(ENTRY), Bytes::new())
    }

    fn transaction_with(nonce: u64, to: TxKind, input: Bytes) -> DogeosPooledTransaction {
        transaction_with_price(nonce, to, input, 10_000_000_000)
    }

    fn transaction_with_price(
        nonce: u64,
        to: TxKind,
        input: Bytes,
        gas_price: u128,
    ) -> DogeosPooledTransaction {
        let signed: ScrollTransactionSigned = Signed::new_unchecked(
            TxLegacy {
                chain_id: Some(DOGEOS_DEV.chain().id()),
                nonce,
                gas_price,
                gas_limit: 1_000_000,
                to,
                input,
                ..Default::default()
            },
            Signature::test_signature(),
            B256::repeat_byte(0x44),
        )
        .into();
        let encoded_length = signed.encode_2718_len();
        DogeosPooledTransaction::new(Recovered::new_unchecked(signed, SENDER), encoded_length)
    }

    fn extcodesize_revert() -> Vec<u8> {
        let mut code = vec![];
        for _ in 0..2 {
            code.push(0x73); // PUSH20 target
            code.extend(TARGET.as_slice());
            code.extend([0x3b, 0x50]); // EXTCODESIZE, POP
        }
        code.extend([0x5f, 0x5f, 0xfd]); // PUSH0, PUSH0, REVERT
        code
    }

    #[test]
    fn admission_rejects_oversize_but_accepts_exact_deduplicated_reverting_witness() {
        let code = extcodesize_revert();
        let bytes = code.len() as u64 + 1024;
        let provider = provider(code);
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        let config = CodeWitnessValidationConfig {
            max_code_witness_bytes: bytes - 1,
            ..Default::default()
        };
        assert!(
            matches!(validate_code_witness(&provider, &evm, &transaction(0), config),
            Err(CodeWitnessValidationError::CodeBytes { observed, .. }) if observed == bytes)
        );
        assert!(
            validate_code_witness(
                &provider,
                &evm,
                &transaction(0),
                CodeWitnessValidationConfig {
                    max_code_witness_bytes: bytes,
                    ..config
                }
            )
            .is_ok()
        );
        assert_eq!(
            provider
                .latest()
                .unwrap()
                .basic_account(&SENDER)
                .unwrap()
                .unwrap()
                .nonce,
            0
        );
    }

    #[test]
    fn future_nonce_is_simulated_and_still_cannot_bypass_code_budget() {
        let provider = provider(extcodesize_revert());
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        let config = CodeWitnessValidationConfig {
            max_code_witness_bytes: 1024,
            ..Default::default()
        };
        assert!(matches!(
            validate_code_witness(&provider, &evm, &transaction(12), config),
            Err(CodeWitnessValidationError::CodeBytes { .. })
        ));
    }

    #[test]
    fn future_nonce_create_uses_the_transactions_creation_address() {
        let provider = provider(vec![0]);
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        let nonce = 12;
        // Execute the expensive access only at CREATE(sender, nonce). A simulation that merely
        // disables the nonce check would run at CREATE(sender, 0) and miss the overflow.
        let mut init = vec![0x30, 0x73]; // ADDRESS, PUSH20
        init.extend(SENDER.create(nonce).as_slice());
        init.extend([0x14, 0x60, 27, 0x57, 0x00, 0x5b]); // EQ, PUSH1 27, JUMPI, STOP, JUMPDEST
        init.push(0x73);
        init.extend(TARGET.as_slice());
        init.extend([0x3b, 0x50, 0x5f, 0x5f, 0xf3]);
        let tx = transaction_with(nonce, TxKind::Create, init.into());
        let config = CodeWitnessValidationConfig {
            max_code_witness_bytes: 1023,
            ..Default::default()
        };
        assert!(matches!(
            validate_code_witness(&provider, &evm, &tx, config),
            Err(CodeWitnessValidationError::CodeBytes { .. })
        ));
        assert_eq!(
            provider
                .latest()
                .unwrap()
                .basic_account(&SENDER)
                .unwrap()
                .unwrap()
                .nonce,
            0
        );
    }

    #[test]
    fn missing_canonical_head_is_retryable() {
        let provider = NoopProvider::<DogeosChainSpec, DogeosPrimitives>::new(DOGEOS_DEV.clone());
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        assert!(matches!(
            validate_code_witness(&provider, &evm, &transaction(0), Default::default()),
            Err(CodeWitnessValidationError::MissingHead)
        ));
    }

    #[test]
    fn header_provider_failure_is_not_code_overflow() {
        let provider =
            MockEthProvider::<DogeosPrimitives>::new().with_chain_spec(DOGEOS_DEV.as_ref().clone());
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        assert!(matches!(
            validate_code_witness(&provider, &evm, &transaction(0), Default::default()),
            Err(CodeWitnessValidationError::Provider(_))
        ));
    }

    #[test]
    fn parked_low_fee_transaction_still_executes_the_witness_check() {
        let provider = provider(extcodesize_revert());
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        let tx = transaction_with_price(0, TxKind::Call(ENTRY), Bytes::new(), 1);
        assert!(validate_code_witness(&provider, &evm, &tx, Default::default()).is_ok());
        let config = CodeWitnessValidationConfig {
            max_code_witness_bytes: 1024,
            ..Default::default()
        };
        assert!(matches!(
            validate_code_witness(&provider, &evm, &tx, config),
            Err(CodeWitnessValidationError::CodeBytes { .. })
        ));
    }

    #[test]
    fn execution_allowance_exhaustion_is_not_code_overflow() {
        let provider = provider(vec![0x5b, 0x5f, 0x56]); // JUMPDEST, PUSH0, JUMP
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        let config = CodeWitnessValidationConfig {
            max_steps: 20,
            ..Default::default()
        };
        assert!(matches!(
            validate_code_witness(&provider, &evm, &transaction(0), config),
            Err(CodeWitnessValidationError::ExecutionSteps { limit: 20 })
        ));
    }

    #[test]
    fn ordinary_validation_failure_is_inconclusive() {
        let provider = provider(vec![0]);
        provider.add_account(SENDER, ExtendedAccount::new(0, U256::ZERO));
        let evm = ScrollEvmConfig::dogeos(DOGEOS_DEV.clone());
        assert!(matches!(
            validate_code_witness(&provider, &evm, &transaction(0), Default::default()),
            Err(CodeWitnessValidationError::Indeterminate(_))
        ));
    }
}

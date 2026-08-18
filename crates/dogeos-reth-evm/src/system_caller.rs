use alloc::{boxed::Box, string::ToString};

use alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;
use alloy_evm::{
    Evm,
    block::{
        BlockExecutionError, BlockValidationError, OnStateHook, StateChangePreBlockSource,
        StateChangeSource,
    },
};
use alloy_primitives::B256;
use dogeos_hardforks::DogeosHardforks;
use revm::{
    DatabaseCommit,
    context::{Block, result::ResultAndState},
    state::EvmState,
};

/// An ephemeral helper type for executing system calls.
#[derive(derive_more::Debug)]
pub(crate) struct ScrollSystemCaller<Spec> {
    spec: Spec,
    /// Optional hook invoked with each state change the executor commits.
    #[debug("installed={}", hook.is_some())]
    hook: Option<Box<dyn OnStateHook>>,
}

impl<Spec> ScrollSystemCaller<Spec> {
    /// Creates a system caller for a chain's hardfork schedule.
    pub(crate) const fn new(spec: Spec) -> Self {
        Self { spec, hook: None }
    }

    /// Replaces the hook that receives state changes the executor commits.
    ///
    /// Dropping the previous hook sends `FinishedStateUpdates` to reth's state-root task. Callers
    /// must therefore clear it before awaiting `state_root()`.
    pub(crate) fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.hook = hook;
    }

    /// Notifies the installed hook about a state change the executor commits.
    pub(crate) fn on_state(&mut self, source: StateChangeSource, state: &EvmState) {
        if let Some(hook) = &mut self.hook {
            hook.on_state(source, state);
        }
    }

    /// Notifies the hook about a rollup-native pre-block transition.
    pub(crate) fn on_pre_block_state(&mut self, state: &EvmState) {
        // Empty state is the sentinel returned by skipped or already-applied transitions.
        if state.is_empty() {
            return;
        }

        // Alloy has no custom rollup transition variant. Root consumers use the state payload;
        // the source is only a phase label, so use the existing pre-block category. A transition
        // block may emit this source multiple times with non-EIP-2935 accounts such as the oracle
        // or NativeDogeToken, so consumers must not filter or attribute updates by this source.
        self.on_state(
            StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract),
            state,
        );
    }
}

impl<Spec> ScrollSystemCaller<Spec>
where
    Spec: DogeosHardforks,
{
    /// Applies the pre-block call to the EIP-2935 blockhashes contract.
    pub(crate) fn apply_blockhashes_contract_call(
        &mut self,
        parent_block_hash: B256,
        evm: &mut impl Evm<DB: DatabaseCommit>,
    ) -> Result<(), BlockExecutionError> {
        if let Some(result) =
            transact_blockhashes_contract_call(&self.spec, parent_block_hash, evm)?
        {
            self.on_state(
                StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract),
                &result.state,
            );
            evm.db_mut().commit(result.state);
        }
        Ok(())
    }
}

/// Runs the EIP-2935 pre-block call when Feynman is active and the block is not genesis.
#[inline]
fn transact_blockhashes_contract_call<Halt>(
    spec: impl DogeosHardforks,
    parent_block_hash: B256,
    evm: &mut impl Evm<HaltReason = Halt>,
) -> Result<Option<ResultAndState<Halt>>, BlockExecutionError> {
    if !spec.is_feynman_active_at_timestamp(evm.block().timestamp().to()) {
        return Ok(None);
    }
    if evm.block().number().to::<u64>() == 0 {
        return Ok(None);
    }

    evm.transact_system_call(
        alloy_eips::eip4788::SYSTEM_ADDRESS,
        HISTORY_STORAGE_ADDRESS,
        parent_block_hash.0.into(),
    )
    .map(Some)
    .map_err(|error| {
        BlockValidationError::BlockHashContractCall {
            message: error.to_string(),
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollDefaultPrecompilesFactory, ScrollEvmFactory};
    use alloy_eips::eip2935::HISTORY_STORAGE_CODE;
    use alloy_evm::{EvmEnv, EvmFactory};
    use alloy_primitives::{Address, U256, keccak256};
    use dogeos_hardforks::{DogeosChainHardforks, DogeosHardfork, ForkCondition};
    use revm::{
        Database,
        bytecode::Bytecode,
        context::{BlockEnv, CfgEnv},
        database::{EmptyDB, State},
        state::{Account, AccountInfo},
    };
    use revm_scroll::ScrollSpecId;
    use std::sync::{Arc, Mutex};

    fn state_with_history_contract() -> State<EmptyDB> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo {
                code_hash: keccak256(HISTORY_STORAGE_CODE.as_ref()),
                code: Some(Bytecode::new_raw(HISTORY_STORAGE_CODE.clone())),
                ..Default::default()
            },
        );
        state
    }

    fn block_env(number: u64, timestamp: u64) -> BlockEnv {
        BlockEnv {
            number: U256::from(number),
            timestamp: U256::from(timestamp),
            ..Default::default()
        }
    }

    #[test]
    fn skips_blockhash_call_before_feynman() {
        let mut caller = ScrollSystemCaller::new(DogeosChainHardforks::new([(
            DogeosHardfork::Feynman,
            ForkCondition::Timestamp(100),
        )]));
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state_with_history_contract(),
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::FEYNMAN),
                block_env(1, 99),
            ),
        );
        caller.set_state_hook(Some(Box::new(|_, _: &EvmState| {
            panic!("blockhash hook fired before Feynman")
        })));

        caller
            .apply_blockhashes_contract_call(B256::repeat_byte(0x11), &mut evm)
            .unwrap();

        assert_eq!(
            evm.db_mut()
                .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
                .unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn stores_parent_hash_after_feynman() {
        let mut caller = ScrollSystemCaller::new(DogeosChainHardforks::new([(
            DogeosHardfork::Feynman,
            ForkCondition::Timestamp(0),
        )]));
        let parent_hash = B256::repeat_byte(0x22);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let hook_updates = Arc::clone(&updates);
        caller.set_state_hook(Some(Box::new(move |source, state: &EvmState| {
            let mut keys = state.keys().copied().collect::<Vec<_>>();
            keys.sort_unstable();
            hook_updates.lock().unwrap().push((source, keys));
        })));
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state_with_history_contract(),
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::FEYNMAN),
                block_env(1, 1),
            ),
        );

        caller
            .apply_blockhashes_contract_call(parent_hash, &mut evm)
            .unwrap();

        assert_eq!(
            B256::from(
                evm.db_mut()
                    .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
                    .unwrap()
            ),
            parent_hash
        );
        let updates = updates.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            updates[0].0,
            StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract)
        ));
        assert_eq!(updates[0].1, [HISTORY_STORAGE_ADDRESS]);
    }

    #[test]
    fn skips_blockhash_call_at_genesis() {
        let mut caller = ScrollSystemCaller::new(DogeosChainHardforks::mainnet());
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state_with_history_contract(),
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::FEYNMAN),
                block_env(0, 1),
            ),
        );
        caller.set_state_hook(Some(Box::new(|_, _: &EvmState| {
            panic!("blockhash hook fired at genesis")
        })));

        caller
            .apply_blockhashes_contract_call(B256::repeat_byte(0x33), &mut evm)
            .unwrap();

        assert_eq!(
            evm.db_mut()
                .storage(HISTORY_STORAGE_ADDRESS, U256::ZERO)
                .unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn pre_block_state_skips_empty_transitions() {
        let mut caller = ScrollSystemCaller::new(DogeosChainHardforks::mainnet());
        let updates = Arc::new(Mutex::new(Vec::new()));
        let hook_updates = Arc::clone(&updates);
        caller.set_state_hook(Some(Box::new(move |source, state: &EvmState| {
            hook_updates
                .lock()
                .unwrap()
                .push((source, state.keys().copied().collect::<Vec<_>>()));
        })));

        caller.on_pre_block_state(&EvmState::default());

        let address = Address::repeat_byte(0x44);
        let mut account = Account::from(AccountInfo::default());
        account.mark_touch();
        caller.on_pre_block_state(&[(address, account)].into_iter().collect());

        let updates = updates.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            updates[0].0,
            StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract)
        ));
        assert_eq!(updates[0].1, [address]);
    }
}

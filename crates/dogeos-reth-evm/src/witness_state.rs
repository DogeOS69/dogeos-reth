use alloy_primitives::{Address, U256, address};
use revm::{
    Database,
    database::{State, bal::EvmDatabaseError},
};

/// Canonical L2 message queue contract whose state must be included in execution witnesses.
pub const L2_MESSAGE_QUEUE_ADDRESS: Address =
    address!("0x5300000000000000000000000000000000000000");
/// Storage slot containing the withdrawal trie root.
pub const WITHDRAW_TRIE_ROOT_SLOT: U256 = U256::ZERO;
/// Storage slot containing the next L1-message queue index.
pub const NEXT_MESSAGE_INDEX_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Loads protocol state that is read outside normal transaction execution but belongs in a
/// complete Scroll execution witness.
pub trait LoadMessageQueueWitnessState<DB: Database> {
    fn load_withdraw_root(&mut self) -> Result<(), EvmDatabaseError<DB::Error>>;
    fn load_next_message_index(&mut self) -> Result<(), EvmDatabaseError<DB::Error>>;

    fn load_message_queue_witness_state(
        &mut self,
        include_next_message_index: bool,
    ) -> Result<(), EvmDatabaseError<DB::Error>> {
        self.load_withdraw_root()?;
        if include_next_message_index {
            self.load_next_message_index()?;
        }
        Ok(())
    }
}

impl<DB: Database> LoadMessageQueueWitnessState<DB> for State<DB> {
    fn load_withdraw_root(&mut self) -> Result<(), EvmDatabaseError<DB::Error>> {
        self.load_cache_account(L2_MESSAGE_QUEUE_ADDRESS)
            .map_err(EvmDatabaseError::Database)?;
        Database::storage(self, L2_MESSAGE_QUEUE_ADDRESS, WITHDRAW_TRIE_ROOT_SLOT)?;
        Ok(())
    }

    fn load_next_message_index(&mut self) -> Result<(), EvmDatabaseError<DB::Error>> {
        self.load_cache_account(L2_MESSAGE_QUEUE_ADDRESS)
            .map_err(EvmDatabaseError::Database)?;
        Database::storage(self, L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use revm::{bytecode::Bytecode, state::AccountInfo};
    use std::{collections::HashMap, convert::Infallible};

    #[derive(Default)]
    struct InMemoryDb {
        accounts: HashMap<Address, AccountInfo>,
        storage: HashMap<(Address, U256), U256>,
    }

    impl Database for InMemoryDb {
        type Error = Infallible;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            Ok(self.accounts.get(&address).cloned())
        }

        fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
            Ok(self
                .storage
                .get(&(address, index))
                .copied()
                .unwrap_or_default())
        }

        fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
            Ok(B256::ZERO)
        }
    }

    #[test]
    fn loads_both_message_queue_slots_for_post_tsuki_witnesses() {
        let mut db = InMemoryDb::default();
        db.accounts
            .insert(L2_MESSAGE_QUEUE_ADDRESS, AccountInfo::default());
        db.storage.insert(
            (L2_MESSAGE_QUEUE_ADDRESS, WITHDRAW_TRIE_ROOT_SLOT),
            U256::from(7),
        );
        db.storage.insert(
            (L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT),
            U256::from(8),
        );
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .build();

        state.load_message_queue_witness_state(true).unwrap();

        let account = state.cache.accounts.get(&L2_MESSAGE_QUEUE_ADDRESS).unwrap();
        assert_eq!(
            account.storage_slot(WITHDRAW_TRIE_ROOT_SLOT),
            Some(U256::from(7))
        );
        assert_eq!(
            account.storage_slot(NEXT_MESSAGE_INDEX_SLOT),
            Some(U256::from(8))
        );
    }

    #[test]
    fn pre_tsuki_witness_does_not_load_next_message_index() {
        let mut db = InMemoryDb::default();
        db.accounts
            .insert(L2_MESSAGE_QUEUE_ADDRESS, AccountInfo::default());
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .build();

        state.load_message_queue_witness_state(false).unwrap();

        let account = state.cache.accounts.get(&L2_MESSAGE_QUEUE_ADDRESS).unwrap();
        assert!(account.storage_slot(WITHDRAW_TRIE_ROOT_SLOT).is_some());
        assert!(account.storage_slot(NEXT_MESSAGE_INDEX_SLOT).is_none());
    }
}

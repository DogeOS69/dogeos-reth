//! State transition for the Tsuki utilization-controlled base fee.

use crate::NEXT_CONTROLLED_BASE_FEE_SLOT;
use alloy_primitives::{Address, U256};
use revm::{Database, DatabaseCommit, state::EvmState};

/// Persists the controlled component that the next block must use.
///
/// REVM clears touched EIP-161-empty accounts even when they contain a changed storage slot, so a
/// previously empty system-config account receives nonce one. Existing account metadata is
/// otherwise preserved verbatim.
pub fn store_next_controlled_base_fee<DB>(
    db: &mut DB,
    system_config: Address,
    next_controlled_fee: u64,
) -> Result<(), DB::Error>
where
    DB: Database + DatabaseCommit,
{
    store_next_controlled_base_fee_with_state(db, system_config, next_controlled_fee).map(drop)
}

/// Persists the controlled fee and returns the committed update for state-root hooks.
pub(crate) fn store_next_controlled_base_fee_with_state<DB>(
    db: &mut DB,
    system_config: Address,
    next_controlled_fee: u64,
) -> Result<EvmState, DB::Error>
where
    DB: Database + DatabaseCommit,
{
    let old_info = db.basic(system_config)?.unwrap_or_default();
    let mut new_info = old_info.clone();
    if new_info.is_empty() {
        new_info.nonce = 1;
    }

    super::commit_account_update(
        db,
        system_config,
        old_info,
        new_info,
        [(
            NEXT_CONTROLLED_BASE_FEE_SLOT,
            U256::from(next_controlled_fee),
        )],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{B256, Bytes};
    use revm::{
        Database,
        bytecode::Bytecode,
        database::{
            EmptyDB, State,
            states::{StorageSlot, bundle_state::BundleRetention, plain_account::PlainStorage},
        },
        state::AccountInfo,
    };

    #[test]
    fn initializes_an_empty_system_config_account() -> eyre::Result<()> {
        let address = Address::repeat_byte(0x11);
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();

        store_next_controlled_base_fee(&mut state, address, 500_000_000_000)?;

        assert_eq!(state.basic(address)?.unwrap().nonce, 1);
        assert_eq!(
            state.storage(address, NEXT_CONTROLLED_BASE_FEE_SLOT)?,
            U256::from(500_000_000_000u64)
        );
        Ok(())
    }

    #[test]
    fn preserves_existing_account_metadata() -> eyre::Result<()> {
        let address = Address::repeat_byte(0x22);
        let code = Bytecode::new_raw(Bytes::from_static(&[0x00]));
        let info = AccountInfo {
            balance: U256::from(42),
            nonce: 7,
            code_hash: B256::repeat_byte(0x33),
            code: Some(code),
            ..Default::default()
        };
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let unrelated_slot = U256::from(77);
        state.insert_account_with_storage(
            address,
            info.clone(),
            PlainStorage::from_iter([(unrelated_slot, U256::from(88))]),
        );

        store_next_controlled_base_fee(&mut state, address, 600_000_000_000)?;

        assert_eq!(state.basic(address)?.unwrap(), info);
        assert_eq!(
            state.storage(address, NEXT_CONTROLLED_BASE_FEE_SLOT)?,
            U256::from(600_000_000_000u64)
        );
        assert_eq!(state.storage(address, unrelated_slot)?, U256::from(88));
        Ok(())
    }

    #[test]
    fn controller_write_is_retained_in_bundle_and_revert_tracking() -> eyre::Result<()> {
        let address = Address::repeat_byte(0x33);
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();

        store_next_controlled_base_fee(&mut state, address, 700_000_000_000)?;
        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();
        let account = bundle.state.get(&address).unwrap();

        assert_eq!(account.info.as_ref().unwrap().nonce, 1);
        assert_eq!(
            account.storage.get(&NEXT_CONTROLLED_BASE_FEE_SLOT),
            Some(&StorageSlot {
                present_value: U256::from(700_000_000_000u64),
                ..Default::default()
            })
        );
        assert!(!bundle.reverts.is_empty());
        Ok(())
    }
}

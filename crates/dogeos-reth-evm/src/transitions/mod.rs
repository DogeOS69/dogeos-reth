//! Idempotent Feynman+ protocol state transitions.

mod feynman;
mod galileo_v2;
mod tsuki;

pub(crate) use feynman::apply_feynman_hard_fork;
pub(crate) use galileo_v2::apply_galileo_v2_hard_fork;
pub(crate) use tsuki::apply_tsuki_hard_fork;

use revm::{
    Database, DatabaseCommit,
    primitives::{Address, U256},
    state::{Account, AccountInfo, EvmState, EvmStorageSlot},
};

/// Commits a protocol-owned account update through REVM's public database boundary.
///
/// Reading every original slot before committing is important for witness generation and for
/// preserving the revert information accumulated by [`revm::database::State`].
fn commit_account_update<DB>(
    db: &mut DB,
    address: Address,
    original_info: AccountInfo,
    new_info: AccountInfo,
    storage: impl IntoIterator<Item = (U256, U256)>,
) -> Result<EvmState, DB::Error>
where
    DB: Database + DatabaseCommit,
{
    let mut account = Account::from(original_info);
    account.info = new_info;
    account.mark_touch();

    for (slot, present_value) in storage {
        let original_value = db.storage(address, slot)?;
        account.storage.insert(
            slot,
            EvmStorageSlot::new_changed(original_value, present_value, 0),
        );
    }

    let state = [(address, account)].into_iter().collect::<EvmState>();
    db.commit(state.clone());
    Ok(state)
}

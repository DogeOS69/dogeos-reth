//! Withdrawal queue code replacement for a network upgraded from a drained checkpoint.
//!
//! At the configured block, the L2MessageQueue code is replaced by the hash-chain queue. Its
//! storage is kept, so the drained queue's withdrawal root seeds the new hash chain.

use alloc::{format, string::String};
use alloy_primitives::{B256, b256, bytes};
use dogeos_chainspec::WithdrawalQueueUpgrade;
use revm::{
    Database, DatabaseCommit,
    bytecode::Bytecode,
    primitives::{Bytes, U256},
    state::{AccountInfo, EvmState},
};

use crate::{L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT, WITHDRAW_TRIE_ROOT_SLOT};

// Hash-chain L2MessageQueue runtime from DogeOS69/scroll-contracts@419c693c, solc 0.8.24. The same
// code is predeployed in the mainnet genesis.
const HASH_CHAIN_QUEUE_BYTECODE: Bytes = bytes!(
    "608060405234801561000f575f80fd5b5060043610610085575f3560e01c80638da5cb5b116100585780638da5cb5b146100ed578063c4d66de814610100578063d4b9f4fa14610113578063f2fde38b1461011b575f80fd5b806326aad7b7146100895780633cb747bf146100a5578063600a2e77146100d0578063715018a6146100e3575b5f80fd5b61009260015481565b6040519081526020015b60405180910390f35b6053546100b8906001600160a01b031681565b6040516001600160a01b03909116815260200161009c565b6100926100de366004610412565b61012e565b6100eb6101d1565b005b6052546100b8906001600160a01b031681565b6100eb61010e366004610429565b610206565b6100925f5481565b6100eb610129366004610429565b610296565b6053545f906001600160a01b031633146101805760405162461bcd60e51b815260206004820152600e60248201526d37b7363c9036b2b9b9b2b733b2b960911b60448201526064015b60405180910390fd5b5f8061018b84610322565b60408051838152602081018890529294509092507ffaa617c2d8ce12c62637dbce76efcc18dae60574aa95709bdcedce7e76071693910160405180910390a19392505050565b6052546001600160a01b031633146101fb5760405162461bcd60e51b815260040161017790610456565b6102045f6103c1565b565b6052546001600160a01b031633146102305760405162461bcd60e51b815260040161017790610456565b600154156102745760405162461bcd60e51b815260206004820152601160248201527063616e6e6f7420696e697469616c697a6560781b6044820152606401610177565b605380546001600160a01b0319166001600160a01b0392909216919091179055565b6052546001600160a01b031633146102c05760405162461bcd60e51b815260040161017790610456565b6001600160a01b0381166103165760405162461bcd60e51b815260206004820152601d60248201527f6e6577206f776e657220697320746865207a65726f20616464726573730000006044820152606401610177565b61031f816103c1565b50565b6001545f90819067ffffffffffffffff81106103795760405162461bcd60e51b81526020600482015260166024820152756d65737361676520696e646578206f766572666c6f7760501b6044820152606401610177565b5f80546040805160208082019390935280820188905281518082038301815260609091019091528051910120908190556103b482600161048d565b6001559094909350915050565b605280546001600160a01b038381166001600160a01b0319831681179093556040519116919082907f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0905f90a35050565b5f60208284031215610422575f80fd5b5035919050565b5f60208284031215610439575f80fd5b81356001600160a01b038116811461044f575f80fd5b9392505050565b60208082526017908201527f63616c6c6572206973206e6f7420746865206f776e6572000000000000000000604082015260600190565b808201808211156104ac57634e487b7160e01b5f52601160045260245ffd5b9291505056fea164736f6c6343000818000a"
);
const HASH_CHAIN_QUEUE_BYTECODE_HASH: B256 =
    b256!("baf761a1daac2df442f2a373c18f86cbb257d60ee9e62933e5ffa1c9088ab5af");

/// Replaces the L2MessageQueue code at `upgrade.block`, after checking the account against the
/// drained checkpoint.
///
/// Earlier blocks are untouched, and later blocks are a no-op once the code is replaced. Any
/// mismatch with the checkpoint fails the block.
///
/// Returns the committed state update so the executor can forward it to state hooks.
pub(crate) fn apply_withdrawal_queue_upgrade<DB>(
    db: &mut DB,
    block: u64,
    tsuki_v2_active: bool,
    upgrade: WithdrawalQueueUpgrade,
) -> Result<EvmState, String>
where
    DB: Database + DatabaseCommit,
{
    if block < upgrade.block {
        return Ok(EvmState::default());
    }
    let old_info = db
        .basic(L2_MESSAGE_QUEUE_ADDRESS)
        .map_err(|err| format!("failed to read the withdrawal queue: {err:?}"))?
        .ok_or("the withdrawal queue account is missing")?;
    if old_info.code_hash == HASH_CHAIN_QUEUE_BYTECODE_HASH {
        return Ok(EvmState::default());
    }
    if block != upgrade.block {
        return Err(format!(
            "the withdrawal queue code was not replaced at block {}",
            upgrade.block
        ));
    }
    if !tsuki_v2_active {
        return Err("the withdrawal queue replacement requires TsukiV2".into());
    }
    if old_info.code_hash != upgrade.legacy_code_hash {
        return Err(format!(
            "the withdrawal queue code hash is {}, expected {}",
            old_info.code_hash, upgrade.legacy_code_hash
        ));
    }
    let mut read_slot = |slot| {
        db.storage(L2_MESSAGE_QUEUE_ADDRESS, slot)
            .map_err(|err| format!("failed to read withdrawal queue slot {slot}: {err:?}"))
    };
    let root = read_slot(WITHDRAW_TRIE_ROOT_SLOT)?;
    let next_index = read_slot(NEXT_MESSAGE_INDEX_SLOT)?;
    if root != U256::from_be_bytes(upgrade.seed_root.0)
        || next_index != U256::from(upgrade.next_message_index)
    {
        return Err(format!(
            "the withdrawal queue holds root {} and next index {next_index}, expected the \
             checkpoint's {} and {}",
            B256::from(root),
            upgrade.seed_root,
            upgrade.next_message_index
        ));
    }

    let bytecode = Bytecode::new_raw(HASH_CHAIN_QUEUE_BYTECODE);
    debug_assert_eq!(bytecode.hash_slow(), HASH_CHAIN_QUEUE_BYTECODE_HASH);
    let new_info = AccountInfo {
        code_hash: HASH_CHAIN_QUEUE_BYTECODE_HASH,
        code: Some(bytecode),
        ..old_info.clone()
    };
    super::commit_account_update(
        db,
        L2_MESSAGE_QUEUE_ADDRESS,
        old_info,
        new_info,
        core::iter::empty(),
    )
    .map_err(|err| format!("failed to replace the withdrawal queue code: {err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::DOGEOS_MAINNET;
    use reth_chainspec::EthChainSpec;
    use revm::database::{CacheDB, EmptyDB};

    const BLOCK: u64 = 2374;
    const ROOT: B256 = B256::repeat_byte(0x51);
    const NEXT_INDEX: u64 = 1173;
    const OTHER_SLOT: U256 = U256::from_limbs([82, 0, 0, 0]);

    fn legacy_code() -> Bytecode {
        Bytecode::new_raw(bytes!("6080604052"))
    }

    fn upgrade() -> WithdrawalQueueUpgrade {
        WithdrawalQueueUpgrade {
            block: BLOCK,
            legacy_code_hash: legacy_code().hash_slow(),
            seed_root: ROOT,
            next_message_index: NEXT_INDEX,
        }
    }

    fn drained_queue(code: Bytecode, root: B256, next_index: u64) -> CacheDB<EmptyDB> {
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            L2_MESSAGE_QUEUE_ADDRESS,
            AccountInfo {
                balance: U256::from(7),
                nonce: 3,
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        for (slot, value) in [
            (WITHDRAW_TRIE_ROOT_SLOT, U256::from_be_bytes(root.0)),
            (NEXT_MESSAGE_INDEX_SLOT, U256::from(next_index)),
            (OTHER_SLOT, U256::from(9)),
        ] {
            db.insert_account_storage(L2_MESSAGE_QUEUE_ADDRESS, slot, value)
                .unwrap();
        }
        db
    }

    #[test]
    fn replaces_only_the_code_at_the_checkpoint_block() {
        let mut db = drained_queue(legacy_code(), ROOT, NEXT_INDEX);
        let state = apply_withdrawal_queue_upgrade(&mut db, BLOCK, true, upgrade()).unwrap();
        assert!(state[&L2_MESSAGE_QUEUE_ADDRESS].storage.is_empty());

        let info = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
        assert_eq!(info.code_hash, HASH_CHAIN_QUEUE_BYTECODE_HASH);
        assert_eq!((info.balance, info.nonce), (U256::from(7), 3));
        assert_eq!(
            db.storage(L2_MESSAGE_QUEUE_ADDRESS, WITHDRAW_TRIE_ROOT_SLOT)
                .unwrap(),
            U256::from_be_bytes(ROOT.0)
        );
        assert_eq!(
            db.storage(L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT)
                .unwrap(),
            U256::from(NEXT_INDEX)
        );
        assert_eq!(
            db.storage(L2_MESSAGE_QUEUE_ADDRESS, OTHER_SLOT).unwrap(),
            U256::from(9)
        );

        // Later blocks leave the replaced queue alone.
        let state = apply_withdrawal_queue_upgrade(&mut db, BLOCK + 1, true, upgrade()).unwrap();
        assert!(state.is_empty());
    }

    #[test]
    fn leaves_blocks_before_the_checkpoint_alone() {
        let mut db = drained_queue(legacy_code(), ROOT, NEXT_INDEX);
        let state = apply_withdrawal_queue_upgrade(&mut db, BLOCK - 1, false, upgrade()).unwrap();
        assert!(state.is_empty());
        let info = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
        assert_eq!(info.code_hash, legacy_code().hash_slow());
    }

    #[test]
    fn rejects_a_queue_that_does_not_match_the_checkpoint() {
        let other_code = Bytecode::new_raw(bytes!("00"));
        for (name, mut db, block, tsuki_v2_active) in [
            (
                "late",
                drained_queue(legacy_code(), ROOT, NEXT_INDEX),
                BLOCK + 1,
                true,
            ),
            (
                "no TsukiV2",
                drained_queue(legacy_code(), ROOT, NEXT_INDEX),
                BLOCK,
                false,
            ),
            (
                "wrong code",
                drained_queue(other_code, ROOT, NEXT_INDEX),
                BLOCK,
                true,
            ),
            (
                "wrong root",
                drained_queue(legacy_code(), B256::ZERO, NEXT_INDEX),
                BLOCK,
                true,
            ),
            (
                "wrong index",
                drained_queue(legacy_code(), ROOT, NEXT_INDEX + 1),
                BLOCK,
                true,
            ),
            (
                "missing account",
                CacheDB::new(EmptyDB::default()),
                BLOCK,
                true,
            ),
        ] {
            assert!(
                apply_withdrawal_queue_upgrade(&mut db, block, tsuki_v2_active, upgrade()).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn replacement_code_matches_the_mainnet_genesis_queue() {
        let queue = &DOGEOS_MAINNET.genesis().alloc[&L2_MESSAGE_QUEUE_ADDRESS];
        assert_eq!(queue.code.as_ref(), Some(&HASH_CHAIN_QUEUE_BYTECODE));
        assert_eq!(
            Bytecode::new_raw(HASH_CHAIN_QUEUE_BYTECODE).hash_slow(),
            HASH_CHAIN_QUEUE_BYTECODE_HASH
        );
    }
}

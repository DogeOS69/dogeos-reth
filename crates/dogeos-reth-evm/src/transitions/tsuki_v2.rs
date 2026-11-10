//! TsukiV2 fork transition for Dogeos.
//!
//! On the first block of the TsukiV2 fork, a network whose L2MessageQueue at
//! `0x5300000000000000000000000000000000000000` still runs the legacy append-only Merkle-tree code
//! switches it to the hash-chain queue after checking the configured frozen checkpoint. Only the
//! code changes; storage is kept, so the legacy withdrawal root (slot 0), next index (slot 1), owner and messenger carry
//! over and the root seeds the new hash chain. The legacy tree's branch slots stay unused.

use alloc::{format, string::String};
use alloy_primitives::{B256, b256, bytes};
use dogeos_chainspec::WithdrawalQueueCheckpoint;
use revm::{
    Database, DatabaseCommit,
    bytecode::Bytecode,
    primitives::{Bytes, U256},
    state::{AccountInfo, EvmState},
};

use crate::{L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT, WITHDRAW_TRIE_ROOT_SLOT};

/// Code hash of the legacy Merkle-tree L2MessageQueue predeployed in the Chikyu genesis.
const LEGACY_QUEUE_BYTECODE_HASH: B256 =
    b256!("dc3a5ee2e8f8bcfd3a6ae7d2b5f38c8bf5c77b9a3fad31aa5dfe1ffed7aad50c");

// Hash-chain L2MessageQueue runtime from DogeOS69/scroll-contracts@419c693c. The same code is
// predeployed in the mainnet genesis.
// Build: solc 0.8.24, Cancun, optimizer 200 runs, bytecode metadata hash disabled.
const HASH_CHAIN_QUEUE_BYTECODE: Bytes = bytes!(
    "608060405234801561000f575f80fd5b5060043610610085575f3560e01c80638da5cb5b116100585780638da5cb5b146100ed578063c4d66de814610100578063d4b9f4fa14610113578063f2fde38b1461011b575f80fd5b806326aad7b7146100895780633cb747bf146100a5578063600a2e77146100d0578063715018a6146100e3575b5f80fd5b61009260015481565b6040519081526020015b60405180910390f35b6053546100b8906001600160a01b031681565b6040516001600160a01b03909116815260200161009c565b6100926100de366004610412565b61012e565b6100eb6101d1565b005b6052546100b8906001600160a01b031681565b6100eb61010e366004610429565b610206565b6100925f5481565b6100eb610129366004610429565b610296565b6053545f906001600160a01b031633146101805760405162461bcd60e51b815260206004820152600e60248201526d37b7363c9036b2b9b9b2b733b2b960911b60448201526064015b60405180910390fd5b5f8061018b84610322565b60408051838152602081018890529294509092507ffaa617c2d8ce12c62637dbce76efcc18dae60574aa95709bdcedce7e76071693910160405180910390a19392505050565b6052546001600160a01b031633146101fb5760405162461bcd60e51b815260040161017790610456565b6102045f6103c1565b565b6052546001600160a01b031633146102305760405162461bcd60e51b815260040161017790610456565b600154156102745760405162461bcd60e51b815260206004820152601160248201527063616e6e6f7420696e697469616c697a6560781b6044820152606401610177565b605380546001600160a01b0319166001600160a01b0392909216919091179055565b6052546001600160a01b031633146102c05760405162461bcd60e51b815260040161017790610456565b6001600160a01b0381166103165760405162461bcd60e51b815260206004820152601d60248201527f6e6577206f776e657220697320746865207a65726f20616464726573730000006044820152606401610177565b61031f816103c1565b50565b6001545f90819067ffffffffffffffff81106103795760405162461bcd60e51b81526020600482015260166024820152756d65737361676520696e646578206f766572666c6f7760501b6044820152606401610177565b5f80546040805160208082019390935280820188905281518082038301815260609091019091528051910120908190556103b482600161048d565b6001559094909350915050565b605280546001600160a01b038381166001600160a01b0319831681179093556040519116919082907f8be0079c531659141344cd1fd0a4f28419497f9722a3daafe3b4186f6b6457e0905f90a35050565b5f60208284031215610422575f80fd5b5035919050565b5f60208284031215610439575f80fd5b81356001600160a01b038116811461044f575f80fd5b9392505050565b60208082526017908201527f63616c6c6572206973206e6f7420746865206f776e6572000000000000000000604082015260600190565b808201808211156104ac57634e487b7160e01b5f52601160045260245ffd5b9291505056fea164736f6c6343000818000a"
);
const HASH_CHAIN_QUEUE_BYTECODE_HASH: B256 =
    b256!("baf761a1daac2df442f2a373c18f86cbb257d60ee9e62933e5ffa1c9088ab5af");

/// Applies the TsukiV2 hard fork to the state by replacing the legacy L2MessageQueue code with the
/// hash-chain queue.
///
/// The checkpoint binds the legacy code, root and index to the converted bridge snapshot.
/// Missing or mismatched checkpoints reject a legacy queue before any code is changed.
/// An already upgraded queue is a no-op, including after withdrawals advance its root/index.
/// Without a checkpoint, unrelated code and missing accounts are left alone; the known Chikyu
/// legacy queue must have a checkpoint. Balance, nonce and storage are kept.
///
/// Returns the committed state update so the executor can forward it to state hooks.
pub(crate) fn apply_tsuki_v2_hard_fork<DB>(
    db: &mut DB,
    checkpoint: Option<WithdrawalQueueCheckpoint>,
) -> Result<EvmState, String>
where
    DB: Database + DatabaseCommit,
{
    // Compare hashes without loading old bytecode. The witness carries the account and both
    // checkpoint slots; the replacement bytecode is supplied by the executor.
    let old_info = db
        .basic(L2_MESSAGE_QUEUE_ADDRESS)
        .map_err(|err| format!("failed to read the withdrawal queue: {err:?}"))?;
    let Some(old_info) = old_info else {
        return if checkpoint.is_some() {
            Err("the withdrawal queue account is missing".into())
        } else {
            Ok(EvmState::default())
        };
    };
    if old_info.code_hash == HASH_CHAIN_QUEUE_BYTECODE_HASH {
        return Ok(EvmState::default());
    }
    let Some(checkpoint) = checkpoint else {
        return if old_info.code_hash == LEGACY_QUEUE_BYTECODE_HASH {
            Err("the legacy withdrawal queue requires withdrawalQueueCheckpoint at TsukiV2".into())
        } else {
            Ok(EvmState::default())
        };
    };
    if old_info.code_hash != checkpoint.legacy_code_hash {
        return Err(format!(
            "the withdrawal queue code hash is {}, expected {}",
            old_info.code_hash, checkpoint.legacy_code_hash
        ));
    }
    let mut read_slot = |slot| {
        db.storage(L2_MESSAGE_QUEUE_ADDRESS, slot)
            .map_err(|err| format!("failed to read the withdrawal queue slot {slot}: {err:?}"))
    };
    let root = read_slot(WITHDRAW_TRIE_ROOT_SLOT)?;
    let next_index = read_slot(NEXT_MESSAGE_INDEX_SLOT)?;
    if root != U256::from_be_bytes(checkpoint.seed_root.0)
        || next_index != U256::from(checkpoint.next_message_index)
    {
        return Err(format!(
            "the withdrawal queue holds root {} and next index {next_index}, expected the \
             checkpoint's {} and {}",
            B256::from(root),
            checkpoint.seed_root,
            checkpoint.next_message_index
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
    use crate::{
        NEXT_MESSAGE_INDEX_SLOT, ScrollEvmFactory, ScrollTransactionIntoTxEnv,
        TX_L1_FEE_PRECISION_U256, WITHDRAW_TRIE_ROOT_SLOT,
    };
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{Address, TxKind, address, keccak256};
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_MAINNET};
    use reth_chainspec::EthChainSpec;
    use revm::{
        context::{
            BlockEnv, CfgEnv, TxEnv,
            result::{ExecutionResult, Output},
        },
        database::{CacheDB, EmptyDB},
        primitives::U256,
    };
    use revm_scroll::ScrollSpecId;

    const ROOT: B256 = B256::repeat_byte(0x51);
    const NEXT_INDEX: u64 = 1173;
    const OWNER_SLOT: U256 = U256::from_limbs([82, 0, 0, 0]);
    const MESSENGER_SLOT: U256 = U256::from_limbs([83, 0, 0, 0]);
    const MESSENGER: Address = address!("0000000000000000000000000000000000001001");

    fn legacy_code() -> Bytecode {
        let queue = &DOGEOS_CHIKYU.genesis().alloc[&L2_MESSAGE_QUEUE_ADDRESS];
        Bytecode::new_raw(queue.code.clone().unwrap())
    }

    fn checkpoint() -> Option<WithdrawalQueueCheckpoint> {
        Some(WithdrawalQueueCheckpoint {
            legacy_code_hash: legacy_code().hash_slow(),
            seed_root: ROOT,
            next_message_index: NEXT_INDEX,
        })
    }

    fn queue_with(code: Bytecode) -> CacheDB<EmptyDB> {
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
            (WITHDRAW_TRIE_ROOT_SLOT, U256::from_be_bytes(ROOT.0)),
            (NEXT_MESSAGE_INDEX_SLOT, U256::from(NEXT_INDEX)),
            (OWNER_SLOT, U256::from(9)),
            (MESSENGER_SLOT, U256::from_be_slice(MESSENGER.as_slice())),
        ] {
            db.insert_account_storage(L2_MESSAGE_QUEUE_ADDRESS, slot, value)
                .unwrap();
        }
        db
    }

    #[test]
    fn replaces_only_the_legacy_queue_code() {
        let mut db = queue_with(legacy_code());
        let state = apply_tsuki_v2_hard_fork(&mut db, checkpoint()).unwrap();
        assert!(state[&L2_MESSAGE_QUEUE_ADDRESS].storage.is_empty());

        let info = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
        assert_eq!(info.code_hash, HASH_CHAIN_QUEUE_BYTECODE_HASH);
        assert_eq!((info.balance, info.nonce), (U256::from(7), 3));
        for (slot, value) in [
            (WITHDRAW_TRIE_ROOT_SLOT, U256::from_be_bytes(ROOT.0)),
            (NEXT_MESSAGE_INDEX_SLOT, U256::from(NEXT_INDEX)),
            (OWNER_SLOT, U256::from(9)),
            (MESSENGER_SLOT, U256::from_be_slice(MESSENGER.as_slice())),
        ] {
            assert_eq!(db.storage(L2_MESSAGE_QUEUE_ADDRESS, slot).unwrap(), value);
        }

        // Later TsukiV2 blocks leave the replaced queue alone.
        assert!(
            apply_tsuki_v2_hard_fork(&mut db, checkpoint())
                .unwrap()
                .is_empty()
        );
    }

    /// The messenger kept in storage appends the next withdrawal on top of the legacy root.
    #[test]
    fn the_replaced_queue_continues_the_legacy_root() {
        let mut db = queue_with(legacy_code());
        apply_tsuki_v2_hard_fork(&mut db, checkpoint()).unwrap();

        let message_hash = B256::repeat_byte(0xab);
        // appendMessage(bytes32)
        let mut data = vec![0x60, 0x0a, 0x2e, 0x77];
        data.extend_from_slice(message_hash.as_slice());
        let tx = ScrollTransactionIntoTxEnv::new(
            TxEnv {
                caller: MESSENGER,
                kind: TxKind::Call(L2_MESSAGE_QUEUE_ADDRESS),
                gas_limit: 1_000_000,
                data: data.into(),
                ..Default::default()
            },
            Some(Bytes::new()),
            Some(TX_L1_FEE_PRECISION_U256),
            Some(0),
        );
        let env = EvmEnv::new(
            CfgEnv::new_with_spec(ScrollSpecId::TSUKI_V2),
            BlockEnv::default(),
        );
        let evm_factory: ScrollEvmFactory = ScrollEvmFactory::default();
        let mut evm = evm_factory.create_evm(&mut db, env);
        let result = evm.transact_commit(tx).unwrap();

        let root = keccak256([ROOT.as_slice(), message_hash.as_slice()].concat());
        match result {
            ExecutionResult::Success {
                output: Output::Call(output),
                ..
            } => assert_eq!(output.as_ref(), root.as_slice()),
            result => panic!("appendMessage failed: {result:?}"),
        }
        let mut queue_slot = |slot| {
            evm.db_mut()
                .storage(L2_MESSAGE_QUEUE_ADDRESS, slot)
                .unwrap()
        };
        assert_eq!(
            queue_slot(WITHDRAW_TRIE_ROOT_SLOT),
            U256::from_be_bytes(root.0)
        );
        assert_eq!(
            queue_slot(NEXT_MESSAGE_INDEX_SLOT),
            U256::from(NEXT_INDEX + 1)
        );
        // Re-running the transition must preserve withdrawals made after activation.
        assert!(
            apply_tsuki_v2_hard_fork(evm.db_mut(), checkpoint())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            evm.db_mut()
                .storage(L2_MESSAGE_QUEUE_ADDRESS, WITHDRAW_TRIE_ROOT_SLOT)
                .unwrap(),
            U256::from_be_bytes(root.0)
        );
        assert_eq!(
            evm.db_mut()
                .storage(L2_MESSAGE_QUEUE_ADDRESS, NEXT_MESSAGE_INDEX_SLOT)
                .unwrap(),
            U256::from(NEXT_INDEX + 1)
        );
    }

    /// The hash-chain queue (mainnet genesis), any other code, and a missing account are left
    /// alone.
    #[test]
    fn leaves_any_other_queue_alone() {
        for code in [
            Bytecode::new_raw(HASH_CHAIN_QUEUE_BYTECODE),
            Bytecode::new_raw(bytes!("00")),
        ] {
            let mut db = queue_with(code.clone());
            assert!(apply_tsuki_v2_hard_fork(&mut db, None).unwrap().is_empty());
            let info = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
            assert_eq!(info.code_hash, code.hash_slow());
        }

        let mut db = CacheDB::new(EmptyDB::default());
        assert!(apply_tsuki_v2_hard_fork(&mut db, None).unwrap().is_empty());
        assert!(db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().is_none());
    }

    #[test]
    fn rejects_missing_or_mismatched_checkpoints_before_changing_code() {
        let valid = checkpoint().unwrap();
        for (expected, checkpoint) in [
            ("requires withdrawalQueueCheckpoint", None),
            (
                "code hash is",
                Some(WithdrawalQueueCheckpoint {
                    legacy_code_hash: B256::ZERO,
                    ..valid
                }),
            ),
            (
                "holds root",
                Some(WithdrawalQueueCheckpoint {
                    seed_root: B256::ZERO,
                    ..valid
                }),
            ),
            (
                "next index",
                Some(WithdrawalQueueCheckpoint {
                    next_message_index: NEXT_INDEX + 1,
                    ..valid
                }),
            ),
        ] {
            let mut db = queue_with(legacy_code());
            let before = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
            let error = apply_tsuki_v2_hard_fork(&mut db, checkpoint).unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert_eq!(db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap(), before);
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
        }
    }

    #[test]
    fn configured_checkpoint_rejects_missing_and_unexpected_queue_code() {
        let mut db = CacheDB::new(EmptyDB::default());
        let error = apply_tsuki_v2_hard_fork(&mut db, checkpoint()).unwrap_err();
        assert!(error.contains("account is missing"), "{error}");
        let mut db = queue_with(Bytecode::new_raw(bytes!("00")));
        let before = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap();
        let error = apply_tsuki_v2_hard_fork(&mut db, checkpoint()).unwrap_err();
        assert!(error.contains("code hash is"), "{error}");
        assert_eq!(db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap(), before);
    }

    #[test]
    fn upgraded_queue_does_not_read_or_validate_checkpoint_storage() {
        struct UpgradedQueue;
        impl Database for UpgradedQueue {
            type Error = core::convert::Infallible;
            fn basic(&mut self, _: Address) -> Result<Option<AccountInfo>, Self::Error> {
                Ok(Some(AccountInfo {
                    code_hash: HASH_CHAIN_QUEUE_BYTECODE_HASH,
                    ..Default::default()
                }))
            }
            fn code_by_hash(&mut self, _: B256) -> Result<Bytecode, Self::Error> {
                panic!("must not load code")
            }
            fn storage(&mut self, _: Address, _: U256) -> Result<U256, Self::Error> {
                panic!("must not read the old checkpoint after migration")
            }
            fn block_hash(&mut self, _: u64) -> Result<B256, Self::Error> {
                panic!("must not read block hashes")
            }
        }
        impl DatabaseCommit for UpgradedQueue {
            fn commit(&mut self, _: EvmState) {
                panic!("must not commit an update")
            }
        }
        for checkpoint in [None, checkpoint()] {
            assert!(
                apply_tsuki_v2_hard_fork(&mut UpgradedQueue, checkpoint)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn queue_codes_match_the_genesis_predeploys() {
        let queue = &DOGEOS_MAINNET.genesis().alloc[&L2_MESSAGE_QUEUE_ADDRESS];
        assert_eq!(queue.code.as_ref(), Some(&HASH_CHAIN_QUEUE_BYTECODE));
        assert_eq!(
            Bytecode::new_raw(HASH_CHAIN_QUEUE_BYTECODE).hash_slow(),
            HASH_CHAIN_QUEUE_BYTECODE_HASH
        );
        assert_eq!(legacy_code().hash_slow(), LEGACY_QUEUE_BYTECODE_HASH);
    }
}

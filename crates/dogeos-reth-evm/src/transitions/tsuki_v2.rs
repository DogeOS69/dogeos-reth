//! TsukiV2 fork transition for Dogeos.
//!
//! On the first block of the TsukiV2 fork, a network whose L2MessageQueue at
//! `0x5300000000000000000000000000000000000000` still runs the legacy append-only Merkle-tree code
//! of the Chikyu genesis switches it to the hash-chain queue. Only the code changes; storage is
//! kept, so the legacy withdrawal root (slot 0), next index (slot 1), owner and messenger carry
//! over and the root seeds the new hash chain. The legacy tree's branch slots stay unused.

use alloy_primitives::{B256, b256, bytes};
use revm::{
    Database, DatabaseCommit,
    bytecode::Bytecode,
    primitives::Bytes,
    state::{AccountInfo, EvmState},
};

use crate::L2_MESSAGE_QUEUE_ADDRESS;

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
/// Only an account that still runs the legacy code is changed, so the migration is a no-op on every
/// later block and on networks whose genesis already holds the hash-chain queue (mainnet). Balance,
/// nonce and storage are kept.
///
/// Returns the committed state update so the executor can forward it to state hooks.
pub(crate) fn apply_tsuki_v2_hard_fork<DB>(db: &mut DB) -> Result<EvmState, DB::Error>
where
    DB: Database + DatabaseCommit,
{
    // Only the code hash is compared, so the legacy code is never loaded and the read needs
    // nothing beyond the queue account, which the witness carries for the withdrawal root.
    let Some(old_info) = db.basic(L2_MESSAGE_QUEUE_ADDRESS)? else {
        return Ok(EvmState::default());
    };
    if old_info.code_hash != LEGACY_QUEUE_BYTECODE_HASH {
        return Ok(EvmState::default());
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
        let state = apply_tsuki_v2_hard_fork(&mut db).unwrap();
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
        assert!(apply_tsuki_v2_hard_fork(&mut db).unwrap().is_empty());
    }

    /// The messenger kept in storage appends the next withdrawal on top of the legacy root.
    #[test]
    fn the_replaced_queue_continues_the_legacy_root() {
        let mut db = queue_with(legacy_code());
        apply_tsuki_v2_hard_fork(&mut db).unwrap();

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
            CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
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
            assert!(apply_tsuki_v2_hard_fork(&mut db).unwrap().is_empty());
            let info = db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().unwrap();
            assert_eq!(info.code_hash, code.hash_slow());
        }

        let mut db = CacheDB::new(EmptyDB::default());
        assert!(apply_tsuki_v2_hard_fork(&mut db).unwrap().is_empty());
        assert!(db.basic(L2_MESSAGE_QUEUE_ADDRESS).unwrap().is_none());
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

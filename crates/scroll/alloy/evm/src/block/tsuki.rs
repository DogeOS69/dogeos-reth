//! Tsuki fork transition for Dogeos.
//!
//! On the first block of the Tsuki fork, Dogeos performed a transition to the Tsuki fork state,
//! changes to the protocol:
//!   1. Set the code of address `0x530000000000000000000000000000000000d09e` to NativeDogeToken
//!      bytecode.

use alloc::vec;
use alloy_primitives::{b256, bytes, B256};
use revm::{
    bytecode::Bytecode,
    database::{
        bal::EvmDatabaseError,
        states::{State, StorageSlot},
    },
    primitives::{Bytes, U256},
    state::AccountInfo,
    Database,
};
use revm_scroll::precompile::transfer::NATIVE_DOGE_TOKEN_ADDRESS;

// Generated from DogeOS69/scroll-contracts@88c845b1 with `yarn export:native-doge-token`.
// Build: solc 0.8.24, Cancun, optimizer 200 runs, bytecode metadata hash disabled.
// This runtime follows Celo semantics: CALL status indicates transfer success;
// the native-transfer precompile returns empty data on success.
const TSUKI_NATIVE_DOGE_TOKEN_BYTECODE: Bytes = bytes!("608060405234801561000f575f80fd5b5060043610610090575f3560e01c8063313ce56711610063578063313ce5671461011657806370a082311461012557806395d89b4114610140578063a9059cbb14610160578063dd62ed3e14610173575f80fd5b806306fdde0314610094578063095ea7b3146100ca57806318160ddd146100ed57806323b872dd14610103575b5f80fd5b6040805180820190915260088152672237b3b2b1b7b4b760c11b60208201525b6040516100c1919061054e565b60405180910390f35b6100dd6100d8366004610596565b6101ab565b60405190151581526020016100c1565b6100f5610238565b6040519081526020016100c1565b6100dd6101113660046105be565b610260565b604051601281526020016100c1565b6100f56101333660046105f7565b6001600160a01b03163190565b604080518082019091526004815263444f474560e01b60208201526100b4565b6100dd61016e366004610596565b6103b0565b6100f5610181366004610617565b6001600160a01b039182165f90815260016020908152604080832093909416825291909152205490565b5f6001600160a01b0383166101d357604051630571f51760e01b815260040160405180910390fd5b335f8181526001602090815260408083206001600160a01b03881680855290835292819020869055518581529192917f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b92591015b60405180910390a35060015b92915050565b5f805480820361025b5760405163405641a960e01b815260040160405180910390fd5b919050565b5f6001600160a01b0384166102885760405163105fb90f60e31b815260040160405180910390fd5b6001600160a01b0383166102af57604051637a31731760e01b815260040160405180910390fd5b6001600160a01b0384165f9081526001602090815260408083203384529091529020545f19811461034d578281101561031f5760405163ba805b7560e01b81526001600160a01b038616600482015233602482015260448101829052606481018490526084015b60405180910390fd5b6103298382610648565b6001600160a01b0386165f9081526001602090815260408083203384529091529020555b610358858585610420565b836001600160a01b0316856001600160a01b03167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef8560405161039d91815260200190565b60405180910390a3506001949350505050565b5f6001600160a01b0383166103d857604051637a31731760e01b815260040160405180910390fd5b6103e3338484610420565b6040518281526001600160a01b0384169033907fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef90602001610226565b6001600160a01b038316318181101561046557604051630c898ac560e21b81526001600160a01b03851660048201526024810182905260448101839052606401610316565b604080516001600160a01b038681166020830152851691810191909152606081018390525f9060fd9060800160408051601f19818403018152908290526104ab91610667565b5f604051808303815f865af19150503d805f81146104e4576040519150601f19603f3d011682016040523d82523d5f602084013e6104e9565b606091505b50509050806105255760405163238b556560e21b81526001600160a01b0380871660048301528516602482015260448101849052606401610316565b5050505050565b5f5b8381101561054657818101518382015260200161052e565b50505f910152565b602081525f825180602084015261056c81604085016020870161052c565b601f01601f19169190910160400192915050565b80356001600160a01b038116811461025b575f80fd5b5f80604083850312156105a7575f80fd5b6105b083610580565b946020939093013593505050565b5f805f606084860312156105d0575f80fd5b6105d984610580565b92506105e760208501610580565b9150604084013590509250925092565b5f60208284031215610607575f80fd5b61061082610580565b9392505050565b5f8060408385031215610628575f80fd5b61063183610580565b915061063f60208401610580565b90509250929050565b8181038181111561023257634e487b7160e01b5f52601160045260245ffd5b5f825161067881846020870161052c565b919091019291505056fea164736f6c6343000818000a");
const TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH: B256 =
    b256!("90a64eee730d7b76311162eaac2977d5a2f0608dc01641e365c4173aa8da1384");
const TSUKI_NATIVE_DOGE_TOKEN_TOTAL_SUPPLY: U256 =
    U256::from_limbs([0, 0, 0, 0x0080_0000_0000_0000]);
const TSUKI_NATIVE_DOGE_TOKEN_STORAGE: [(U256, U256); 1] =
    [(U256::ZERO, TSUKI_NATIVE_DOGE_TOKEN_TOTAL_SUPPLY)];

/// Applies the Tsuki hard fork to the state by installing the NativeDogeToken predeploy.
///
/// The token account remains ordinary EVM state. The transfer precompile hardcodes this address as
/// its only allowed caller, and this migration only creates the account if it is still empty. This
/// makes the transition compatible with mainnet genesis predeploys: if genesis already contains
/// code at the same address, the migration is a no-op and does not overwrite it.
pub(super) fn apply_tsuki_hard_fork<DB: Database>(
    state: &mut State<DB>,
) -> Result<(), <State<DB> as Database>::Error> {
    let token =
        state.load_cache_account(NATIVE_DOGE_TOKEN_ADDRESS).map_err(EvmDatabaseError::Database)?;

    let old_info = token.account_info().unwrap_or_default();
    if old_info.nonce != 0 || !old_info.is_empty_code_hash() {
        return Ok(());
    }

    let bytecode = Bytecode::new_raw(TSUKI_NATIVE_DOGE_TOKEN_BYTECODE);
    debug_assert_eq!(bytecode.hash_slow(), TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH);
    let code_hash = TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH;
    let new_info = AccountInfo { nonce: 1, code_hash, code: Some(bytecode), ..old_info };
    let new_storage = TSUKI_NATIVE_DOGE_TOKEN_STORAGE
        .into_iter()
        .map(|(slot, present_value)| {
            (
                slot,
                StorageSlot {
                    present_value,
                    previous_or_original_value: token.storage_slot(slot).unwrap_or_default(),
                },
            )
        })
        .collect();

    let transition = token.change(new_info, new_storage);

    if let Some(s) = state.transition_state.as_mut() {
        s.add_transitions(vec![(NATIVE_DOGE_TOKEN_ADDRESS, transition)])
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollEvmFactory, ScrollTransactionIntoTxEnv, TX_L1_FEE_PRECISION_U256};
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{address, Address, TxKind};
    use revm::{
        context::{
            result::{ExecutionResult, Output},
            BlockEnv, CfgEnv, TxEnv,
        },
        database::{
            states::{bundle_state::BundleRetention, StorageSlot},
            CacheDB, EmptyDB, State,
        },
        primitives::{bytes, U256},
        state::{AccountInfo, Bytecode},
    };
    use revm_scroll::ScrollSpecId;

    fn native_doge_token_tx(
        caller: Address,
        nonce: u64,
        data: Bytes,
    ) -> ScrollTransactionIntoTxEnv<TxEnv> {
        ScrollTransactionIntoTxEnv::new(
            TxEnv {
                caller,
                nonce,
                kind: TxKind::Call(NATIVE_DOGE_TOKEN_ADDRESS),
                gas_limit: 1_000_000,
                data,
                ..Default::default()
            },
            Some(Bytes::new()),
            Some(TX_L1_FEE_PRECISION_U256),
            Some(0),
        )
    }

    fn transfer_calldata(recipient: Address, amount: U256) -> Bytes {
        // transfer(address,uint256)
        let mut calldata = Vec::with_capacity(68);
        calldata.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
        calldata.extend_from_slice(&[0; 12]);
        calldata.extend_from_slice(recipient.as_slice());
        calldata.extend_from_slice(&amount.to_be_bytes::<32>());
        calldata.into()
    }

    #[test]
    fn test_apply_tsuki_fork_inserts_native_doge_token() -> eyre::Result<()> {
        let db = EmptyDB::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        apply_tsuki_hard_fork(&mut state)?;

        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();
        let token = bundle.state.get(&NATIVE_DOGE_TOKEN_ADDRESS).unwrap();

        let code_hash = TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH;
        let bytecode = Bytecode::new_raw(TSUKI_NATIVE_DOGE_TOKEN_BYTECODE);
        assert_eq!(bytecode.hash_slow(), code_hash);
        let expected_info = AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash,
            code: Some(bytecode.clone()),
            ..Default::default()
        };

        assert_eq!(token.info.as_ref().unwrap(), &expected_info);
        assert_eq!(
            token.storage.get(&U256::ZERO),
            Some(&StorageSlot {
                present_value: TSUKI_NATIVE_DOGE_TOKEN_TOTAL_SUPPLY,
                ..Default::default()
            })
        );

        // check deployed contract
        assert_eq!(bundle.contracts.get(&code_hash).unwrap(), &bytecode);

        Ok(())
    }

    #[test]
    fn test_native_doge_token_transfer_uses_tsuki_precompile() -> eyre::Result<()> {
        let sender = address!("0000000000000000000000000000000000001001");
        let recipient = address!("0000000000000000000000000000000000001002");
        let initial_balance = U256::from(1_000);
        let transfer_amount = U256::from(250);

        let db = EmptyDB::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        state
            .insert_account(sender, AccountInfo { balance: initial_balance, ..Default::default() });
        apply_tsuki_hard_fork(&mut state)?;
        assert_eq!(
            state.basic(NATIVE_DOGE_TOKEN_ADDRESS)?.unwrap().code_hash,
            TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH
        );
        assert_eq!(
            state.storage(NATIVE_DOGE_TOKEN_ADDRESS, U256::ZERO)?,
            TSUKI_NATIVE_DOGE_TOKEN_TOTAL_SUPPLY
        );

        let evm_env = EvmEnv::new(CfgEnv::new_with_spec(ScrollSpecId::TSUKI), BlockEnv::default());
        let evm_factory: ScrollEvmFactory = ScrollEvmFactory::default();
        let mut evm = evm_factory.create_evm(&mut state, evm_env);

        // totalSupply()
        let result = evm.transact_commit(native_doge_token_tx(sender, 0, bytes!("18160ddd")))?;
        match result {
            ExecutionResult::Success { output: Output::Call(output), .. } => {
                assert_eq!(
                    output.as_ref(),
                    TSUKI_NATIVE_DOGE_TOKEN_TOTAL_SUPPLY.to_be_bytes::<32>()
                );
            }
            result => panic!("NativeDogeToken totalSupply failed: {result:?}"),
        }

        let tx = native_doge_token_tx(sender, 1, transfer_calldata(recipient, transfer_amount));
        let result = evm.transact_commit(tx)?;

        match result {
            ExecutionResult::Success { output: Output::Call(output), .. } => {
                assert_eq!(output.as_ref(), U256::from(1).to_be_bytes::<32>());
            }
            result => panic!("NativeDogeToken transfer failed: {result:?}"),
        }

        assert_eq!(
            evm.db_mut().basic(sender)?.unwrap_or_default().balance,
            initial_balance - transfer_amount
        );
        assert_eq!(evm.db_mut().basic(recipient)?.unwrap_or_default().balance, transfer_amount);

        Ok(())
    }

    #[test]
    fn test_native_doge_token_transfer_reverts_for_insufficient_balance() -> eyre::Result<()> {
        let sender = address!("0000000000000000000000000000000000001001");
        let recipient = address!("0000000000000000000000000000000000001002");
        let initial_balance = U256::from(1_000);

        let db = EmptyDB::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();
        state
            .insert_account(sender, AccountInfo { balance: initial_balance, ..Default::default() });
        apply_tsuki_hard_fork(&mut state)?;

        let evm_env = EvmEnv::new(CfgEnv::new_with_spec(ScrollSpecId::TSUKI), BlockEnv::default());
        let evm_factory: ScrollEvmFactory = ScrollEvmFactory::default();
        let mut evm = evm_factory.create_evm(&mut state, evm_env);

        let transfer_amount = initial_balance + U256::from(1);
        let tx = native_doge_token_tx(sender, 0, transfer_calldata(recipient, transfer_amount));
        let result = evm.transact_commit(tx)?;

        assert!(
            matches!(&result, ExecutionResult::Revert { .. }),
            "expected insufficient-balance transfer to revert, got {result:?}"
        );
        assert_eq!(evm.db_mut().basic(sender)?.unwrap_or_default().balance, initial_balance);
        assert_eq!(evm.db_mut().basic(recipient)?.unwrap_or_default().balance, U256::ZERO);

        Ok(())
    }

    #[test]
    fn test_apply_tsuki_fork_does_not_overwrite_existing_predeploy() -> eyre::Result<()> {
        let bytecode = Bytecode::new_raw(bytes!("00"));
        let predeploy_info = AccountInfo {
            code_hash: bytecode.hash_slow(),
            code: Some(bytecode),
            ..Default::default()
        };

        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(NATIVE_DOGE_TOKEN_ADDRESS, predeploy_info);
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        apply_tsuki_hard_fork(&mut state)?;

        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        assert_eq!(bundle.state.get(&NATIVE_DOGE_TOKEN_ADDRESS), None);

        Ok(())
    }

    #[test]
    fn test_apply_tsuki_fork_does_not_run_twice() -> eyre::Result<()> {
        let bytecode = Bytecode::new_raw(TSUKI_NATIVE_DOGE_TOKEN_BYTECODE);
        let token_info = AccountInfo {
            nonce: 1,
            code_hash: bytecode.hash_slow(),
            code: Some(bytecode),
            ..Default::default()
        };

        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(NATIVE_DOGE_TOKEN_ADDRESS, token_info);
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        apply_tsuki_hard_fork(&mut state)?;

        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        assert_eq!(bundle.state.get(&NATIVE_DOGE_TOKEN_ADDRESS), None);

        Ok(())
    }
}

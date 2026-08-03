//! Tsuki fork transition for Dogeos.
//!
//! On the first block of the Tsuki fork, Dogeos performed a transition to the Tsuki fork state,
//! changes to the protocol:
//!   1. Set the code of address `0x530000000000000000000000000000000000d09e` to NativeDogeToken
//!      bytecode.

use alloc::vec;
use alloy_primitives::{B256, b256, bytes};
use revm::{
    Database,
    bytecode::Bytecode,
    database::{
        bal::EvmDatabaseError,
        states::{State, StorageSlot},
    },
    primitives::{Bytes, U256},
    state::AccountInfo,
};
use revm_scroll::precompile::transfer::NATIVE_DOGE_TOKEN_ADDRESS;

const TSUKI_NATIVE_DOGE_TOKEN_BYTECODE: Bytes = bytes!(
    "608060405234801561000f575f80fd5b5060043610610090575f3560e01c8063313ce56711610063578063313ce5671461011657806370a082311461012557806395d89b4114610140578063a9059cbb14610160578063dd62ed3e14610173575f80fd5b806306fdde0314610094578063095ea7b3146100ca57806318160ddd146100ed57806323b872dd14610103575b5f80fd5b6040805180820190915260088152672237b3b2b1b7b4b760c11b60208201525b6040516100c19190610580565b60405180910390f35b6100dd6100d83660046105c8565b6101ab565b60405190151581526020016100c1565b6100f5610238565b6040519081526020016100c1565b6100dd6101113660046105f0565b610260565b604051601281526020016100c1565b6100f5610133366004610629565b6001600160a01b03163190565b604080518082019091526004815263444f474560e01b60208201526100b4565b6100dd61016e3660046105c8565b6103b0565b6100f5610181366004610649565b6001600160a01b039182165f90815260016020908152604080832093909416825291909152205490565b5f6001600160a01b0383166101d357604051630571f51760e01b815260040160405180910390fd5b335f8181526001602090815260408083206001600160a01b03881680855290835292819020869055518581529192917f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b92591015b60405180910390a35060015b92915050565b5f805480820361025b5760405163405641a960e01b815260040160405180910390fd5b919050565b5f6001600160a01b0384166102885760405163105fb90f60e31b815260040160405180910390fd5b6001600160a01b0383166102af57604051637a31731760e01b815260040160405180910390fd5b6001600160a01b0384165f9081526001602090815260408083203384529091529020545f19811461034d578281101561031f5760405163ba805b7560e01b81526001600160a01b038616600482015233602482015260448101829052606481018490526084015b60405180910390fd5b610329838261067a565b6001600160a01b0386165f9081526001602090815260408083203384529091529020555b610358858585610420565b836001600160a01b0316856001600160a01b03167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef8560405161039d91815260200190565b60405180910390a3506001949350505050565b5f6001600160a01b0383166103d857604051637a31731760e01b815260040160405180910390fd5b6103e3338484610420565b6040518281526001600160a01b0384169033907fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef90602001610226565b6001600160a01b038316318181101561046557604051630c898ac560e21b81526001600160a01b03851660048201526024810182905260448101839052606401610316565b604080516001600160a01b038681166020830152851691810191909152606081018390525f90819060fd9060800160408051601f19818403018152908290526104ad91610699565b5f604051808303815f865af19150503d805f81146104e6576040519150601f19603f3d011682016040523d82523d5f602084013e6104eb565b606091505b50915091508115806104ff57508051602014155b8061051e57508080602001905181019061051991906106b4565b600114155b156105565760405163238b556560e21b81526001600160a01b0380881660048301528616602482015260448101859052606401610316565b505050505050565b5f5b83811015610578578181015183820152602001610560565b50505f910152565b602081525f825180602084015261059e81604085016020870161055e565b601f01601f19169190910160400192915050565b80356001600160a01b038116811461025b575f80fd5b5f80604083850312156105d9575f80fd5b6105e2836105b2565b946020939093013593505050565b5f805f60608486031215610602575f80fd5b61060b846105b2565b9250610619602085016105b2565b9150604084013590509250925092565b5f60208284031215610639575f80fd5b610642826105b2565b9392505050565b5f806040838503121561065a575f80fd5b610663836105b2565b9150610671602084016105b2565b90509250929050565b8181038181111561023257634e487b7160e01b5f52601160045260245ffd5b5f82516106aa81846020870161055e565b9190910192915050565b5f602082840312156106c4575f80fd5b505191905056fea164736f6c6343000818000a"
);
const TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH: B256 =
    b256!("90ab6533669ec49eb9cc98628aa6e142180477b8cc6c26ea80386fa6a858a51a");
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
pub fn apply_tsuki_hard_fork<DB: Database>(
    state: &mut State<DB>,
) -> Result<(), <State<DB> as Database>::Error> {
    let token = state
        .load_cache_account(NATIVE_DOGE_TOKEN_ADDRESS)
        .map_err(EvmDatabaseError::Database)?;

    let old_info = token.account_info().unwrap_or_default();
    if old_info.nonce != 0 || !old_info.is_empty_code_hash() {
        return Ok(());
    }

    let bytecode = Bytecode::new_raw(TSUKI_NATIVE_DOGE_TOKEN_BYTECODE);
    debug_assert_eq!(bytecode.hash_slow(), TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH);
    let code_hash = TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH;
    let new_info = AccountInfo {
        nonce: 1,
        code_hash,
        code: Some(bytecode),
        ..old_info
    };
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
    use revm::{
        database::{
            CacheDB, EmptyDB, State,
            states::{StorageSlot, bundle_state::BundleRetention},
        },
        primitives::{U256, bytes},
        state::{AccountInfo, Bytecode},
    };

    #[test]
    fn test_apply_tsuki_fork_inserts_native_doge_token() -> eyre::Result<()> {
        let db = EmptyDB::new();
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .build();

        apply_tsuki_hard_fork(&mut state)?;

        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();
        let token = bundle.state.get(&NATIVE_DOGE_TOKEN_ADDRESS).unwrap();

        let code_hash = TSUKI_NATIVE_DOGE_TOKEN_BYTECODE_HASH;
        let bytecode = Bytecode::new_raw(TSUKI_NATIVE_DOGE_TOKEN_BYTECODE);
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
    fn test_apply_tsuki_fork_does_not_overwrite_existing_predeploy() -> eyre::Result<()> {
        let bytecode = Bytecode::new_raw(bytes!("00"));
        let predeploy_info = AccountInfo {
            code_hash: bytecode.hash_slow(),
            code: Some(bytecode),
            ..Default::default()
        };

        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(NATIVE_DOGE_TOKEN_ADDRESS, predeploy_info);
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .build();

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
        let mut state = State::builder()
            .with_database(db)
            .with_bundle_update()
            .build();

        apply_tsuki_hard_fork(&mut state)?;

        state.merge_transitions(BundleRetention::Reverts);
        let bundle = state.take_bundle();

        assert_eq!(bundle.state.get(&NATIVE_DOGE_TOKEN_ADDRESS), None);

        Ok(())
    }
}

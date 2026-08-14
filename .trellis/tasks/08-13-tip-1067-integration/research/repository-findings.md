# Repository findings for TIP-1067 integration

## Source and current behavior

- `/Users/hhq/Downloads/tip-1067.md` is technical input only. Its reusable shape
  is a fixed-target EIP-1559 controller with truncating division, minimum upward
  delta, clamps, activation seed, and exact header validation.
- `crates/dogeos-reth-evm/src/base_fee.rs` currently subtracts system-config slot
  `101` overhead, runs Alloy's standard parent-gas-limit-derived EIP-1559 helper,
  adds overhead back, and caps the total at `10_000_000_000`.
- Payload construction and the final `eth_feeHistory` entry already call
  `ScrollBaseFeeProvider::next_block_base_fee`.
- `DogeosConsensus` currently checks only fee presence and a global 10 Gwei cap;
  it has no chain spec or state access.

## State layout

- `/Users/hhq/workspace/scroll-contracts/src/L2/L2SystemConfig.sol` declares
  `baseFeeOverhead` then `baseFeeScalar`. With its OpenZeppelin upgradeable
  inheritance this matches the client's existing overhead slot `101`; scalar is
  `102`. The new controller state deliberately does not consume the next
  sequential Solidity slot. It uses
  `keccak256("dogeos.storage.dynamic_base_fee.next_controlled_fee")`, whose
  fixed value is
  `0x74ae897ed5751dd32419f1eee8d4ec13d296adf0d77978ea55df0dd18345c8e3`.
- Every future DogeOS protocol-owned storage slot must follow the same stable,
  domain-separated Keccak-256 namespace convention. The implementation should
  provide a reusable derivation helper or equivalent canonical pattern plus a
  fixed-vector test, so later additions do not return to sequential integers or
  silently change byte interpretation.
- Bundled mainnet/Chikyu genesis documents configure
  `l2SystemConfigAddress = 0x2669B071E88e272CBDA1e12182D8C754CB737400`
  but do not allocate that address in L2 state.
- Pinned `revm-database 12.0.0` applies EIP-161 state clear to a touched account
  whose `AccountInfo` is empty, discarding supplied changed storage. A first
  protocol write therefore needs to preserve existing account data and set nonce
  `1` when the account would otherwise remain empty.
- `crates/dogeos-reth-evm/src/transitions/mod.rs:17-44` provides
  `commit_account_update`, which reads original slots before `DatabaseCommit` and
  is the correct witness/revert-safe write path.

## Execution path

- `ScrollBlockExecutor::apply_pre_execution_changes` runs before transactions and
  has both the EVM block environment and parent-state database.
- `ScrollBlockExecutor` accumulates actual transaction gas in `self.gas_used`.
- The default `apply_post_execution_changes` delegates to `finish`; overriding
  or extending `finish` allows the Keccak-derived controlled-fee slot to be
  written before the EVM/state overlay is returned for state-root construction.
- `ConfigureEvm::context_for_next_block` sees a parent header, but imported-block
  `context_for_block` sees only the current block. Persisting the next controlled
  fee at the end of the parent avoids adding provider/header resolution to the
  executor context.

## Arithmetic and bounds

- At 20M gas limit, 10M target, and denominator 8, a full block multiplies the
  unclamped controlled fee by about 1.125.
- From the 500 Gwei seed, an unbounded internal value exceeds the 1,000 Gwei
  visible cap after about 6 full blocks, `u64::MAX` after about 148, and
  `U256::MAX` after about 1,278. The user therefore approved reusing the single
  `MAX_L2_BASE_FEE` constant for both controlled state and final header clamps.
- `MAX_L2_BASE_FEE * u64::MAX` fits in `u128`; controller operations should still
  be checked and final conversion should occur only after clamps.

## Compatibility

- Existing DogeOS hardforks are timestamp-based, and the controller will reuse
  the existing `Tsuki` activation gate rather than introduce a new hardfork,
  genesis field, or schedule entry.
- Before Tsuki, the legacy 10 Gwei cap and EIP-1559 calculation remain
  unchanged. At and after Tsuki, the new controller and 1,000 Gwei cap apply.
- The target deployment has not activated Tsuki, so it needs no already-active
  state migration. This task does not modify bundled Tsuki schedules or choose
  a production activation timestamp; compatibility with a non-target chain that
  already activated Tsuki without the controller is explicitly out of scope.
- Frozen genesis hashes and the 20M sequencer gas-limit default remain unchanged.
- `dogeos-revm` only consumes the header base fee and needs no modification.

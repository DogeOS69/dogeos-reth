# TIP-1067-inspired base-fee implementation plan

## Implementation result

Implemented on 2026-08-14. The focused affected-package tests, affected-package
Clippy gate, workspace all-target check, EVM no-default-features check, formatting,
and diff checks pass. See `handoff.md` for the delivered behavior and deferred
activation/economic decisions.

## Preconditions

- [ ] Review and explicitly approve `prd.md`, `design.md`, and this plan.
- [ ] Run `task.py start` only after that approval.
- [ ] Load package guidelines with `trellis-before-dev` before product edits.
- [ ] Preserve unrelated worktree changes.

## 1. Add canonical parameters and bind them to Tsuki

- [ ] Define one DogeOS dynamic-fee parameter structure/constants in
      `crates/dogeos-chainspec/src/constants.rs`, including the separate legacy
      cap and the new shared cap.
- [ ] Reuse `DogeosHardforks::is_tsuki_active_at_timestamp`; do not add a fork,
      genesis field, or activation schedule.
- [ ] Add chainspec tests for legacy/Tsuki parameter selection and prove bundled
      schedules and frozen genesis hashes are unchanged.

Validation checkpoint:

```bash
cargo test -p dogeos-chainspec
```

Rollback point: parameter changes are independently revertible before
state-transition work begins; no activation schedule is changed.

## 2. Implement the canonical controller

- [ ] Refactor `crates/dogeos-reth-evm/src/base_fee.rs` so pre-Tsuki calls retain
      the current parent-header EIP-1559 calculation and 10 Gwei cap.
- [ ] Keep inherited overhead slot `101`, and derive the new controlled-fee slot
      from `keccak256("dogeos.storage.dynamic_base_fee.next_controlled_fee")`.
- [ ] Add a reusable namespace-to-slot derivation/verification helper or pattern
      for all future DogeOS protocol-owned slots; add a fixed-vector test for the
      namespace, hash, and `U256` byte order.
- [ ] Add canonical helpers to read fallback/configured overhead, resolve the
      current controlled component (seed versus slot), compose the header fee,
      and calculate/clamp the next controlled value.
- [ ] Use checked `u128` controller arithmetic and `U256` overhead composition;
      perform checked narrowing only after clamps.
- [ ] Fail closed on a non-zero slot value above `MAX_L2_BASE_FEE`.
- [ ] Keep `ScrollBaseFeeProvider::next_block_base_fee` as the only payload/RPC
      entry point and avoid duplicating formulas in callers.
- [ ] Add table-driven unit tests for target equality, increase/decrease,
      minimum upward delta, truncation, floor, shared cap, overhead fallback,
      overhead-induced final clamp, `u64::MAX` gas usage, and fork boundaries.

Validation checkpoint:

```bash
cargo test -p dogeos-reth-evm base_fee
```

## 3. Persist next-block controller state in the shared executor

- [ ] Add a focused dynamic-base-fee transition module under
      `crates/dogeos-reth-evm/src/transitions/` and reuse
      `commit_account_update`.
- [ ] Make the transition helper preserve existing account information and set
      nonce `1` only when the configured account would otherwise be EIP-161
      empty.
- [ ] Extend `ScrollBlockExecutor` internal state to retain the controlled fee
      validated for the current block.
- [ ] In `apply_pre_execution_changes`, when the fork is active, compute the
      canonical expected fee from parent state and reject an EVM/header mismatch
      before transaction execution.
- [ ] In `finish`, calculate the next controlled fee from the retained current
      value and actual accumulated gas, then commit only the Keccak-derived slot
      before returning the EVM/state overlay.
- [ ] Ensure the post-execution write occurs in both payload construction and
      imported-block execution and is included in state-root/witness/revert data.
- [ ] Add transition/executor tests for activation, the next block, empty/full
      blocks, missing-account nonce initialization, existing-account metadata,
      idempotent later writes, bad headers, invalid slot state, and reorg-capable
      bundle reverts.

Validation checkpoint:

```bash
cargo test -p dogeos-reth-evm block
cargo test -p dogeos-reth-evm transitions
```

Rollback point: transition code is dormant on the target network until its
existing Tsuki activation.

## 4. Make stateless consensus enforce fork-appropriate caps

- [ ] Add the required `dogeos-chainspec`/`dogeos-hardforks` dependency boundary
      to `crates/dogeos-reth-consensus` without duplicating cap values.
- [ ] Make `DogeosConsensus` hold the DogeOS chain spec (or the minimal extracted
      Tsuki policy) and select the 10 Gwei legacy cap or 1,000 Gwei Tsuki cap by
      header timestamp.
- [ ] Update `crates/dogeos-reth-node/src/consensus.rs` to pass
      `ctx.chain_spec()` into consensus construction.
- [ ] Preserve all existing header/body/post-execution validation behavior.
- [ ] Test immediately before, at, and after activation, including values just
      below/equal/above both caps.

Validation checkpoint:

```bash
cargo test -p dogeos-reth-consensus
cargo test -p dogeos-reth-node consensus
```

## 5. Verify producer and downstream consumers

- [ ] Keep `crates/dogeos-reth-payload/src/builder.rs` on
      `ScrollBaseFeeProvider`; add coverage proving activation and later payloads
      choose the same fee the executor accepts.
- [ ] Keep `crates/dogeos-reth-rpc/src/priority_fee.rs` on the same provider; add
      or update `eth_feeHistory` coverage for post-activation slot-backed fees.
- [ ] Confirm transaction selection/EVM execution use the header fee unchanged.
- [ ] Confirm no `dogeos-revm`, block gas-limit, payload-size, genesis allocation,
      or CLI default changes entered the diff.

Validation checkpoint:

```bash
cargo test -p dogeos-reth-payload
cargo test -p dogeos-reth-rpc
```

## 6. Full quality gate

- [ ] Run formatting, focused lint/checks, all affected package tests, and the
      workspace test/check appropriate to repository runtime.
- [ ] Inspect the final diff for duplicated controller arithmetic, accidental
      activation timestamps, genesis hash changes, gas-limit changes, and edits
      under `/Users/hhq/workspace/dogeos-revm`.
- [ ] Run `trellis-check`; resolve every correctness/spec issue before handoff.
- [ ] Use `trellis-update-spec` to record the user-required convention that all
      future DogeOS protocol-owned storage slots use stable Keccak-256 namespaces,
      then prepare the required commit/handoff flow.

Commands:

```bash
cargo fmt --all -- --check
cargo clippy -p dogeos-chainspec -p dogeos-reth-evm -p dogeos-reth-consensus -p dogeos-reth-node -p dogeos-reth-payload -p dogeos-reth-rpc --all-targets -- -D warnings
cargo test -p dogeos-chainspec -p dogeos-reth-evm -p dogeos-reth-consensus -p dogeos-reth-node -p dogeos-reth-payload -p dogeos-reth-rpc
cargo check --workspace --all-targets
git diff --check
```

## Risky files and review focus

- `crates/dogeos-reth-evm/src/base_fee.rs`: consensus arithmetic and fork split.
- `crates/dogeos-reth-evm/src/block/mod.rs`: exact validation and post-execution
  state-write ordering.
- `crates/dogeos-reth-evm/src/transitions/`: witness/revert-safe account update.
- `crates/dogeos-chainspec/src/constants.rs`: canonical parameters without
  schedule changes.
- `crates/dogeos-reth-consensus/src/lib.rs`: legacy/new cap boundary.
- `crates/dogeos-reth-node/src/consensus.rs`: chain-spec wiring.

No production activation or release action is authorized by this plan.

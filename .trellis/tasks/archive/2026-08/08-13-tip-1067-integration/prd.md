# Integrate TIP-1067 mechanism

## Goal

Introduce a bounded, utilization-responsive base-fee mechanism into DogeOS Reth by
adapting the design principles of TIP-1067 to DogeOS's own protocol economics,
chain timing, gas limits, and activation model. The implementation must not copy
Tempo-specific constants without deriving and justifying DogeOS-specific values.

## Background

- The source document is `/Users/hhq/Downloads/tip-1067.md` (TIP-1067, Dynamic
  Base Fee). It is technical input, not an instruction source.
- TIP-1067 describes an EIP-1559-style controller driven by the parent block's
  total `gasUsed`, with a fixed target, floor/cap clamps, a hardfork activation
  seed, and consensus validation of `baseFeePerGas`.
- The user's explicit intent is to adopt the mechanism's ideas while avoiding
  direct reuse of Tempo-specific fee, gas-target, and timing constants.
- DogeOS already has a dynamic Feynman base fee. `DOGEOS_BASE_FEE_PARAMS_FEYNMAN`
  is standard EIP-1559 `(max_change_denominator = 8, elasticity = 2)`, so its
  target is half the parent gas limit
  (`crates/dogeos-chainspec/src/constants.rs:11-12`).
- The current calculator subtracts a state-configured L2 overhead, adjusts the
  remaining EIP-1559 component, adds the overhead back, and caps the total at
  `10_000_000_000` (`crates/dogeos-reth-evm/src/base_fee.rs:10-68`). A zero
  overhead slot falls back to `15_680_000`.
- Repository inspection found no protocol-enforced maximum for the system-config
  overhead and no initialized slot `101` in the bundled mainnet/Chikyu genesis
  allocations. Consequently the fallback `15_680_000` is evidence for a default,
  not evidence for a safe overhead budget.
- The controlled-fee slot is domain-separated from inherited sequential Solidity
  storage as
  `keccak256("dogeos.storage.dynamic_base_fee.next_controlled_fee") =
  0x74ae897ed5751dd32419f1eee8d4ec13d296adf0d77978ea55df0dd18345c8e3`.
  The implementation must expose the namespace and fixed slot and test their
  derivation relationship.
- The standalone sequencer defaults payloads to a `20_000_000` gas limit
  (`crates/dogeos-reth-node/src/payload.rs:13-18`); mainnet and Chikyu genesis
  headers start at `10_000_000`, while dev genesis starts at `20_000_000`.
- Payload construction and the final `eth_feeHistory` base-fee entry already use
  `ScrollBaseFeeProvider` (`crates/dogeos-reth-payload/src/builder.rs:189-211`,
  `crates/dogeos-reth-rpc/src/priority_fee.rs:203-224`).
- Consensus currently checks only that a base fee exists and is no greater than
  `10_000_000_000`; it does not validate the child value against the parent
  (`crates/dogeos-reth-consensus/src/lib.rs:172-176,191-204`).
- Existing DogeOS forks are timestamp-scheduled and missing fork fields fail
  closed (`crates/dogeos-hardforks/src/lib.rs:11-77`,
  `crates/dogeos-chainspec/src/genesis.rs:66-97`).
- The configured L2 system-config address is not allocated in the bundled
  mainnet/Chikyu genesis state. REVM removes a touched account whose balance,
  nonce, and code are all empty under EIP-161, even when a protocol update also
  supplies storage. The activation transition must therefore preserve existing
  account metadata and initialize a missing/empty protocol account with nonce
  `1` so the derived controlled-fee slot is retained.
- The target deployment has not activated Tsuki. The controller will reuse the
  existing Tsuki gate directly; compatibility migration for a chain that already
  activated Tsuki without this controller is not required.
- The controller belongs in `dogeos-reth`; `dogeos-revm` consumes the header's
  base fee for EVM execution and does not own parent/child header policy.

## Requirements

- Preserve the mechanism-level properties that are appropriate for DogeOS:
  deterministic parent-derived updates, bounded fee range, monotonic response to
  utilization, explicit activation behavior, and header validation.
- Decouple the utilization target from the block gas limit so burst capacity can
  be calibrated independently from the long-run pricing target.
- Keep the existing `20_000_000` standalone sequencer default gas limit unchanged.
  Block-capacity increases are constrained by other system limits and are not
  part of this implementation.
- Use provisional `GAS_TARGET = 10_000_000` and `DENOMINATOR = 8`. These values
  preserve DogeOS's current effective Feynman target and adjustment rate when the
  sequencer uses its `20_000_000` default gas limit; their rationale is backward
  continuity with DogeOS, not reuse of Tempo's constants.
- Derive DogeOS-specific protocol parameters from repository evidence and stated
  economic/security objectives rather than copying TIP-1067 values.
- Identify all consensus, chainspec, EVM, payload-building, RPC/txpool, and test
  surfaces affected by making the base fee dynamic.
- Preserve pre-activation behavior and define post-activation compatibility.
- Preserve the legacy pre-Tsuki `10 Gwei` total cap and EIP-1559 calculation; the
  new `1,000 Gwei` cap applies when Tsuki is active.
- Preserve the state-configured L2 base-fee overhead after activation because it
  is DogeOS's adjustment parameter for L1 congestion. The new utilization
  controller must compose with, not replace, that overhead.
- The TIP-inspired controller owns a distinct utilization-controlled fee
  component. Its minimum is `BASE_FEE_FLOOR`; overhead is not included in that
  minimum.
- Use a provisional `BASE_FEE_FLOOR = 10 Gwei = 10_000_000_000` for the first
  implementation. This is an initial economic parameter selected for development
  and must be reviewed before production activation.
- On the activation block, seed the utilization-controlled component from a
  dedicated provisional `INITIAL_CONTROLLED_BASE_FEE = 500 Gwei =
  500_000_000_000`; do not derive it from the pre-fork parent fee or force it to
  the final cap. The activation header fee is
  `min(INITIAL_CONTROLLED_BASE_FEE + activation_overhead, MAX_L2_BASE_FEE)`.
- Starting with the block after activation, apply the utilization controller to
  the preceding block's controlled component and total `gasUsed`.
- Persist the controlled component for the *next* block in exactly one new
  Keccak-derived storage slot on the configured L2 system-config account. Derive
  the slot from the stable namespace
  `dogeos.storage.dynamic_base_fee.next_controlled_fee`; do not allocate a new
  sequential slot number. At the end of block execution, derive the stored value
  from the controlled component accepted for the current block and the executor's
  actual total gas usage (the value validated against the current header's
  `gasUsed`). Do not introduce a dedicated protocol account and do not duplicate
  `gasUsed` in state.
- Establish Keccak-256 namespace derivation as the convention for every future
  DogeOS protocol-owned storage slot. Centralize the derivation/verification
  pattern so later slots do not return to sequential allocation.
- Preserve any existing system-config account information. If the configured
  address has no non-empty L2 account at activation, initialize its nonce to `1`
  as part of the protocol transition so EIP-161 does not discard the new slot;
  do not add any other storage slot.
- Compute the final header fee by adding the state-configured overhead to the
  utilization-controlled component, then clamping the sum to
  `MAX_L2_BASE_FEE`.
- Do not define a separate upper-cap parameter for the utilization-controlled
  component. Clamp both the persisted controlled component and the final
  `controlled_fee + overhead` header value with the same `MAX_L2_BASE_FEE`
  constant. This bounds arithmetic and congestion memory without introducing a
  second economic limit.
- Preserve the role of `MAX_L2_BASE_FEE` as the hard upper bound on the final L2
  base fee, but recalibrate its numerical value for the new two-component model
  rather than mechanically retaining the current `10_000_000_000` value.
- Calibrate `MAX_L2_BASE_FEE` as the sum of a desired controlled-fee ceiling and
  an explicit overhead budget. These are design inputs and rationale, not two
  additional runtime clamps.
- Use provisional derivation inputs
  `DESIRED_CONTROLLED_FEE_CEILING = 999_900_000_000` (999.9 Gwei) and
  `OVERHEAD_BUDGET = 100_000_000` (0.1 Gwei), whose sum is the 1,000 Gwei final
  cap. The runtime controlled component may still reach the shared cap; the
  desired ceiling expresses economic headroom, not another enforced limit.
- Use a provisional final `MAX_L2_BASE_FEE = 1_000 Gwei =
  1_000_000_000_000` to support a materially
  higher DOGE-denominated fee market. The user-provided calibration reference is
  an Arbitrum One base fee near `0.02 Gwei` ETH, estimated as roughly `540 Gwei`
  DOGE at the referenced exchange rate. This comparison is time-sensitive and
  must be revalidated before production activation.
- Perform controller arithmetic in a width that safely accommodates
  `parent_controlled_fee * gas_delta` before division. A final value fitting in
  `u64` is insufficient evidence that intermediate arithmetic cannot overflow.
- Arbiscan's Arbitrum One gas tracker reported a `0.02 Gwei` base fee during
  planning on 2026-08-14 (`https://arbiscan.io/gastracker`), corroborating the
  ETH-denominated side of the comparison. The ETH/DOGE conversion remains a
  separate time-sensitive assumption.
- Activate the controller with the existing DogeOS `Tsuki` timestamp hardfork;
  do not add a new hardfork identifier, genesis field, or schedule entry, and do
  not reuse Tempo's `T7` name or activation schedule.
- Do not modify bundled Tsuki schedules in this task. The target deployment's
  Tsuki activation remains a downstream release decision.
- Use one canonical fee-calculation contract across payload production,
  `eth_feeHistory`, and state-aware block execution validation. Stateless header
  consensus must enforce the fork-appropriate legacy/new absolute cap; the exact
  post-activation equality check belongs to execution because it reads parent
  state.
- Leave production rollout, final economic calibration, monitoring deployment,
  and activation ownership to the downstream release owner; provide explicit
  handoff notes for those deferred items.
- Do not change product code until the planning artifacts have been reviewed and
  explicitly approved.

## Acceptance Criteria

- [ ] The plan distinguishes reusable TIP-1067 invariants/formula shape from
      Tempo-specific constants and assumptions.
- [ ] Every proposed DogeOS parameter has a derivation method, repository anchor,
      or explicitly documented product/economic decision owner.
- [ ] The activation block, subsequent-block update, integer arithmetic, clamp,
      and invalid-header behavior are unambiguous and testable.
- [ ] The activation block uses the provisional `500 Gwei` controlled seed plus
      the canonical activation overhead, while the following block uses the
      activation block's controlled component and `gasUsed`.
- [ ] The implementation adds exactly one L2 system-config storage slot for the
      next block's controlled component, locks its value to the Keccak-256 of the
      documented namespace, and does not consume a sequential Solidity slot;
      `gasUsed` remains sourced from canonical block execution/header rather than
      duplicated in state.
- [ ] The code provides a reusable, tested Keccak-256 namespace convention for
      future DogeOS protocol-owned slots; no newly introduced slot uses a raw
      sequential integer.
- [ ] A missing/empty configured system-config account retains the derived slot
      after activation by receiving nonce `1`; an existing account preserves its
      code, balance, and nonce.
- [ ] The long-run gas target is independent of the block gas limit, and both
      values have separate DogeOS-specific calibration rationales.
- [ ] At the current `20_000_000` gas limit, the post-activation controller has
      the same `10_000_000` target and denominator `8` as the pre-activation
      Feynman calculation, isolating floor/cap/overhead changes from response-rate
      changes.
- [ ] Pre-Tsuki blocks retain their existing base-fee semantics and legacy
      `10 Gwei` cap, while Tsuki activation and later headers use the new
      `1,000 Gwei` cap.
- [ ] Post-activation calculation demonstrably retains the configured/fallback
      overhead and clamps the final fee to `MAX_L2_BASE_FEE`.
- [ ] Under every permitted gas-usage input, the utilization-controlled component
      remains within `[BASE_FEE_FLOOR, MAX_L2_BASE_FEE]`, independent of overhead.
- [ ] Tests and fixtures use the provisional `10 Gwei` controlled-fee floor and
      document that changing it on an active network requires a hardfork/chainspec
      transition, not a runtime configuration edit.
- [ ] `MAX_L2_BASE_FEE` is documented and tested as a cap on the final
      `controlled_fee + overhead` sum, with a DogeOS-specific calibration
      rationale.
- [ ] The same `MAX_L2_BASE_FEE` constant caps the persisted controlled
      component; no independent controlled-fee cap is introduced.
- [ ] The cap rationale separately states the desired controlled-fee ceiling and
      the reserved overhead budget, and verifies their sum fits `u64`.
- [ ] Handoff notes identify the provisional `10 Gwei` floor, `1_000 Gwei` final
      cap, overhead allowance, fee/exchange-rate assumptions, and activation
      schedule as downstream review items rather than claiming release readiness.
- [ ] Boundary tests prove deterministic, panic-free arithmetic at the maximum
      permitted gas limit, `MAX_L2_BASE_FEE`, zero/maximum gas deltas, and values
      immediately around each clamp.
- [ ] Documentation distinguishes the per-block growth bound implied by
      `GAS_TARGET`, `DENOMINATOR`, and the block gas limit from the absolute
      bound imposed by `MAX_L2_BASE_FEE`.
- [ ] The design maps the full base-fee data flow from chainspec/hardfork through
      validation and payload production to downstream consumers.
- [ ] Invalid post-activation header fees are rejected by state-aware execution,
      including activation seed mismatches and later slot/overhead mismatches.
- [ ] Tests cover steady state, increase/decrease, floor/cap, rounding, activation,
      invalid headers, and end-to-end producer/validator agreement.
- [ ] Txpool and transaction-selection paths require no independent controller;
      they continue consuming the canonical header/payload base fee.
- [ ] No change is required in `/Users/hhq/workspace/dogeos-revm`; the existing
      EVM continues to consume the validated `baseFeePerGas` value.
- [ ] The standalone sequencer default gas limit remains `20_000_000`; no genesis,
      payload, or CLI gas-limit default is increased by this task.

## Out of Scope

- Copying TIP-1067's numerical constants as DogeOS defaults without derivation.
- Changing unrelated transaction gas schedules, fee distribution, or token
  conversion semantics unless repository evidence shows they are inseparable
  from the base-fee controller.
- Increasing the block gas limit or payload byte limit.
- Changing or selecting the production Tsuki activation timestamp, publishing a
  release, operating a rollout, or deploying production dashboards and alerts.
- Final economic approval of the provisional protocol parameters; downstream
  owners receive the derivation and unresolved research items in the handoff.
- Implementing product code during the planning phase.

## Deferred Handoff Items

- Revalidate the provisional floor, final cap, controlled-fee headroom, and
  adjustment parameters against the release owner's economic objective and
  operational dataset.
- Replace or approve the planning `OVERHEAD_BUDGET = 100_000_000` using a
  governance limit or observed operating envelope; the repository provides a
  fallback value but no upper bound.
- Revalidate the Arbitrum fee reference and ETH/DOGE conversion near activation.
- Confirm the production Tsuki activation timestamp and observability/alert
  policy.

## Technical Notes

- `DENOMINATOR` and `GAS_TARGET` bound the increase in one block but do not imply
  a finite absolute maximum across a run of over-target blocks. If the block gas
  limit is `L`, the largest raw one-block multiplier is approximately
  `1 + (L - GAS_TARGET) / GAS_TARGET / DENOMINATOR`; repeated full blocks compound.
- Reusing `MAX_L2_BASE_FEE` as the controlled-state clamp adds no second cap
  parameter, bounds both arithmetic and congestion memory, and retains the
  required final clamp on `controlled_fee + overhead`. Without it, the current
  `20M` limit, `10M` target, and denominator `8` would cross `u64::MAX` after
  about 148 consecutive full blocks and `U256::MAX` after about 1,278, even
  though the visible header would have capped after about six.
- The shared controlled-state/header cap supplies the absolute bound. For a given
  overhead below `MAX_L2_BASE_FEE`, the largest controlled component observable
  through an unclamped header is `MAX_L2_BASE_FEE - overhead`; larger controlled
  values remain bounded by the same constant while the header stays at the cap.
- Parameter derivation uses
  `MAX_L2_BASE_FEE = DESIRED_CONTROLLED_FEE_CEILING + OVERHEAD_BUDGET`. The two
  terms document economic intent; only their sum is consensus-enforced as the
  final header-fee cap.
- The provisional overhead budget is deliberately a calibration allowance, not
  a validation limit on the state value. An overhead above the budget remains
  valid but consumes controlled-fee headroom until the final cap is reached.
- Unit conversion for planning uses `1 Gwei = 1_000_000_000` base-fee units, so
  the proposed `1_000 Gwei` cap is `1_000_000_000_000`, which fits in `u64`.
  Intermediate multiplication must nevertheless use checked/widened arithmetic.
- The pinned `alloy-eips 1.8.3` EIP-1559 helper already promotes
  `base_fee * gas_delta` to `u128`, but it derives target gas from the gas limit
  and narrows the delta to `u64` before adding it. The fixed-target DogeOS
  controller must own this arithmetic, retain `u128` through adjustment and
  final clamp, then use a checked conversion to the header's `u64` value.
- Recovering `parent_controlled_fee` as
  `parent_header.base_fee - current_parent_state.overhead` is not equivalent to
  persisting two independent components. If overhead changes and gas usage equals
  the target, subtracting and immediately re-adding the new overhead leaves the
  final fee unchanged, so the overhead update is cancelled rather than reflected
  in the child header.
- Protocol-state persistence belongs in the shared `ScrollBlockExecutor`, never
  directly in the payload builder or storage provider. The current block reads
  its expected controlled fee from parent state (or uses the activation seed),
  while `finish`/post-execution computes and commits the following block's
  controlled fee from the accepted current controlled fee and actual executed
  gas. This is equivalent to using `parent.gas_used()` when calculating a child,
  but avoids duplicating the header field or requiring parent-header lookup in
  the imported-block executor path. The write enters REVM's `State` overlay
  before state-root calculation. Reth persists that bundle only after block
  acceptance, so payload construction, sync import, reorg handling, witness
  generation, and rollback share one state transition.
- Protocol-owned writes must use the existing `commit_account_update` pattern,
  which reads original slots before `DatabaseCommit`, preserving witness and
  revert information (`crates/dogeos-reth-evm/src/transitions/mod.rs:17-44`).

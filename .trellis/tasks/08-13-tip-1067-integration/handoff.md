# TIP-1067 integration handoff

## Implemented protocol behavior

- Pre-Tsuki blocks retain the inherited Feynman EIP-1559 calculation and 10
  Gwei final cap.
- Tsuki blocks read a separately persisted controlled component from L2
  system-config state, add the configured/fallback overhead, and clamp the
  header fee to 1,000 Gwei.
- The first Tsuki block uses a 500 Gwei controlled seed. Each executed Tsuki
  block calculates and persists the following controlled component using actual
  executor gas usage, a fixed 10M target, denominator 8, and a 10 Gwei floor.
- Imported blocks and locally built payloads share state-aware exact header-fee
  validation in `ScrollBlockExecutor`.
- Stateless consensus selects the 10/1,000 Gwei cap from the current header's
  Tsuki activation state.
- The new state uses
  `keccak256("dogeos.storage.dynamic_base_fee.next_controlled_fee")` =
  `0x74ae897ed5751dd32419f1eee8d4ec13d296adf0d77978ea55df0dd18345c8e3`.
- A reusable `derive_protocol_storage_slot` helper and EVM code-spec require all
  future DogeOS protocol-owned slots to use stable Keccak-256 namespaces.

## Provisional parameters requiring downstream review

| Parameter | Implemented value | Follow-up |
|---|---:|---|
| `BASE_FEE_FLOOR` | 10 Gwei | Economic review before activation |
| `INITIAL_CONTROLLED_BASE_FEE` | 500 Gwei | Economic review before activation |
| `MAX_L2_BASE_FEE` | 1,000 Gwei | Revalidate DOGE fee-market objective |
| `DESIRED_CONTROLLED_FEE_CEILING` | 999.9 Gwei | Calibration input only |
| `BASE_FEE_OVERHEAD_BUDGET` | 0.1 Gwei | Replace/approve using observed or governed overhead envelope |
| `DYNAMIC_BASE_FEE_GAS_TARGET` | 10,000,000 | Revalidate against operating data |
| `DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR` | 8 | Revalidate response rate |

The Arbitrum One reference of roughly 0.02 Gwei ETH / 540 Gwei DOGE was a
time-sensitive planning input and must be recalculated near activation.

## Activation and release ownership

- No hardfork identifier, genesis field, gas limit, or bundled activation
  schedule was changed.
- The controller reuses Tsuki. Bundled schedules still mark mainnet/dev Tsuki at
  timestamp 0 and Chikyu Tsuki as `Never`; the downstream owner must ensure the
  target deployment has the intended authoritative Tsuki timestamp.
- The target deployment was stated not to have activated Tsuki, so no
  already-active migration was implemented.
- Publishing, production activation, monitoring, alerting, and rollback
  operations remain downstream responsibilities.

## Verification completed

```text
cargo fmt --all -- --check
cargo clippy <affected packages> --all-targets -- -D warnings
cargo test <affected packages>
cargo check --workspace --all-targets
cargo check -p dogeos-reth-evm --no-default-features
git diff --check
```

Focused tests cover fixed-vector slot derivation, seed/later-block behavior,
increase/decrease/floor/cap arithmetic, overhead composition, invalid header and
slot rejection, pre-Tsuki compatibility, system-config nonce initialization,
metadata/unrelated-storage preservation, and REVM bundle/revert tracking.

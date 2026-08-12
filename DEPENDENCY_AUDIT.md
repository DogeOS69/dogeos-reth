# Phase 0 dependency audit

Status: **passed on 2026-08-06**.

The standalone spike resolves official Reth v2.0.0 plus the reviewed clean-Reth compatibility
stack as a source dependency and retains DogeOS EVM behavior through `revm-scroll`; it contains
no copied upstream Reth crate and no production dependency on the legacy `scroll-reth` fork.

| Component | Locked selection |
| --- | --- |
| Reth | official `v2.0.0` plus RocksDB, Header-transform, and Composite-RPC layers / `ae160090003d9b04be0521e9e4760558798cdf40` |
| REVM | `36.0.0` |
| `revm-scroll` | `dcf087684f255131c96c0d20f3291eef9198e990` (`dogeos`) |
| Alloy API family | `alloy-consensus 1.8.2` |
| alloy-evm | `0.30.0` |
| Alloy primitives / Solidity types | `1.5.7` |

The first `feat/drop-scroll-patch` candidate resolved REVM 34 and
`alloy-evm 0.27.3`, so it was rejected. The evaluation branch
`chore/upgrade-revm-v36` (revision `1b87ecf...`) was merged into the canonical
`dogeos` branch on 2026-08-12 (PRs #16/#17: drop the Scroll REVM fork, upgrade
to REVM 36); the workspace now selects `dogeos`, which resolves the same single
REVM 36 and single alloy-evm 0.30.0 instance. The audit requires both the
encoded branch source and the exact `dcf08768...` commit resolved in
`Cargo.lock`.

The Reth source manifest uses compatible semver ranges for a subset of the
Alloy 1.5 components. Cargo otherwise selected a newer incompatible parser
subtree. `Cargo.lock` therefore intentionally pins the `alloy-dyn-abi`,
`alloy-json-abi`, `alloy-sol-*`, and `alloy-primitives` group to `1.5.7`, the
same group recorded by the Reth v2.0.0 lockfile.

## Reproduction

```sh
scripts/verify-dependency-graph.sh
cargo check -p dogeos-reth-spike --locked --offline
cargo test -p dogeos-reth-spike --locked --offline
```

This validates the dependency family only. It is not evidence yet for any
Engine, EVM, storage, RPC, or networking capability in the matrix.

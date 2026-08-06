# Phase 0 dependency audit

Status: **passed on 2026-08-05**.

The standalone spike resolves official Reth v2.0.0 plus the reviewed RocksDB durability backport
as a source dependency and retains DogeOS EVM behavior through `revm-scroll`; it contains no
copied upstream Reth crate.

| Component | Locked selection |
| --- | --- |
| Reth | official `v2.0.0` plus RocksDB durability backport / `5235056be94c584edce6ba7900f163aaa9b8cda0` |
| REVM | `36.0.0` |
| `revm-scroll` | `1b87ecf17af029ac2f39e8ad362f3503ff2f4583` (`chore/upgrade-revm-v36`) |
| Alloy API family | `alloy-consensus 1.8.2` |
| alloy-evm | `0.30.0` |
| Alloy primitives / Solidity types | `1.5.7` |

The first `feat/drop-scroll-patch` candidate resolved REVM 34 and
`alloy-evm 0.27.3`, so it was rejected. The selected `chore/upgrade-revm-v36`
revision resolves a single REVM 36 and a single alloy-evm 0.30.0 instance.
The workspace intentionally selects that branch in `Cargo.toml`; the audit requires both the
encoded branch source and the exact `1b87ecf...` commit resolved in `Cargo.lock`.

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

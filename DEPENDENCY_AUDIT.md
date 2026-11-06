# Phase 0 dependency audit

Historical baseline: **passed on 2026-08-06; revalidated on 2026-08-13** at the
clean-Reth stack tip `972366a0bfc11cf6a0d5dc79d5e779cd81e32232`. The table below
records the current selection; the reproduction commands check the selected graph.

The standalone spike resolves official Reth v2.0.0 plus the reviewed clean-Reth compatibility
stack as a source dependency and retains DogeOS EVM behavior through `revm-scroll`; it contains
no copied upstream Reth crate and no production dependency on the legacy `scroll-reth` fork.

| Component | Locked selection |
| --- | --- |
| Reth | rev `b4a77efea79928b82712669ac71eee0e5f878db5` (`tsuki-v0.13` + upstream #28264 via DogeOS69/reth #22; tag pending) |
| REVM | `36.0.0` |
| `revm-scroll` | `tsuki-v0.13` / `516ff663b2fb2503dba8ef6b0d667f16334ac05a` |
| Alloy API family | `alloy-consensus 1.8.2` |
| alloy-evm | `0.30.0` |
| Alloy primitives / Solidity types | `1.5.7` |

The first `feat/drop-scroll-patch` candidate resolved REVM 34 and
`alloy-evm 0.27.3`, so it was rejected. The evaluation branch
`chore/upgrade-revm-v36` (revision `1b87ecf...`) was merged into the canonical
`dogeos` branch on 2026-08-12 (PRs #16/#17: drop the Scroll REVM fork, upgrade
to REVM 36). The workspace now pins both forks to `tsuki-v0.13`. The Reth tag
adds the P2P, RPC and reorg backports from DogeOS69/reth #11–19 and bounded session queues from #21; the
temporary Reth rev adds the upstream trie iterator seek-boundary fix (#28264) on top of that tag; the DogeOS REVM
tag adds the Veridise follow-ups from DogeOS69/dogeos-revm #23–28 and the BLOCKHASH optimization (#22) on top of
TsukiV2. It retains the upfront-balance fix and removal of the optional L1 fee
buffer. The graph still requires a single REVM 36 and alloy-evm 0.30.0 instance.
The audit checks both encoded tag sources and the exact commits above in
`Cargo.lock`; tags alone are not the source allowlist.

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

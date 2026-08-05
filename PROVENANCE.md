# Oracle provenance

This repository starts as a standalone migration workspace.  The current
`dogeos-reth` fork is the behavior oracle until a replacement has passed the
parity gates in `DOGEOS_RETH_MIGRATION_PLAN.md`.

## Pinned oracle

| Field | Value |
| --- | --- |
| Source repository | `https://github.com/DogeOS69/dogeos-reth.git` |
| Source revision | `6b62297a0a8a3d88c873a0fb2a11b52d2cc8824f` |
| Source commit time | `2026-07-26T22:17:50-07:00` |
| Source subject | `fix(scroll): address revm migration review` |
| `Cargo.lock` SHA-256 | `853f76864b35545c47e71261607d3e20fc0d2534f45a6b34ca063c3a0dc713c5` |

The source fork was derived from `https://github.com/scroll-tech/reth.git`.
That is provenance only: no generic Reth crate from that fork may be copied
into this repository.

## Frozen source inputs

The following files are the initial, byte-addressed source inputs for the
supported chain configurations.  `scripts/verify-oracle-baseline.sh` verifies
them against the pinned oracle checkout before an extraction changes them.

| Input | SHA-256 |
| --- | --- |
| `crates/scroll/chainspec/res/genesis/dogeos.json` | `87b23f048986196bdcffe74159b1bdf2924865196af6bda98cabcb4d2cd842da` |
| `crates/scroll/chainspec/res/genesis/chikyu_dogeos.json` | `c6effe795d7a5b000f07167ca1c97b6fa8c428acfdcf65641ee6bf9e4b32390b` |
| `crates/scroll/chainspec/res/genesis/dev.json` | `2e450321d7bf396ca9597d86e1cc5e603065e1ee78663f4b3b85d8265ba92619` |

The oracle checkout does not contain a fixture generator or a complete
differential replay corpus. This repository now freezes the initial byte-level
Engine V1 compatibility vectors under `fixtures/engine-v1/` and authenticates
them with `fixtures/SHA256SUMS`. The payload-ID vector is retained from the
legacy Scroll V1 compatibility implementation; the forced-L1-message vector is
from the local Engine qualification recorded in `ENGINE_V1_SMOKE.md`.
The old-to-new staged-sync comparison is frozen under `fixtures/sync/` and
documented in `SYNC_COMPATIBILITY_SMOKE.md`.

The staged-sync fixture records old-to-new dev oracle parity at block 300 for block, state,
transaction, and receipt roots plus selected RPC fields. It is not a substitute for a reproducible
oracle generator or Chikyu historical replay; broader transaction/receipt, reorg, RPC, and
execution-witness comparisons remain blocking qualification work.

## Verification

```sh
scripts/verify-oracle-baseline.sh /path/to/dogeos-reth
scripts/verify-fixtures.sh
```

The baseline command intentionally validates source identity and chain inputs
only. The fixture command validates JSON structure and the byte manifest.
Neither command treats Storage V1 bytes as a protocol oracle.

## Phase 0 dependency spike inputs

The standalone dependency spike uses upstream Reth `v2.0.0` plus the exact backport of upstream
PR #23603 at `83fde18d01ed0ef6b7bf501280116b4babc69bef` and evaluates the
`chore/upgrade-revm-v36` revision of `dogeos-revm` at
`1b87ecf17af029ac2f39e8ad362f3503ff2f4583`. These are immutable inputs; `Cargo.lock` is the
authoritative resolved graph once the spike succeeds. The backport provenance and removal
condition are recorded in `UPSTREAM_PATCHES.md`.

Reth v2.0.0 specifies the Alloy API crates at `1.8.2`, `alloy-evm` at
`0.30.0`, and its lower-level `alloy-primitives`/`alloy-sol-types` pair at
`1.5.x`. The Reth v2.0.0 lockfile resolves that lower-level group to `1.5.7`;
the spike pins the same group rather than incorrectly forcing
`alloy-primitives` to `1.8.2` or allowing independently upgraded parser
packages to drift out of compatibility.

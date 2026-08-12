# Upstream patch allowlist

A dependency or patch is permitted only after it is recorded here with an
upstream-generic rationale, a test, and a removal condition. Production Reth
crates must resolve from the clean upstream-lineage `DogeOS69/reth` repository;
the legacy, heavily modified `DogeOS69/scroll-reth` fork is not permitted as a
production dependency.

## Required investigation before selecting Reth 2

| Candidate | Status | Required evidence before admission |
| --- | --- | --- |
| Reth PR #23603 (RocksDB synchronous-write durability fix) | **Admitted; crash qualification pending** | Exact upstream backport is pinned and the static durability gate passes; crash/restart/reorg qualification remains required |
| Downloaded (inbound) Header transform hook | **Admitted temporarily** | Required only for the one-way Testnet geth-to-Reth crossover; the hook is optional, defaults to `None`, never transforms served headers, and is removed once the rollup path no longer canonicalizes downloaded peer headers |
| Composite RPC add-on handles (Reth PR #3) | **Retired; not selected** | rollup-node PR #17 adopted Reth's standard `RpcHandle`, so this layer and its Reth revision `ae160090...` are removed from the selected stack |

## Reth RocksDB synchronous-write durability

- Upstream PR: `reth-ethereum/reth#23603`
- Upstream source commit: `3a136fc8c38221e060cbc31ef5c5fa345cf0e17a`
- DogeOS backport commit: `90e08ba40` in `DogeOS69/reth`
- Stack base revision: `5235056be94c584edce6ba7900f163aaa9b8cda0`
- Pinned stack revision: `f851224ee9aaf21c76a14e844cbd12d9756f5f3b`
- Generic rationale: successful RocksDB transactions and batch commits must enable WAL sync so
  acknowledged writes survive a host crash. The patch contains no DogeOS protocol behavior and
  does not change storage encodings.
- Compatibility: the patch is an exact single-file backport onto official Reth v2.0.0. The
  dependency graph remains on REVM 36; the required `revm-scroll` branch is unchanged.
- Coverage: `cargo check -p reth-provider --lib`, `cargo check -p dogeos-reth-node --lib`, and the
  following locked-source audit pass:

```sh
scripts/audit-rocksdb-durability.sh
```

- Remaining qualification: add crash-during-write, restart, and reorg durability tests before a
  Storage V2 release is approved.
- Removal condition: move to an upstream Reth base containing PR #23603 or an equivalent audited
  synchronous-write implementation, then remove the backport while retaining the durability gate.

## Downloaded (inbound) Header transform hook

- Patch PR: `DogeOS69/reth#1`
- Layer revision: `f851224ee9aaf21c76a14e844cbd12d9756f5f3b`
- Generic rationale: expose one optional asynchronous transform hook for downloaded (inbound)
  headers through the network builder. Served/outbound response headers are never transformed. The
  rollup integration uses the hook only for the temporary one-way Testnet geth-to-Reth crossover,
  where downstream code performs legacy-signature removal/canonicalization on downloaded headers
  without embedding DogeOS consensus policy in Reth. The downstream adapter canonicalizes inbound
  headers; it does not restore signatures onto served headers.
- Coverage: the workspace build and tests exercise the opt-in builder API; passing `None` preserves
  ordinary Reth behavior. The standalone DogeOS node and Mainnet pass `None` and use no legacy
  transform.
- Removal condition: remove this layer after the Testnet crossover completes and rollup-node no
  longer canonicalizes downloaded peer headers.

## Composite RPC add-on handles — retired, not selected

- Patch PR: `DogeOS69/reth#3` (former revision `ae160090003d9b04be0521e9e4760558798cdf40`)
- Status: **not selected.** This layer is no longer part of the Reth stack. rollup-node PR #17
  adopted Reth's standard `RpcHandle`, so the composite add-on handle API is unnecessary and its
  Reth revision `ae160090...` has been removed from this workspace. Recorded here only to explain
  provenance; it must not be reintroduced.

## Required DogeOS EVM dependency

The standalone node retains the DogeOS-owned EVM dependency used by the
oracle:

```toml
revm-scroll = { git = "https://github.com/DogeOS69/dogeos-revm", branch = "dogeos", default-features = false }
```

`revm-scroll` is a required DogeOS dependency, not an upstream Reth patch. Its
protocol semantics remain in the DogeOS-owned `dogeos-revm` repository.  The
standalone workspace may depend on it, but its resolved commit must be pinned
in `Cargo.lock` and recorded here before the dependency spike is accepted;
the moving branch name alone is not a reproducible release input.

The oracle lockfile currently resolves this branch to
`8f754e800a1580d181ce001b13aa91c64a7254d9`.  The Reth 2 / REVM 36 spike must
either retain that revision with a compatible dependency graph or record the
replacement `dogeos-revm` revision and its parity evidence here.

The spike evaluated `chore/upgrade-revm-v36` revision
`1b87ecf17af029ac2f39e8ad362f3503ff2f4583`, observed on 2026-08-02. That
branch was merged into the canonical `dogeos` branch (PRs #16/#17) and deleted;
since 2026-08-12 `Cargo.toml` selects `dogeos` and `Cargo.lock` pins its
merge revision `dcf087684f255131c96c0d20f3291eef9198e990`. The dependency
audit still passes: the locked graph contains one REVM 36.0.0 and one
alloy-evm 0.30.0 instance. The exact audit and reproduction commands are in
`DEPENDENCY_AUDIT.md`.

This exception does not permit DogeOS-specific patches to upstream
REVM/Reth/Alloy.  Any such patch still requires a separate allowlist entry.

## Entry template

Add a section in this form before introducing any patch:

```md
## <package or patch>

- Upstream issue / PR: <URL>
- Pinned revision: `<full commit>`
- Generic rationale: <why this is not DogeOS protocol behavior>
- Coverage: <test command and scope>
- Removal condition: <upstream release or SDK hook>
```

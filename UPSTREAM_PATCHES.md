# Upstream patch allowlist

The allowlist is intentionally empty at migration start.  A dependency or
patch is permitted only after it is recorded here with an upstream-generic
rationale, a test, and a removal condition.

## Required investigation before selecting Reth 2

| Candidate | Status | Required evidence before admission |
| --- | --- | --- |
| Reth PR #23603 (RocksDB synchronous-write durability fix) | **Unselected** | Exact Reth 2 / REVM 36-compatible revision or a documented backport; dependency-tree proof that it does not advance to REVM 38; crash/restart test |

## Required DogeOS EVM dependency

The standalone node retains the DogeOS-owned EVM dependency used by the
oracle:

```toml
revm-scroll = { git = "https://github.com/DogeOS69/dogeos-revm", branch = "chore/upgrade-revm-v36", default-features = false }
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

The spike evaluates `chore/upgrade-revm-v36` revision
`1b87ecf17af029ac2f39e8ad362f3503ff2f4583`, observed on 2026-08-02. The branch
is selected by `Cargo.toml` and the exact revision is pinned by `Cargo.lock`.
The dependency spike passed: the locked graph contains
one REVM 36.0.0 and one alloy-evm 0.30.0 instance. The exact audit and
reproduction commands are in `DEPENDENCY_AUDIT.md`.

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

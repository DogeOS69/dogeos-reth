# Reth 2 extension-point capability matrix

This is the Phase 0 spike checklist.  “Unverified” means that no copied
upstream implementation may be used to satisfy the row.  The result must be
changed to **public hook**, **generic patch**, or **unsupported** by a minimal
standalone Reth 2 / REVM 36 spike.

The dependency-family gate is complete; see `DEPENDENCY_AUDIT.md`. All runtime
capabilities below remain unverified until exercised through public Reth APIs.

| Requirement | Oracle owner / evidence | Standalone DogeOS owner | Reth 2 status |
| --- | --- | --- | --- |
| DogeOS-owned `NodeTypes` composition boundary | New `crates/dogeos-reth-node` | `dogeos-reth-node` | Public hook (types only) |
| Custom transaction and receipt encodings | `crates/scroll/primitives`, `crates/scroll/alloy/consensus` | `dogeos-protocol-types` + `dogeos-reth-primitives` | Public traits; canonical protocol tests and Reth compact round trips pass |
| Feynman and Tsuki fork policy | `crates/scroll/alloy/hardforks`, `crates/scroll/hardforks` | `dogeos-hardforks` | Public hook; unit-tested |
| DogeOS mainnet, Chikyu, and dev chainspecs | `crates/scroll/chainspec/{dogeos,chikyu,dev}.rs` | `dogeos-chainspec` | Public `ChainSpec`/`EthChainSpec` hooks; Feynman+ schedules and Chikyu hash unit-tested |
| Native DOGE and Tsuki state transition | `crates/scroll/alloy/evm/src/block/tsuki.rs`, `revm-scroll` | `dogeos-reth-evm` + `revm-scroll` | Public REVM state API; insert/no-overwrite/idempotence tests pass |
| L1 fee and stateful base-fee policy | `crates/scroll/{evm,consensus,txpool}` | `dogeos-reth-evm`, `dogeos-reth-node` | Transaction-env and canonical zstd compression paths unit-tested; executor policy pending |
| Engine types, payload attributes, build and validation | `crates/scroll/{engine-primitives,payload,node}` | `dogeos-reth-engine` | Unverified |
| Equal-timestamp validation and forced transaction ordering | `crates/scroll/{consensus,payload}` | `dogeos-reth-engine` | Unverified |
| Storage V2 body reconstruction (`ommers=[]`, `withdrawals=None`) | Current code is coupled to fork storage crates | `dogeos-reth-primitives` | Unverified |
| RPC receipt conversion and `debug_executionWitness` | `crates/scroll/rpc`, `crates/scroll/alloy/rpc-types` | `dogeos-reth-rpc` | Unverified |
| Txpool fee policy and `noTxPool` | `crates/scroll/txpool`, `crates/scroll/node` | `dogeos-reth-node` | Unverified |
| Block import/listening and `scroll-wire` signed blocks | `crates/scroll/node` plus network integration | `dogeos-reth-node` | Unverified |
| Fresh V2 replay and abrupt-stop recovery | No standalone implementation | `dogeos-reth-node` | Unverified |

## Spike acceptance criteria

For each row, record the public trait/module used, a minimal compile target,
and the associated test.  A “generic patch” entry must be added to
`UPSTREAM_PATCHES.md` before it can be used.  An “unsupported” result blocks
extraction until an upstream-generic extension is proposed.

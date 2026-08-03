# Reth 2 extension-point capability matrix

This is the Phase 0 spike checklist. “Unverified” means that no copied
upstream implementation may be used to satisfy the row.  The result must be
changed to **public hook**, **generic patch**, or **unsupported** by a minimal
standalone Reth 2 / REVM 36 spike.

The dependency-family gate is complete; see `DEPENDENCY_AUDIT.md`. Verified runtime
capabilities below are compiled and unit-tested through public Reth APIs.

| Requirement | Oracle owner / evidence | Standalone DogeOS owner | Reth 2 status |
| --- | --- | --- | --- |
| DogeOS-owned `NodeTypes` composition boundary | New `crates/dogeos-reth-node` | `dogeos-reth-node` | Public hooks; DogeOS primitives, chainspec, Engine payload types, transaction-bound storage, component graph, `eth_` RPC builder, and Engine/RPC add-on graph are wired and type-tested |
| Custom transaction and receipt encodings | `crates/scroll/primitives`, `crates/scroll/alloy/consensus` | `dogeos-protocol-types` + `dogeos-reth-primitives` | Public traits; canonical protocol tests and Reth compact round trips pass |
| Feynman and Tsuki fork policy | `crates/scroll/alloy/hardforks`, `crates/scroll/hardforks` | `dogeos-hardforks` | Public hook; unit-tested |
| DogeOS mainnet, Chikyu, and dev chainspecs | `crates/scroll/chainspec/{dogeos,chikyu,dev}.rs` | `dogeos-chainspec` | Public `ChainSpec`/`EthChainSpec` hooks; Feynman+ schedules and Chikyu hash unit-tested |
| Native DOGE and Tsuki state transition | `crates/scroll/alloy/evm/src/block/tsuki.rs`, `revm-scroll` | `dogeos-reth-evm` + `revm-scroll` | Public REVM state API; insert/no-overwrite/idempotence tests pass |
| L1 fee and stateful base-fee policy | `crates/scroll/{evm,consensus,txpool}` | `dogeos-reth-evm`, `dogeos-reth-txpool`, `dogeos-reth-node` | Public `BlockExecutorFactory`/`ConfigureEvm` and transaction-validator hooks; L1 fee receipts, canonical compression, state-aware payload base fee, Tsuki fee caps, balance buffering, transitions, and EIP-2935 calls are verified |
| Feynman+ block execution and assembly | `crates/scroll/{alloy/evm,evm}` | `dogeos-reth-evm` | Public Alloy EVM `StateDB`, executor-factory, Reth `ConfigureEvm`, and `BlockAssembler` hooks; workspace tests pass |
| Engine types, payload attributes, build and validation | `crates/scroll/{engine-primitives,payload,node}` | `dogeos-reth-engine`, `dogeos-reth-payload` | Public `EngineTypes`/`PayloadTypes`/`BuiltPayload`/`PayloadBuilder` hooks; attributes, payload IDs, forced/pool execution, execution cache, sparse-trie handoff, block hints, built payloads, and Engine validation compile and test |
| Equal-timestamp validation and forced transaction ordering | `crates/scroll/{consensus,payload}` | `dogeos-reth-consensus`, `dogeos-reth-engine`, `dogeos-reth-payload` | Forced transactions decode once, retain order/bytes, reject trailing bytes, execute before pool transactions, and freeze `noTxPool` payloads; consensus and Engine validators both allow equal timestamps |
| Storage V2 body reconstruction (`ommers=[]`, `withdrawals=None`) | Current Scroll node aliases `EthStorage<ScrollTransactionSigned>` | `dogeos-reth-node` | Public generic `EthStorage<T>` hook retained without copied Reth code; Feynman+ chainspec yields post-merge empty ommers and pre-Shanghai absent withdrawals |
| RPC receipt conversion and `debug_executionWitness` | `crates/scroll/rpc`, `crates/scroll/alloy/rpc-types` | `dogeos-rpc-types` + `dogeos-reth-rpc` + `dogeos-reth-node` | Scroll network/request/transaction/receipt schemas, signing, `l1Fee`, receipt metadata, sender recovery context, receipt-derived transaction metadata, simulation and REVM call-environment conversion are assembled on public Reth hooks; the node `eth_` builder, equal-timestamp pending environment, HTTP/WS sequencer client, and exact raw-transaction encoding are verified; asynchronous forwarding policy and `debug_executionWitness` remain pending |
| Txpool fee policy and `noTxPool` | `crates/scroll/txpool`, `crates/scroll/node` | `dogeos-reth-txpool`, `dogeos-reth-payload`, `dogeos-reth-node` | Custom pooled transaction, exact encoded-byte cache, L1 fee validation, blob/L1-message rejection, maintenance tasks, pool bypass, resource limits, and deterministic `noTxPool` freeze use public hooks and compile together |
| Block import/listening and `scroll-wire` signed blocks | `crates/scroll/node` plus `dogeos-rollup-node/crates/scroll-wire` | `dogeos-reth-node` plus shared `scroll-wire` | Public `NetworkBuilder` and `NetworkConfigBuilder::add_rlpx_sub_protocol` hooks verified; canonical eth-wire network compiles. Shared `scroll-wire` attachment and signed-block import tests remain pending |
| Fresh V2 replay and abrupt-stop recovery | No standalone implementation | `dogeos-reth-node` | Unverified |

## Spike acceptance criteria

For each row, record the public trait/module used, a minimal compile target,
and the associated test.  A “generic patch” entry must be added to
`UPSTREAM_PATCHES.md` before it can be used.  An “unsupported” result blocks
extraction until an upstream-generic extension is proposed.

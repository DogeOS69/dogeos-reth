# Old-to-new P2P synchronization smoke

This records the isolated local compatibility run performed on 2026-08-03
between the pinned oracle node and the standalone migration node.

## Nodes

| Role | Binary | Revision | Storage |
| --- | --- | --- | --- |
| Producer/oracle | `scroll-reth 1.11.1-dev` | `6b62297a0a8a3d88c873a0fb2a11b52d2cc8824f` | V1 |
| Syncing node | `dogeos-reth 2.0.0-dev` | upstream Reth `eb4c15e5e36d8776d46629beae4c0a69af7ab04f` plus this workspace | V2 |

Both nodes used fresh isolated `dev` datadirs and deterministic local P2P
keys. They negotiated `eth/69` over loopback. No existing datadir or external
testnet was used.

## Synchronization

The old node produced one-second dev blocks. Its embedded fake consensus client
does not transmit forkchoice state to the new execution node, so merely peering
the nodes correctly left the new head at genesis. Reth's testing-only
`--debug.tip` was then used to supply fixed canonical targets without bypassing
the P2P downloader or execution pipeline.

1. Fresh V2 sync from block 0 to old-node block 146 downloaded 146 headers and
   bodies, executed all blocks, and completed all 13 pipeline stages.
2. After a clean V2 restart, every stage reopened at checkpoint 146.
3. A real EIP-1559 transfer was mined by the old node at block 300.
4. Incremental sync from 146 to 300 downloaded 154 headers and bodies,
   recovered the sender, executed the transaction, updated RocksDB history
   indices, and completed all stages.
5. Another clean restart reopened the V2 database at checkpoint 300.

## Parity result

At block 300 both nodes returned identical:

- block hash `0x24a2343516dc4b9a9ac41e819f5f7daf6ea1300ffd72adf7bf4ad15f7839ebce`;
- state, transaction, and receipt roots;
- transaction hash and RPC transaction fields;
- successful receipt, 21,000 gas, effective gas price, and `l1Fee`;
- recipient balance after the transfer.

The exact compared values are frozen in
`fixtures/sync/dev-p2p-block-300.json` and covered by the fixture SHA-256
manifest.

## Boundary

This proves old-to-new `eth/69` header/body transfer, initial and incremental
staged execution, transaction/receipt compatibility, V1-to-V2 semantic parity,
RocksDB index construction, and clean V2 reopen for this dev range. It does not
prove automatic live following because execution peers do not propagate the
rollup forkchoice decision. A rollup/consensus driver must send the new node
Engine forkchoice updates in production.

It also does not replace Chikyu replay, forced-L1-message replay over a
historical range, reorg, crash-during-sync, snapshot restore, or sustained-load
qualification.

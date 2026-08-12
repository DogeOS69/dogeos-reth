# Chikyu hardfork schedule

This document freezes the consensus inputs currently established for the preserved Chikyu testnet.
The chain will not reset, so unknown activation timestamps must never be inferred from a genesis
file or from another DogeOS network.

## Implemented schedule

| Fork | Condition | Evidence |
| --- | --- | --- |
| Feynman | `Timestamp(0)` | The published genesis allocation already contains the Feynman gas-price-oracle bytecode and storage marker `0xb = 1`. |
| Galileo | `Never` | No authoritative activation timestamp is published; the network remains on Feynman execution at the frozen checkpoint. |
| Galileo V2 | `Never` | Gas-price-oracle marker `0xc` is zero at genesis and checkpoint `0x676c32`; its code is still the genesis Feynman bytecode. |
| Tsuki | `Never` | Chikyu predates Tsuki, no activation timestamp is published, and the NativeDogeToken predeploy has no code at genesis or checkpoint `0x676c32`. |

`Never` is a fail-closed representation of the currently established schedule, not a declaration
that a fork can never be activated. Before a future activation, replace it with the operator-owned
timestamp, add boundary fixtures, and qualify replay through that boundary.

**Operational update (2026-08-12):** the table above is the frozen 2026-08-04 snapshot.
Operations report that Chikyu has since activated Tsuki directly from Feynman (no Galileo stage)
and that a full old-peer sync crossed the boundary. The built-in schedule intentionally remains
fail-closed until the operator-owned activation timestamp is published, encoded, and qualified per
the procedure above; until then `--chain dogeos-chikyu` cannot follow the live network past the
activation. This is tracked as a release blocker in `TODO.md`.

The checked-in Chikyu genesis no longer contains the incorrect `tsukiTime: 0` field. This does not
change the genesis block hash because chain configuration fields are not part of the genesis header.
Built-in Chikyu, mainnet, and dev specifications each supply their own schedule. Custom genesis
construction separately parses `feynmanTime`, `galileoTime`, `galileoV2Time`, and `tsukiTime` and
treats every omitted field as `Never`.

## Frozen public checkpoint

The following observations were made from `https://rpc.testnet.dogeos.com` on 2026-08-04 and are
recorded structurally in `fixtures/chikyu/hardfork-schedule.json`:

- chain ID `0x5fdaf3`;
- genesis hash `0xf9f7c524dce38b51a4d28ec2f18680773e5ba9d3f5f430d0e05f92cfeb65b1bc`;
- block `0x676c32` (6,777,906), hash
  `0x2b499525a2b051242025c89bf4c0287e56ed26f52dc38ba5972751671fa6d86d`, timestamp
  `0x6a72906a` (1,785,892,970);
- `eth_getStorageAt(0x5300...0002, 0xc, 0x676c32) = 0x00`;
- `eth_getCode(0x5300...d09e, 0x676c32) = 0x`.

The public endpoint does not expose `admin_nodeInfo` or `debug_chainConfig`, so it cannot provide a
future activation timestamp. The l2geth launch/deployment configuration and the rollup operator are
the authority for changing this schedule.

## Remaining qualification

This correction prevents retroactive Galileo/Tsuki execution but does not replace full historical
qualification. A release candidate still needs a fresh Storage V2 replay from genesis through the
frozen checkpoint, root and RPC parity against l2geth/live Chikyu, restart/reorg coverage, and real
pre-/at-/post-boundary fixtures once a later fork is scheduled.

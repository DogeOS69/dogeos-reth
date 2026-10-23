# Chikyu hardfork schedule

The built-in Chikyu specification is derived from
`crates/dogeos-chainspec/res/genesis/chikyu_dogeos.json`, supplied by Akase as
`genesis.json(dogeos testnet)` on 2026-10-20. The checked-in document preserves that JSON with
normalized formatting. It supplies both the L1 configuration and the fork activation timestamps.
Loading the document as a custom chain and selecting `--chain dogeos-chikyu` produce the same spec.

## Implemented schedule

| Fork | Condition | Evidence |
| --- | --- | --- |
| Feynman | `Timestamp(0)` | `config.feynmanTime` in the operator-provided document. |
| Galileo | `Timestamp(1790030800)` | `config.galileoTime` in the same document. |
| Galileo V2 | `Timestamp(1790030800)` | `config.galileoV2Time` in the same document. |
| Tsuki | `Timestamp(1790030900)` | `config.tsukiTime` in the same document. |

This replaces the earlier `Never` placeholders and the unqualified operational report of a direct
Feynman-to-Tsuki transition. The public checkpoint below predates these timestamps and continues
to select Feynman. Tests cover the checkpoint, each activation boundary, and parity between the
built-in spec, custom-genesis parser, builder, and public hardfork/configuration helpers.
Custom genesis construction still treats omitted fork timestamps as `Never`.

The genesis header fields and allocation are unchanged. The actual computed header hash is
`0xf9f7c524dce38b51a4d28ec2f18680773e5ba9d3f5f430d0e05f92cfeb65b1bc`; the former built-in
`0x93146785...` value was an incorrect pre-sealed hash (Veridise 9991). Construction now computes
the hash from the document, and regression tests compare it with the published value. The L1
chain ID is `111111`, matching the document instead of the former hard-coded Goerli value `5`.

## Frozen public checkpoint

The following observations were made from `https://rpc.testnet.dogeos.com` on 2026-08-04 and are
recorded structurally in `fixtures/chikyu/hardfork-schedule.json`. Its checkpoint observations are
preserved; its schedule is updated from the operator-provided document:

- chain ID `0x5fdaf3`;
- genesis hash `0xf9f7c524dce38b51a4d28ec2f18680773e5ba9d3f5f430d0e05f92cfeb65b1bc`;
- block `0x676c32` (6,777,906), hash
  `0x2b499525a2b051242025c89bf4c0287e56ed26f52dc38ba5972751671fa6d86d`, timestamp
  `0x6a72906a` (1,785,892,970);
- `eth_getStorageAt(0x5300...0002, 0xc, 0x676c32) = 0x00`;
- `eth_getCode(0x5300...d09e, 0x676c32) = 0x`.

The public endpoint did not expose `admin_nodeInfo` or `debug_chainConfig`, so the checkpoint alone
did not establish later activation timestamps. The supplied network configuration is the source
for the schedule above.

## Remaining qualification

The corrected identity and schedule do not replace full historical qualification. A release
candidate still needs a fresh Storage V2 replay through the checkpoint and scheduled transitions,
root and RPC parity against l2geth/live Chikyu, restart/reorg coverage, and real pre-/at-/post-boundary
block fixtures. Unit tests establish configuration and EVM-selection parity, not a full replay.

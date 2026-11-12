# Fee parameter activation

The EIP-1559 fee parameters and the base-fee cap switch at the TsukiV2 hardfork. Set
`tsukiV2Time` in the genesis JSON's `config`, alongside `tsukiTime`:

```json
{
  "config": {
    "tsukiTime": 0,
    "tsukiV2Time": 0
  }
}
```

This is a configuration excerpt; retain the other fields of the network's genesis.

| | Before TsukiV2 | From TsukiV2 |
| --- | --- | --- |
| EIP-1559 parameters | denominator 8, elasticity 2 | denominator 48, elasticity 10 |
| Base-fee formula cap | 10 gwei | 420,000 gwei |

Both are keyed on the child block's timestamp, including the boundary block.

- `tsukiV2Time` is required whenever `tsukiTime` is set, must not be earlier than
  `tsukiTime`, and is rejected without it. There is no fallback to `tsukiTime`.
- Like the other DogeOS timestamps, numeric, decimal-string, and `0x` hexadecimal-string
  forms are accepted. Invalid values reject the configuration.
- The retired `tsukiFeeParamsTime` key is rejected in any form, including `null`.

Mainnet and dev activate TsukiV2 at genesis. Chikyu keeps D8/E2 and the 10 gwei cap for its
Tsuki history; its built-in `tsukiV2Time` is the placeholder `1798761600`
(2027-01-01T00:00:00Z), and ops sets the real time before rollout. For any network with Tsuki
already active, choose a coordinated future timestamp: all sequencers, followers, and RPC nodes
must use the same value. The key does not change the genesis block hash.

The schedule is shared by payload construction, the import base-fee check, pool fee prediction,
and fee RPCs. TsukiV2 also switches execution rules in `revm-scroll` (`ScrollSpecId::TSUKI_V2`).
An activation after Tsuki and after genesis participates in the fork-ID schedule, so
un-upgraded peers disconnect at the activation. The header validator's 420,000 gwei upper bound
is not gated. Before TsukiV2 the formula clamps every base fee, overhead included, to 10 gwei.

At the first TsukiV2 block, an L2MessageQueue (`0x5300000000000000000000000000000000000000`) that
still runs the legacy Merkle-tree code of the Chikyu genesis gets the hash-chain queue code of the
mainnet genesis; its storage is kept, so the legacy withdrawal root seeds the hash chain. Any other
code is left alone, so mainnet is unaffected. On a network with the legacy queue, TsukiV2 is
therefore also the withdrawal-queue cutover: drain withdrawals before it, and move the bridge,
the provers and the L1 withdrawal verification to the hash chain at the same time.

The swap checks nothing but the code hash. The hash chain is seeded by whatever the queue holds
at the parent of the first TsukiV2 block (root in slot 0, next index in slot 1). Before
activation, with withdrawals drained, an operator must confirm with `eth_getCode` that the queue
has the legacy code (`0xdc3a5ee2…d50c`) and that slots 0 and 1 match the frozen snapshot the
converted bridge was built from, then re-check both slots at the parent of the first TsukiV2
block.

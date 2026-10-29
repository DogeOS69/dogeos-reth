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

# Fee parameter activation

Set `tsukiFeeParamsTime` in the genesis JSON's `config`, alongside `tsukiTime`:

```json
{
  "config": {
    "tsukiTime": 0,
    "tsukiFeeParamsTime": null
  }
}
```

This is a configuration excerpt; retain the other fields of the network's genesis.

Two rules follow this key, both keyed on the child block's timestamp, including the boundary
block:

| | Before activation | From activation |
| --- | --- | --- |
| EIP-1559 parameters | denominator 8, elasticity 2 | denominator 48, elasticity 10 |
| Base-fee formula cap | 10 gwei | 420,000 gwei |

- Omitted or `null`: activate both at the chain spec's Tsuki fork. Mainnet omits the key, so
  both apply from genesis.
- An explicit unsigned Unix timestamp, in seconds: use the earlier rules before that timestamp
  and the later rules from it. An explicit `0` means activation from timestamp zero, not the
  fallback.
- Like the other DogeOS timestamps, numeric, decimal-string, and `0x` hexadecimal-string
  forms are accepted. Invalid values reject the configuration instead of falling back.

Chikyu's Tsuki history was produced with D8/E2 and the 10 gwei cap, so its built-in genesis sets
`tsukiFeeParamsTime` to the placeholder `1798761600` (2027-01-01T00:00:00Z,
`DOGEOS_CHIKYU_TSUKI_FEE_PARAMS_TIME`); ops sets the real time before rollout. For any existing
network with Tsuki already active, choose a coordinated future timestamp to preserve its history
on replay. All sequencers, followers, and RPC nodes must use the same value. Leaving the field
omitted or null retains activation at Tsuki and does not protect that history from
reinterpretation. The setting is independent of Tsuki's execution transition; changing it does
not move `tsukiTime` or change the genesis block hash.

The schedule is shared by payload construction, the import base-fee check, pool fee prediction,
and fee RPCs. Fee predictors evaluate it at `parent.timestamp + 1`. An activation after Tsuki and
after genesis participates in the existing fork-ID schedule, so un-upgraded peers disconnect at
the activation. The header validator's 420,000 gwei upper bound does not follow the key.

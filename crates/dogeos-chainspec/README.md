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

- Omitted or `null`: activate D48/E10 at the chain spec's Tsuki fork.
- An explicit unsigned Unix timestamp, in seconds: use D8/E2 before that timestamp and
  D48/E10 from that timestamp, including the boundary block. An explicit `0` means
  activation from timestamp zero, not the fallback.
- Like the other DogeOS timestamps, numeric, decimal-string, and `0x` hexadecimal-string
  forms are accepted. Invalid values reject the configuration instead of falling back.

For an existing dev or test network with Tsuki already active, choose a coordinated
future timestamp to preserve its historical D8/E2 fees during replay. All sequencers,
followers, and RPC nodes must use the same value. Leaving this field omitted or null
retains activation at Tsuki and does not protect that history from reinterpretation.
The setting controls the fee parameters independently of Tsuki's execution transition;
changing it does not move `tsukiTime` or change the genesis block hash.

The schedule is shared by payload construction, pool fee prediction, and fee RPCs.
An activation after Tsuki and after genesis participates in the existing fork-ID schedule.
The 420,000 gwei cap and the on-chain overhead retain their separate rollout requirements:
upgrade validating nodes before increasing the overhead beyond the old 10 gwei cap.

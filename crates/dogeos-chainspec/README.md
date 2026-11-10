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

# Withdrawal queue upgrade

The withdrawal queue code swap is part of TsukiV2, using the same `tsukiV2Time`
as the fee and EVM rules above. At the first executed block whose timestamp is at
or after activation, the executor replaces the legacy Merkle-tree L2MessageQueue
at `0x5300000000000000000000000000000000000000` with the hash-chain queue.
There is no separate block-number schedule or checkpoint setting.

Like the Feynman oracle transition, the transition is called on every active block
and uses on-chain state to make the change idempotent. Only the exact legacy code
hash from the Chikyu genesis (`0xdc3a5ee2e8f8bcfd3a6ae7d2b5f38c8bf5c77b9a3fad31aa5dfe1ffed7aad50c`)
is replaced. The hash-chain queue, other code, and a missing account are left alone.
Mainnet already has the hash-chain code in genesis. If TsukiV2 is active at genesis,
the transition runs on the first executed block; genesis itself is not executed.

Only code changes: balance, nonce, withdrawal root (slot 0), next index (slot 1),
owner, messenger, and all other storage are preserved. The old root seeds the new
hash chain. Subsequent blocks keep the new code and the evolving queue state.

Before scheduling activation on an existing network, operators must verify the
live queue's legacy code hash and complete the bridge drain and checkpoint
conversion for that same cutover. Root and index are taken from the parent state;
they are no longer checked against values supplied in genesis. Activating this
rule retroactively on a chain that ran TsukiV2 with the legacy queue changes its
historical state roots.

Remove `config.scroll.withdrawalQueueUpgrade` from configurations made for the old
block-based mechanism: it is rejected as an unknown field, including `null`.
Sequencers, followers, RPC nodes, and proving programs must agree on the TsukiV2
schedule and execution code. Update the stateless verifier/prover pins with the
upgrade-support release and validate a proof across the cutover before rollout.

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
or after activation, the executor checks the frozen checkpoint and replaces the
legacy L2MessageQueue at `0x5300000000000000000000000000000000000000` with the
hash-chain queue. There is no separate block-number schedule. If TsukiV2 is active
at genesis, this happens in the first executed block; genesis itself is not executed.

A network migrating from the legacy queue must set `withdrawalQueueCheckpoint`
under `config.scroll`, with all three values taken from the authenticated frozen
checkpoint used to build the converted bridge snapshot:

```json
{
  "config": {
    "scroll": {
      "withdrawalQueueCheckpoint": {
        "legacyCodeHash": "0x…",
        "seedRoot": "0x…",
        "nextMessageIndex": 1173
      }
    }
  }
}
```

This is a configuration excerpt; retain the other `scroll` fields. The hashes above
are placeholders and the count is illustrative, not a production checkpoint.
All three fields are required when the checkpoint is supplied; unknown fields,
including `block`, are rejected. `nextMessageIndex` is a JSON unsigned integer.

At migration, the account's code hash, withdrawal root (slot 0), and next index
(slot 1) must match the checkpoint. A mismatch rejects the block before replacing
the code. No withdrawal may be appended between the frozen checkpoint and the
parent of the first TsukiV2 block. A withdrawal in the activation block executes
with the new code, after the checkpoint check and swap.

Only code changes: balance, nonce, root, next index, owner, messenger, and all other
storage are preserved. The old root seeds the new hash chain. Already-upgraded
queues are a no-op without rereading or checking checkpoint storage, so subsequent
withdrawals can advance the root and index normally. Mainnet already has the
hash-chain code in genesis and does not need a checkpoint.

With a configured checkpoint, missing accounts and unexpected code hashes reject
the block. Without one, unrelated code and missing accounts remain unchanged, but
the known Chikyu legacy code hash
(`0xdc3a5ee2e8f8bcfd3a6ae7d2b5f38c8bf5c77b9a3fad31aa5dfe1ffed7aad50c`)
is rejected at TsukiV2: omitting the checkpoint cannot bypass its validation.
The built-in Chikyu configuration has no checkpoint; operators must supply the
actual frozen values before migrating its legacy queue.

Publish one set of checkpoint values for the network, generated from the same
capture as the converted bridge snapshot. Sequencers, followers, RPC nodes, and
stateless verifier/prover builds must use those identical values and the same
TsukiV2 schedule and execution code. These are not independent operator choices.
The checkpoint setting does not change the genesis block hash or fork ID.

Migrate old `config.scroll.withdrawalQueueUpgrade` configurations to
`withdrawalQueueCheckpoint`, retaining the code hash, root and index and removing
`block`. The old key is rejected, including `null`. Align bridge drain and snapshot
conversion with TsukiV2 activation; activating this rule retroactively on a chain
that ran TsukiV2 with the legacy queue changes historical state roots.
Update stateless verifier/prover pins and validate a real proof across the cutover
before rollout.

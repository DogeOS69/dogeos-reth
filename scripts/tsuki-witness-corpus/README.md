# Base-fee import and witness check

```bash
scripts/tsuki-witness-corpus/generate.sh [OUT_DIR]
```

Requires Cargo, Foundry (`cast`), `jq`, and Python 3. The script builds the current source,
starts a local node, then replays the payloads into a fresh node. Both are stopped on exit. `CARGO_TARGET_DIR` selects the shared build cache;
output defaults to `target/tsuki-witness-corpus`.

The Engine API drives seven blocks and checks:

- A rehashed payload with a base fee one wei too high returns `INVALID` for the fee rule;
  the correct payload is accepted afterwards.
- Replaying the same payloads without forkchoice updates uses non-canonical parent state,
  including the overhead change, and still rejects the wrong fee.
- Pending block construction and `eth_simulateV1` with `validation: false` still work.
- Every execution witness contains the L2SystemConfig account and storage proof nodes
  returned by `eth_getProof`, both before and after slot 101 is set in block 4.

Outputs are `1.json` through `7.json`, `provenance.json`, and `SHA256SUMS`.

The custom genesis uses Chikyu's registered chain ID **6281971** and L2SystemConfig address,
but activates Tsuki and TsukiV2 at genesis, so fee parameters (48, 10) apply. It includes funded development
accounts, a gas burner, and a stand-in contract that writes slot 101. It is not the Chikyu
network genesis.

SBV generates its own fixtures using `testdata/dogeos/base-fee-rule/`, including absent and
mid-chain-created system config accounts. Keep the shared block sequence and assertions in sync.

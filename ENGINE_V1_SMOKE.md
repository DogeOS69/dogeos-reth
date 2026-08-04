# Engine V1 runtime smoke

This records the standalone-node Engine API check performed on 2026-08-03 against the locally
built `dogeos-reth` binary. It supplements unit tests with the same Engine method family used by
`dogeos-rollup-node`.

## Scope

- Fresh `dev` Storage V2 datadir with local mining disabled.
- External JWT-authenticated Engine driver.
- Scroll-compatible nested payload attributes, `noTxPool=true`, no forced transactions,
  `withdrawals=null`, and `parentBeaconBlockRoot=null`.
- Payload timestamp after both the parent and the wall clock, so Reth's payload job deadline stays
  active while the payload is resolved.

## Observed transcript

1. `engine_forkchoiceUpdatedV1` returned `VALID` and payload ID
   `0x01536ade66e7f8a8`. The leading `0x01` proves the Scroll V1 payload-ID domain is
   present in the deployable binary.
2. `engine_getPayloadV1` returned block 1 with block hash
   `0x5265ffde2752fcca46208345f88134bc7b79273be5710591631f244298ee8e02` and state root
   `0x9b32336dbf3fc1790fccbfccf4345db44868eda006391aa239b66c598aa365cc`.
3. `engine_newPayloadV1` returned `VALID` with that block as `latestValidHash`.
4. A final `engine_forkchoiceUpdatedV1` made the block canonical.
5. JSON-RPC exposed the post-Euclid header invariants: difficulty one, zero beneficiary, zero nonce,
   zero mix hash, empty extra data, and no transactions or uncles.
6. After a clean shutdown, the same Storage V2 datadir passed Reth consistency checks and reopened
   the same canonical block hash and state root at height 1.

## Execution witness

After the restart, `debug_executionWitnessByBlockHash` succeeded for the Engine-imported block.
The returned `keys` included the canonical message-queue address
`0x5300000000000000000000000000000000000002`, withdraw-root slot `0xb`, and
next-message-index slot `0xc`. This confirms that the DogeOS witness preload runs through the
installed public RPC replacement and affects the generated witness.

## Forced L1 message

A second Engine V1 build supplied one canonical queue-index-zero L1 message in the inherited
`transactions` payload-attributes field with `noTxPool=true`:

```text
0x7ef180830186a09400000000000000000000000000000000000000008080940000000000000000000000000000000000000000
```

`engine_getPayloadV1` returned the identical bytes at transaction index zero. The payload used
21,000 gas, `engine_newPayloadV1` returned `VALID`, and the final forkchoice committed block 2.
JSON-RPC returned transaction type `0x7e`, queue index zero, and a successful receipt with
`gasUsed=0x5208`, `effectiveGasPrice=0`, and `l1Fee=0`. After another clean restart, Storage V2
returned the same block, transaction hash, transaction root, receipt root, and receipt fields.

The arbitrary transaction bytes in the historical payload-ID hashing fixture are intentionally not
an execution fixture: its first RLP field exceeds `u64` and correctly fails L1-message decoding with
`Overflow`. Runtime qualification therefore uses the valid canonical encoding above.

The first attempt intentionally used a historical timestamp one second after genesis and expired
before `getPayloadV1`; Reth correctly removed that past-deadline build job. Repeating with a current
timestamp completed the full flow above.

## Remaining qualification

This is an extension-point and local persistence check. It does not replace Chikyu replay, oracle
root comparison, sustained payload load, reorg, or abrupt-termination qualification.

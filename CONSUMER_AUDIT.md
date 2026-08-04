# Consumer and database-access audit

This is the locally reproducible portion of the Phase 0 consumer inventory.
It records source-level evidence from the adjacent DogeOS repositories. It does
not substitute for an inventory of deployed services, sidecars, backup jobs, or
operator scripts.

## Audited revisions

| Repository | Revision | Classification |
| --- | --- | --- |
| `dogeos-rollup-node` | `4d30abdeb6d044389a67ae68aeac641753babc96` | In-process Reth/node composition plus Engine/RPC consumer |
| `dogeos-core` | `4f693e855eb70ec037ab9887f1cf417781187907` | Reth-free protocol consumer with an oracle fixture-generator tool |

The `dogeos-core` checkout had unrelated local modifications during this
read-only audit. No consumer repository was changed.

## Findings

### `dogeos-rollup-node`

- Its workspace still selects the `scroll-tech/reth` `scroll-v91.7` family,
  including `scroll-alloy-*`, generic `reth-*`, and `reth-scroll-*` packages.
- Its node crate composes the Scroll execution node in process and uses
  `reth_provider::BlockReader` as a public provider trait.
- Direct `reth_db::DatabaseEnv` use found by the audit is confined to optional
  test utilities and temporary test databases.
- No source-level evidence was found of a separate production process opening
  the execution client's MDBX or RocksDB directories directly.

This consumer therefore needs an API/type migration to the standalone
DogeOS-owned packages. The checked source does not, by itself, block Storage V2
as an out-of-process database reader.

### `dogeos-core`

- The workspace still consumes `scroll-alloy-consensus` from the old
  `DogeOS69/dogeos-reth` fork at `39b31f822cc2b4c54db32ba2f0484ca2a157c3f5`.
- `tools/dogeos-fork-fixture-reth-generator` intentionally pins the old fork at
  `8594a121f9824538f0ebf78d763da561772d1bef` as its behavior owner.
- No production `reth_db`, `reth_provider`, MDBX, or RocksDB API use was found.

The protocol dependency should move to the canonical Reth-free
`dogeos-protocol-types` release. The historical generator must remain pinned
until replacement fixtures are generated and compared; it must not silently
follow the new client.

## Remaining operational decision

The local source audit found no known out-of-process direct database reader.
Storage V2 readiness still requires operators to confirm that deployed
services, backup/snapshot jobs, and one-off tooling do not open the old node's
MDBX, RocksDB, or static-file directories. That confirmation cannot be derived
from these two checkouts.

## Reproduction

Search production code separately from test utilities:

```sh
rg -n 'reth_db::|DatabaseEnv|libmdbx|rocksdb|open_db' \
  /path/to/dogeos-rollup-node/crates /path/to/dogeos-core/crates \
  --glob '*.rs' --glob '!**/tests/**' --glob '!**/test_utils/**'
```

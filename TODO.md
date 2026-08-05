# DogeOS Reth 2 Migration TODO

Last updated: 2026-08-04

This list tracks the remaining work required to qualify and cut over to the standalone DogeOS Reth
2 node. Completed migration evidence is recorded in [CAPABILITY_MATRIX.md](CAPABILITY_MATRIX.md),
[ENGINE_V1_SMOKE.md](ENGINE_V1_SMOKE.md), and
[SYNC_COMPATIBILITY_SMOKE.md](SYNC_COMPATIBILITY_SMOKE.md).

## P0 — Release Blockers

- [x] **Separate the Chikyu hardfork schedule and restore custom-genesis parsing.**
  - Chikyu is Feynman-active from genesis; Galileo, Galileo V2, and Tsuki fail closed until an
    authoritative activation timestamp is published.
  - Public checkpoint `0x676c32` confirms the Feynman oracle remains installed, the Galileo marker
    is zero, and the Tsuki NativeDogeToken predeploy is absent.
  - Custom genesis files preserve numeric, decimal-string, and hexadecimal fork timestamps.
  - See [CHIKYU_HARDFORK_SCHEDULE.md](CHIKYU_HARDFORK_SCHEDULE.md) and the frozen fixture.

- [ ] **Integrate the RocksDB synchronous-write durability fix.**
  - Select an upstream Reth revision or a minimal documented backport containing the equivalent of
    Reth PR #23603.
  - Preserve the Reth 2 and REVM 36 dependency family; do not upgrade to REVM 38 implicitly.
  - Record the exact revision and rationale in [UPSTREAM_PATCHES.md](UPSTREAM_PATCHES.md).
  - Make `scripts/audit-rocksdb-durability.sh` pass without weakening the audit.
  - Add crash-during-write, restart, and reorg durability tests.

- [ ] **Replay the Chikyu chain into a fresh Storage V2 datadir.**
  - Sync from genesis to an agreed finalized Chikyu height.
  - Compare block hashes plus state, transaction, and receipt roots with the pinned legacy oracle.
  - Compare gas usage, base fee, logs bloom, receipt status, `l1Fee`, forced L1 messages, native
    DOGE/Tsuki state, and execution witnesses over representative historical ranges.
  - Freeze the compared height, node revisions, commands, and results as reproducible evidence.
  - Use the frozen schedule checkpoint as the first full-replay target, then extend to the current
    finalized head.

- [ ] **Qualify the real rollup/consensus driver against the new execution node.**
  - Run `dogeos-rollup-node` against the new node through the Engine API.
  - Prove automatic live following without `--debug.tip`.
  - Test every rollup-node version required by the staged deployment plan.
  - Verify Engine `VALID`, `INVALID`, and `SYNCING` behavior during normal sync and recovery.

- [ ] **Complete reorg and failure-recovery qualification.**
  - Exercise canonical extension, side chains, and multi-block reorgs.
  - Terminate the node during staged sync, payload persistence, and reorg persistence.
  - Reopen MDBX, RocksDB, and static files after each failure point.
  - Prove that the restarted node converges to the same canonical head and roots as an
    uninterrupted node.

- [ ] **Produce and restore a release-candidate Storage V2 snapshot.**
  - Create a snapshot at an agreed finalized Chikyu height.
  - Restore it into an independent datadir/node.
  - Verify head, roots, RPC results, and subsequent incremental synchronization.

## P1 — Directly Implementable Qualification Work

- [ ] **Build a reproducible legacy-oracle fixture generator.**
  - Pin the old node revision and generation inputs.
  - Generate transaction envelope fixtures for Legacy, EIP-2930, EIP-1559, EIP-7702, and L1
    message transactions.
  - Generate receipt RLP/compact bytes, payload IDs, signed `scroll/1` messages, RPC responses, and
    execution witnesses.
  - Add all frozen outputs to `fixtures/SHA256SUMS` and `scripts/verify-fixtures.sh`.
  - Keep the historical generator pinned until every replacement fixture has been compared.

- [ ] **Finish payload DA-size boundary enforcement.**
  - Define the authoritative transaction codec/framing overhead used by the block-size limit.
  - Recompute and validate the final encoded block transaction size after payload construction.
  - Test exact-limit, one-byte-over-limit, forced-only, pool-only, and forced-plus-pool payloads.
  - Confirm that rejected pool transactions do not affect accounting and that an oversized forced
    set fails the payload build deterministically.

- [ ] **Expand Engine API negative and edge-case coverage.**
  - Test invalid parent hash, state root, transaction root, and receipt root inputs.
  - Test unsupported withdrawals and payload fields.
  - Test malformed/trailing forced-transaction bytes and invalid forced ordering.
  - Test equal, earlier, and future timestamps.
  - Test invalid payloads before and after restart and across reorg boundaries.

- [ ] **Add real two-node `scroll/1` integration tests.**
  - Announce and import a correctly signed block.
  - Reject an unauthorized signer before the block reaches Engine import.
  - Cover duplicate delivery, disconnect/reconnect, peer failure, and reorg announcements.
  - Verify the configured production signer on mainnet and Chikyu startup paths.

- [ ] **Run sustained-load and performance qualification.**
  - Measure payload build, `newPayload`, and forkchoice p50/p95/p99/max latency.
  - Record missed payload deadlines and persistence backpressure.
  - Exercise maximum-size blocks, cold start, cold cache, memory growth, and disk growth.
  - Run continuously for thousands of blocks with restart checkpoints.

- [ ] **Verify CI on the remote repository.**
  - Push the current workflow and run it on Rust 1.93.
  - Require formatting, dependency provenance, fixture integrity, workspace tests, Clippy with
    warnings denied, and no-default-features checks.
  - Enable the RocksDB durability audit as a required release gate after the patch is integrated.

## P2 — Consumer Migration and Cleanup

- [ ] **Migrate `dogeos-rollup-node` off the legacy Scroll Reth fork.**
  - Remove `scroll-tech/reth` `scroll-v91.7`, `scroll-alloy-*`, and old `reth-scroll-*`
    dependencies.
  - Consume the standalone DogeOS execution node through the intended Engine/RPC boundary.
  - Preserve required transaction bytes, receipt fields, payload IDs, RPC methods, and staged
    compatibility aliases.

- [ ] **Migrate `dogeos-core` to the Reth-free protocol package.**
  - Publish and pin `dogeos-protocol-types` at a reviewed revision/version.
  - Replace the production dependency on protocol types from the old Reth fork.
  - Keep the legacy fixture generator pinned until the differential fixture work is complete.

- [ ] **Audit deployed direct database consumers.**
  - Confirm that production services, backup jobs, snapshot tooling, and operator scripts do not
    open the old MDBX, RocksDB, or static-file directories directly.
  - Record owners and migration plans for every discovered consumer.
  - See [CONSUMER_AUDIT.md](CONSUMER_AUDIT.md) for the completed source-level audit.

- [ ] **Classify and clean up compatibility names.**
  - Decide which `Scroll*` names are stable external compatibility contracts.
  - Rename internal-only types to `Dogeos*` where useful.
  - Add temporary aliases only when they reduce staged consumer migration risk, with removal
    conditions documented.

- [ ] **Remove migration-only scaffolding and obsolete compatibility paths.**
  - Remove `dogeos-reth-spike` after the real node fully supersedes it.
  - Confirm that l2geth-only header transforms, signature backfill, and signed eth-wire fallbacks
    are absent or remove any remaining paths.
  - Remove dead feature gates, unused aliases, and stale migration documentation.

- [ ] **Promote the repository and binary to the canonical `dogeos-reth` identity.**
  - Complete the cutover from the temporary `dogeos-reth2` workspace/repository name.
  - Preserve the canonical `dogeos-reth` executable name.
  - Prove rollback by switching endpoints between independent legacy and new datadirs.

## Required Decisions and External Inputs

- [ ] Provide and verify the authorized `network.valid-signer` values for mainnet and Chikyu.
- [ ] Select the finalized Chikyu height and legacy oracle revision for release qualification.
- [ ] Decide whether Storage V1 datadir migration is required; otherwise retain the fresh Storage
  V2 replay strategy.
- [ ] Decide which old and new rollup-node versions must be supported during staged deployment.
- [ ] Commit the migration plan currently referenced by repository documentation, or remove those
  references if the plan will remain external.

## Standard Local Gates

Run these after each implementation batch:

```sh
scripts/verify-workspace.sh
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo check --workspace --no-default-features --locked --offline
```

The durability audit remains intentionally failing until the release-blocking Reth patch is
selected:

```sh
scripts/audit-rocksdb-durability.sh
```

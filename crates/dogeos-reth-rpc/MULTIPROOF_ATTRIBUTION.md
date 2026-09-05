# Optional multiproof attribution

Attach `Arc<MultiProofObserver>` with
`DogeosMultiProofApi::new(eth, limits).with_observer(observer.clone())` before
`into_rpc()`. Normal construction leaves observation disabled, with no observer
clock calls. There are no extra RPC methods, response fields, log writes, resets,
dependencies, or admission changes. The fixture owns opt-in and export.

`MultiProofObserver::default()` records wall time only.
`MultiProofObserver::with_thread_cpu_clock(fn() -> Option<Duration>)` additionally
records calling-thread CPU for synchronous operations. The supplied clock must
be monotonic, inexpensive, and non-panicking; return `None` if unavailable. It
must measure thread CPU, not process CPU. Other worker threads are excluded.
Async scopes never read that clock.

`snapshot() -> Vec<ProofStageSnapshot>` is serializable. Each fixed stage has
`active` and `success`, `error`, `abandoned` totals. Each outcome contains
`samples`, `wall_ns`, `thread_cpu_samples`, `thread_cpu_ns`. Zero CPU samples means
unavailable; a smaller CPU sample count means partial availability. Abandoned
means dropped without a result, including cancellation, discarded dispatch, or
unwinding. A timed-out request can have a subsequently successful worker sample.
Counters are cumulative; concurrent snapshots are approximate. Never reset them.

| Stage | Exact boundary | CPU |
| --- | --- | --- |
| `request` | Registered handler construction through result/drop; includes validation and admission | unavailable |
| `shared_admission_wait` | Shared tracing permit wait after successful endpoint admission | unavailable |
| `worker_dispatch_wait` | Job construction before spawn through start/discard | unavailable |
| `worker_service` | Synchronous worker body, including snapshot, proofs, serialization and local cleanup | optional thread |
| `snapshot` | `MultiProofProvider::proof_snapshot` | optional thread |
| `provider_proof_reconstruction` | One `state.multiproof` call, after target hashing | optional thread |
| `account_extraction` | Storage invariant check and `multiproof.account_proof`, once per attempted account | optional thread |
| `account_verification` | Strict verification, including inline-copy normalization of a verification copy | optional thread |
| `account_conversion` | EIP-1186 conversion and output key/order checks | optional thread |
| `serialization` | Bounded JSON serialization and raw-value construction | optional thread |
| `ordinary_proof_reconstruction` | Fixture-only: one ordinary `state.proof` call | optional thread |

Request and worker service overlap their child stages. Their sums are **not
additive**. Service includes enabled instrumentation overhead. Wall measurements
are elapsed time, never CPU. Snapshot does not isolate historical reconstruction:
the pinned historical provider invokes `revert_state` inside both `proof` and
`multiproof`, immediately before overlay proof generation. Provider time therefore
combines reconstruction and trie work.

## Quiescence and collection

Stop ingress and resolve all request outcomes before collecting a boundary.
`observer.await_idle().await` uses notifications to wait for observed scopes,
including unpolled constructed handlers, queued dispatch and detached shared
workers. The fixture must wrap it in its own bounded drain timeout. Service
starts before dispatch finishes. It ends just before the owned permits drop.

After observer idle, acquire **all configured tracing permits** from the same Eth
API (`acquire_many_owned_tracing(total)`) and hold them through snapshot/export.
Do not infer capacity from currently available permits. Drain before acquiring
the permits, to avoid blocking observed jobs waiting on the same semaphore.
Take before/after cumulative snapshots under this barrier; report count/sum
deltas. Record drain duration separately, outside the measured case.

This barrier requires **no uncertain untracked cancellation**. Ordinary Reth
`get_proof` holds its tracing permit in the outer request future, not the detached
proof worker. After cancellation, acquiring every permit cannot establish that
the ordinary worker exited. Ancillary RPC work, including a cancelled debug
witness request, is also untracked by this observer. Preserve the original case
failure and report `undrained`; stop subsequent cases when safe quiescence cannot
be established. Neither client semaphore release nor elapsed sleep proves idle.
This observer does not close ingress or instrument ordinary RPC internals.

## Matched provider baseline

Freeze one real provider snapshot and authenticated root. On that same retained
state and identical ordered targets, alternate these two arms:

1. For each target, `ordinary_observer.measure(ProofStage::OrdinaryProofReconstruction,
   || state.proof(Default::default(), address, keys))`. Separately measure strict
   verification with `verify_account_proof` and EIP-1186 conversion.
2. `build_proofs_observed(state, request, root, Some(&shared_observer))`.

Use a distinct observer per arm, compare the complete serialized response bytes,
and join the synchronous fixture worker before acknowledging a boundary. No
provider benchmark worker may be abandoned between cases. This compares repeated
provider work with shared provider work, including any reconstruction. It does
not isolate reconstruction alone or measure ordinary RPC CPU savings. The real
node quiet/state-churn matrix remains the end-to-end latency/CPU comparison.
The historical provider test validates this paired measurement interface and
byte equality; its small state is a correctness fixture, not performance evidence.

Adapter checks use repository CI toolchain Rust 1.93.0 through the shared node
build wrapper. Measurement artifacts must record their actual compiler/build;
do not silently compare builds made with different toolchains.

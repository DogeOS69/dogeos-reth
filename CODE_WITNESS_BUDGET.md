# Code witness budget

Transaction admission and payload construction share a local limit on distinct bytecode bytes.
The default is **32 MiB (33,554,432 bytes)**. This is an operator policy; it does not change gas
costs or consensus validity, and it is not a guarantee about total serialized witness size or
prover capacity. Trie nodes and witness encoding overhead are outside this counter.
Fixed fork-transition data loaded directly by host code is also outside the opcode inspector;
the policy bounds transaction-driven code growth rather than all proof input bytes.

Both `dogeos-reth` and the rollup node accept:

| Option | Default | Meaning |
| --- | ---: | --- |
| `--scroll.max-code-witness-bytes` | `33554432` | Block code budget and single-transaction admission budget |
| `--txpool.code-witness-max-steps` | `1000000` | Interpreter instruction allowance per admission simulation |
| `--txpool.code-witness-max-inflight` | `64` | Total waiting and executing admission validations |
| `--txpool.additional-validation-tasks` | Existing node default | Additional workers beyond the first validation worker |

The three new limits must be nonzero. The worker count is capped by the inflight limit.

## Admission

After the inexpensive pool checks, a transaction executes against one canonical parent's exact
state in a disposable cache. Future-nonce transactions use their declared nonce in that cache,
so they are simulated without consuming or changing canonical state. Execution uses an estimated
next-block environment; pending predecessors and the actual future block can change the result.

Accessing more unique code than the full block budget rejects admission immediately. Exhausting
the instruction allowance, a full admission queue, or an inconclusive simulation also rejects
the current request with a distinct retryable error. These errors do not label the transaction
permanently invalid. Ordinary REVERT and EVM halts within the allowance can still enter the pool.
Cancellation of a requesting RPC or peer does not release a running simulation's capacity early.

This bounds unpaid work in flight; it does not eliminate sustained admission load. Even a
transaction that passes admission must be checked again against the builder's actual state.

## Payload construction

The builder meters the union of code hashes across accepted transactions and system execution.
It charges original byte length, including cache hits and reads in reverted frames. Repeated
accesses to the same code, within one transaction or across transactions, are counted once.
`EXTCODESIZE` and `EXTCODECOPY` are covered; `EXTCODEHASH` does not force a bytecode load.
Each EIP-7702 authorization also reserves 23 bytes conservatively for a previous delegation
marker that validation may overwrite before the inspector can observe it.

Each pool transaction has a speculative code set. The builder commits its state, receipt, and
code set only if the union fits. Otherwise it skips the transaction and its dependent nonce
chain for this candidate, retaining other candidates. Physical cache entries left by a skipped
transaction do not exempt a later access from metering. Forced-transaction or system-execution
overflow fails the candidate explicitly instead of dropping mandatory inputs.

After a failed candidate containing multiple L1 messages, the rollup sequencer retries on a
later slot with a shorter ordered prefix. The Engine API does not preserve the specific builder
error, so this recovery also applies to other build failures and can temporarily reduce L1
throughput. A message that cannot fit alone remains an explicit failure; it is never skipped.

Code metering is host-only and does not alter block import, proof verification, or EVM opcode
pricing. Changing this policy alone is not a protocol hard fork.

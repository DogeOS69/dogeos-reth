# Protocol Storage Guidelines

## Scenario: Add DogeOS protocol-owned state

### 1. Scope / Trigger

Use this contract whenever `dogeos-reth-evm` adds protocol state that is not an
existing field in an inherited Solidity storage layout. It prevents collisions
with sequential contract fields and makes slot ownership auditable across native
state transitions, payload production, execution validation, witnesses, and
reverts.

Existing inherited contract fields keep their canonical Solidity slot. For
example, the L2 system-config base-fee overhead remains slot `101`.

### 2. Signatures

The canonical derivation helper is:

```rust
pub fn derive_protocol_storage_slot(namespace: &str) -> U256;
```

Protocol writes must pass through the shared REVM transition boundary:

```rust
fn commit_account_update<DB>(
    db: &mut DB,
    address: Address,
    original_info: AccountInfo,
    new_info: AccountInfo,
    storage: impl IntoIterator<Item = (U256, U256)>,
) -> Result<(), DB::Error>
where
    DB: Database + DatabaseCommit;
```

### 3. Contracts

- Every new DogeOS protocol-owned slot uses
  `U256::from_be_bytes(keccak256(namespace).0)`.
- The namespace is a stable, lowercase, domain-separated string. Use the shape
  `dogeos.storage.<feature>.<field>`.
- Consensus code stores the precomputed `U256` as a constant; it does not hash
  the namespace on every block.
- The namespace and constant are both public or otherwise inspectable, and a
  fixed-vector test proves that they match.
- Different fields use different namespaces. Renaming a namespace after
  activation is a consensus storage migration, not a refactor.
- New protocol-owned state never claims a sequential integer slot adjacent to an
  inherited Solidity layout.
- Writes use `commit_account_update` so original values are loaded before
  `DatabaseCommit` and witness/revert information remains complete.
- If a protocol write targets an EIP-161-empty account, initialize nonce `1`
  while preserving any existing balance, code, nonce, and unrelated storage.

The first adopted namespace is:

```text
dogeos.storage.dynamic_base_fee.next_controlled_fee
```

Its fixed slot is:

```text
0x74ae897ed5751dd32419f1eee8d4ec13d296adf0d77978ea55df0dd18345c8e3
```

### 4. Validation & Error Matrix

| Condition | Required behavior |
|---|---|
| Precomputed constant differs from namespace hash | Fixed-vector test fails; do not merge |
| New native slot uses a raw sequential integer | Review failure; replace it with a namespaced Keccak-256 slot |
| Two fields reuse one namespace | Review/test failure; allocate distinct stable namespaces |
| Active namespace is renamed | Require an explicit hardfork and state migration |
| Protocol update cannot read original account/slot | Return the database error and reject execution |
| Empty account would be cleared by EIP-161 | Set nonce `1` in the same protocol transition |

### 5. Good / Base / Bad Cases

- Good: add `dogeos.storage.example.next_value`, precompute its Keccak-256
  `U256`, verify it with `derive_protocol_storage_slot`, and write it through a
  focused transition helper using `commit_account_update`.
- Base: continue reading an inherited Solidity field from its already-established
  numeric slot without changing its layout.
- Bad: choose slot `103` because slots `101` and `102` are currently occupied.
- Bad: compute Keccak-256 at runtime on every block without pinning and testing a
  constant.

### 6. Tests Required

- Fixed-vector unit test: assert
  `derive_protocol_storage_slot(NAMESPACE) == PRECOMPUTED_SLOT`.
- Byte-order assertion: the `U256` big-endian bytes equal the published
  Keccak-256 digest.
- State-transition test: assert the expected address and only the intended slot
  change.
- Empty-account test: assert nonce becomes `1` and the slot survives REVM state
  clearing.
- Existing-account test: assert balance, code, nonce, and unrelated storage are
  preserved.
- Bundle/revert test for consensus transitions: assert the update is present in
  the state overlay used for root, witness, and rollback generation.

### 7. Wrong vs Correct

#### Wrong

```rust
// Sequential allocation can collide with a future Solidity layout field.
const NEXT_VALUE_SLOT: U256 = U256::from_limbs([103, 0, 0, 0]);
```

#### Correct

```rust
const NEXT_VALUE_NAMESPACE: &str = "dogeos.storage.example.next_value";
const NEXT_VALUE_SLOT: U256 = U256::from_be_bytes([
    // Precomputed Keccak-256 digest bytes.
]);

#[test]
fn next_value_slot_matches_namespace() {
    assert_eq!(
        derive_protocol_storage_slot(NEXT_VALUE_NAMESPACE),
        NEXT_VALUE_SLOT,
    );
}
```

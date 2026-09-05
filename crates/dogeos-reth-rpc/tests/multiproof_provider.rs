//! Differential coverage against the actual pinned Reth DB proof provider.

use alloy_consensus::{EMPTY_ROOT_HASH, Header, constants::KECCAK_EMPTY};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, B256, Bytes, U256, address, keccak256};
use alloy_rpc_types_eth::EIP1186AccountProofResponse;
use alloy_serde::JsonStorageKey;
use dogeos_reth_rpc::{
    GetProofsRequest, MultiProofObserver, MultiProofProvider, ProofStage, ProofTarget,
    build_proofs, build_proofs_observed, verify_account_proof,
};
use reth_chain_state::{ComputedTrieData, ExecutedBlock, NewCanonicalChain};
use reth_chainspec::ChainSpec;
use reth_db_api::{
    models::{AccountBeforeTx, BlockNumberAddress},
    tables,
    transaction::DbTxMut,
};
use reth_ethereum_primitives::Block;
use reth_primitives_traits::{Account, RecoveredBlock, SealedBlock, StorageEntry};
use reth_provider::{
    BlockWriter, HistoricalStateProvider, LatestStateProvider, ProviderFactory,
    providers::BlockchainProvider,
    test_utils::{
        MockNodeTypesWithDB, create_test_provider_factory_with_chain_spec, insert_genesis,
    },
};
use reth_storage_api::{
    StageCheckpointWriter, StateProofProvider, StateProvider, StateWriter, TrieWriter,
};
use reth_trie_common::{HashedPostState, HashedStorage, MultiProofTargets};
use std::{collections::BTreeMap, sync::Arc};

const CONTRACT: Address = address!("0000000000000000000000000000000000000011");
const OTHER_CONTRACT: Address = address!("0000000000000000000000000000000000000022");
const EMPTY_ACCOUNT: Address = address!("0000000000000000000000000000000000000033");
const ABSENT: Address = address!("0000000000000000000000000000000000000044");

fn slot(value: u64) -> B256 {
    B256::from(U256::from(value))
}

fn contract(balance: u64, storage: &[(u64, u64)]) -> GenesisAccount {
    GenesisAccount::default()
        .with_nonce(Some(3))
        .with_balance(U256::from(balance))
        .with_code(Some(Bytes::from_static(&[0x60, 0x00, 0x00])))
        .with_storage(Some(
            storage
                .iter()
                .map(|(key, value)| (slot(*key), slot(*value)))
                .collect(),
        ))
}

fn spec(alloc: impl IntoIterator<Item = (Address, GenesisAccount)>) -> Arc<ChainSpec> {
    Arc::new(ChainSpec {
        genesis: Genesis {
            alloc: alloc.into_iter().collect(),
            ..Default::default()
        },
        ..Default::default()
    })
}

fn populated_spec() -> Arc<ChainSpec> {
    spec([
        (CONTRACT, contract(101, &[(0, 7), (1, 8), (9, 19)])),
        (OTHER_CONTRACT, contract(202, &[(0, 10)])),
        (
            EMPTY_ACCOUNT,
            GenesisAccount::default().with_balance(U256::from(303)),
        ),
    ])
}

fn request(targets: impl IntoIterator<Item = (Address, Vec<B256>)>) -> GetProofsRequest {
    GetProofsRequest {
        // This fixture exercises the proof builder after snapshot resolution. Hash/header
        // resolution is tested separately through the adapter's snapshot provider.
        block_hash: B256::ZERO,
        targets: targets
            .into_iter()
            .map(|(address, storage_keys)| ProofTarget {
                address,
                storage_keys,
            })
            .collect(),
    }
}

fn individual_proofs(
    state: &dyn StateProvider,
    request: &GetProofsRequest,
    root: B256,
) -> Vec<EIP1186AccountProofResponse> {
    request
        .targets
        .iter()
        .map(|target| {
            // This is the StateProofProvider call and conversion used by eth_getProof,
            // intentionally separate from the experimental shared builder.
            let proof = state
                .proof(Default::default(), target.address, &target.storage_keys)
                .unwrap();
            verify_account_proof(&proof, root).unwrap();
            proof.into_eip1186_response(
                target
                    .storage_keys
                    .iter()
                    .copied()
                    .map(JsonStorageKey::from)
                    .collect(),
            )
        })
        .collect()
}

fn assert_equivalent(
    state: &dyn StateProvider,
    request: &GetProofsRequest,
    root: B256,
) -> Vec<EIP1186AccountProofResponse> {
    let expected = individual_proofs(state, request, root);
    let actual = build_proofs(state, request, root).unwrap();
    let observer = MultiProofObserver::default();
    let observed = build_proofs_observed(state, request, root, Some(&observer)).unwrap();
    assert_eq!(
        observed, actual,
        "observation must preserve every proof byte"
    );
    let snapshot = observer.snapshot();
    assert_eq!(
        snapshot[ProofStage::ProviderProofReconstruction as usize]
            .success
            .samples,
        1
    );
    for stage in [
        ProofStage::AccountExtraction,
        ProofStage::AccountVerification,
        ProofStage::AccountConversion,
    ] {
        assert_eq!(
            snapshot[stage as usize].success.samples,
            request.targets.len() as u64
        );
    }
    assert!(snapshot.iter().all(|stage| stage.active == 0
        && stage.error.samples == 0
        && stage.abandoned.samples == 0
        && stage.success.thread_cpu_samples == 0));
    assert_eq!(
        actual, expected,
        "shared proof must preserve every response field and node byte"
    );
    assert_eq!(actual.len(), request.targets.len());
    for (proof, target) in actual.iter().zip(&request.targets) {
        assert_eq!(proof.address, target.address);
        assert_eq!(proof.storage_proof.len(), target.storage_keys.len());
        assert_eq!(
            proof
                .storage_proof
                .iter()
                .map(|entry| entry.key.as_b256())
                .collect::<Vec<_>>(),
            target.storage_keys,
        );
    }
    actual
}

#[test]
fn database_proofs_preserve_order_absence_and_zero_slot_storage_root() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let state = LatestStateProvider::new(factory.provider().unwrap());
    let request = request([
        (ABSENT, vec![slot(9)]),
        (OTHER_CONTRACT, vec![]),
        (CONTRACT, vec![slot(9), slot(0), slot(55)]),
        (EMPTY_ACCOUNT, vec![slot(0)]),
    ]);

    let proofs = assert_equivalent(&state, &request, root);
    assert_eq!(proofs[0].balance, U256::ZERO);
    assert_eq!(proofs[0].nonce, 0);
    assert_eq!(proofs[0].code_hash, KECCAK_EMPTY);
    assert_eq!(proofs[0].storage_hash, EMPTY_ROOT_HASH);
    assert_eq!(proofs[0].storage_proof[0].value, U256::ZERO);
    assert_ne!(proofs[1].storage_hash, EMPTY_ROOT_HASH);
    assert!(proofs[1].storage_proof.is_empty());
    assert_eq!(proofs[2].storage_proof[0].value, U256::from(19));
    assert_eq!(proofs[2].storage_proof[1].value, U256::from(7));
    assert_eq!(proofs[2].storage_proof[2].value, U256::ZERO);
    assert_eq!(proofs[3].storage_hash, EMPTY_ROOT_HASH);
}

#[test]
fn empty_state_nonmembership_matches_individual_proofs() {
    let chain_spec = spec([]);
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    assert_eq!(root, EMPTY_ROOT_HASH);
    let state = LatestStateProvider::new(factory.provider().unwrap());
    let proofs = assert_equivalent(
        &state,
        &request([(ABSENT, vec![slot(1)]), (CONTRACT, vec![])]),
        root,
    );
    assert!(
        proofs
            .iter()
            .all(|proof| proof.storage_hash == EMPTY_ROOT_HASH)
    );
}

#[test]
fn historical_reconstruction_matches_individual_and_independent_parent_state() {
    let old_contract = contract(100, &[(0, 7), (1, 8), (9, 18)]);
    let parent_spec = spec([
        (CONTRACT, old_contract.clone()),
        (OTHER_CONTRACT, contract(202, &[(0, 10)])),
        (
            ABSENT,
            GenesisAccount::default().with_balance(U256::from(404)),
        ),
    ]);
    let parent_factory = create_test_provider_factory_with_chain_spec(parent_spec.clone());
    let parent_root = insert_genesis(&parent_factory, parent_spec).unwrap();

    let child_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(child_spec.clone());
    let child_root = insert_genesis(&factory, child_spec).unwrap();
    assert_ne!(parent_root, child_root);
    let writer = factory.provider_rw().unwrap();
    for (address, info) in [
        (CONTRACT, Some(Account::from(&old_contract))),
        (EMPTY_ACCOUNT, None),
        (
            ABSENT,
            Some(Account {
                balance: U256::from(404),
                ..Default::default()
            }),
        ),
    ] {
        writer
            .tx_ref()
            .put::<tables::AccountChangeSets>(1, AccountBeforeTx { address, info })
            .unwrap();
    }
    writer
        .tx_ref()
        .put::<tables::StorageChangeSets>(
            BlockNumberAddress((1, CONTRACT)),
            StorageEntry {
                key: slot(9),
                value: U256::from(18),
            },
        )
        .unwrap();
    writer.commit().unwrap();

    // HistoricalStateProvider indexes the first changeset to undo, hence 1 for
    // the post-state of block 0. Both account and storage reverts are exercised.
    let historical = HistoricalStateProvider::new(factory.provider().unwrap(), 1);
    let request = request([
        (CONTRACT, vec![slot(9), slot(55)]),
        (EMPTY_ACCOUNT, vec![slot(0)]),
        (ABSENT, vec![]),
        (OTHER_CONTRACT, vec![]),
    ]);
    let proofs = assert_equivalent(&historical, &request, parent_root);
    let independent_parent = LatestStateProvider::new(parent_factory.provider().unwrap());
    assert_eq!(
        proofs,
        individual_proofs(&independent_parent, &request, parent_root)
    );
    assert_eq!(proofs[0].balance, U256::from(100));
    assert_eq!(proofs[0].storage_proof[0].value, U256::from(18));
    assert_eq!(proofs[1].balance, U256::ZERO);
    assert_eq!(proofs[2].balance, U256::from(404));
    assert!(build_proofs(&historical, &request, child_root).is_err());

    // Alternating paired arms on one frozen real historical provider; this checks the
    // attribution baseline and exact wire equivalence, not a synthetic speed claim.
    let ordinary = MultiProofObserver::default();
    let shared = MultiProofObserver::default();
    let ordinary_arm = || {
        request
            .targets
            .iter()
            .map(|target| {
                let proof = ordinary
                    .measure(ProofStage::OrdinaryProofReconstruction, || {
                        historical.proof(Default::default(), target.address, &target.storage_keys)
                    })
                    .unwrap();
                ordinary
                    .measure(ProofStage::AccountVerification, || {
                        verify_account_proof(&proof, parent_root)
                    })
                    .unwrap();
                ordinary
                    .measure(ProofStage::AccountConversion, || {
                        Ok::<_, ()>(
                            proof.into_eip1186_response(
                                target
                                    .storage_keys
                                    .iter()
                                    .copied()
                                    .map(JsonStorageKey::from)
                                    .collect(),
                            ),
                        )
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    for reverse in [false, true] {
        let (left, right) = if reverse {
            let shared =
                build_proofs_observed(&historical, &request, parent_root, Some(&shared)).unwrap();
            (ordinary_arm(), shared)
        } else {
            let ordinary = ordinary_arm();
            (
                ordinary,
                build_proofs_observed(&historical, &request, parent_root, Some(&shared)).unwrap(),
            )
        };
        assert_eq!(
            serde_json::to_vec(&left).unwrap(),
            serde_json::to_vec(&right).unwrap()
        );
    }
    assert_eq!(
        ordinary.snapshot()[ProofStage::OrdinaryProofReconstruction as usize]
            .success
            .samples,
        8
    );
    assert_eq!(
        shared.snapshot()[ProofStage::ProviderProofReconstruction as usize]
            .success
            .samples,
        2
    );
    let failed = MultiProofObserver::default();
    assert_eq!(
        build_proofs_observed(&historical, &request, child_root, Some(&failed)).unwrap_err(),
        build_proofs(&historical, &request, child_root).unwrap_err(),
    );
    let snapshot = failed.snapshot();
    assert_eq!(
        snapshot[ProofStage::ProviderProofReconstruction as usize]
            .success
            .samples,
        1
    );
    assert_eq!(
        snapshot[ProofStage::AccountVerification as usize]
            .error
            .samples,
        1
    );
    assert_eq!(
        snapshot[ProofStage::AccountConversion as usize]
            .success
            .samples,
        0
    );
    assert!(snapshot.iter().all(|stage| stage.active == 0));
}

#[test]
fn pruned_account_or_storage_history_is_not_a_capability_failure() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let request = request([(CONTRACT, vec![slot(0)])]);
    let states = [
        HistoricalStateProvider::new(factory.provider().unwrap(), 1)
            .with_lowest_available_account_history_block_number(2),
        HistoricalStateProvider::new(factory.provider().unwrap(), 1)
            .with_lowest_available_storage_history_block_number(2),
    ];
    for state in states {
        assert!(
            state
                .proof(Default::default(), CONTRACT, &[slot(0)])
                .is_err()
        );
        let error = build_proofs(&state, &request, root).unwrap_err();
        assert_ne!(error.code(), -32601);
    }
}

#[test]
fn shared_storage_proofs_preserve_inline_nodes() {
    // A shared eight-nibble hash prefix leaves short enough suffixes for inline
    // storage leaves. Search is deterministic and bounded, not random fuzzing.
    let mut prefixes = BTreeMap::new();
    let (first, second) = (0..500_000_u64)
        .find_map(|key| {
            let hash = keccak256(slot(key));
            let prefix = u32::from_be_bytes(hash[..4].try_into().unwrap());
            prefixes.insert(prefix, key).map(|previous| (previous, key))
        })
        .expect("deterministic inline-node fixture has a prefix collision");
    let chain_spec = spec([(
        CONTRACT,
        contract(1, &[(first, 1), (second, 2), (500_000, 3)]),
    )]);
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let state = LatestStateProvider::new(factory.provider().unwrap());
    let header = Header {
        state_root: root,
        ..Default::default()
    };
    let mut request = request([
        (CONTRACT, vec![slot(second), slot(first), slot(500_001)]),
        (ABSENT, vec![]),
    ]);
    request.block_hash = header.hash_slow();
    let native = request
        .targets
        .iter()
        .map(|target| {
            state
                .proof(Default::default(), target.address, &target.storage_keys)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let targets = request
        .targets
        .iter()
        .map(|target| {
            (
                keccak256(target.address),
                target.storage_keys.iter().map(keccak256).collect(),
            )
        })
        .collect::<MultiProofTargets>();
    let multiproof = state.multiproof(Default::default(), targets).unwrap();
    let shared = request
        .targets
        .iter()
        .map(|target| {
            multiproof
                .account_proof(target.address, &target.storage_keys)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        native, shared,
        "diagnosis must retain exact ordinary/shared byte equality"
    );
    let mut normalized = native[0].clone();
    for storage in normalized.storage_proofs.iter_mut().take(2) {
        assert_independent_inline_path(
            storage.key,
            storage.value,
            &storage.proof,
            normalized.storage_root,
        );
        storage.proof.pop();
    }
    normalized
        .verify(root)
        .expect("exact redundant inline removal verifies without changing authenticated values");
    let original = native[0].clone();
    verify_account_proof(&original, root).unwrap();
    verify_account_proof(&normalized, root).unwrap();
    assert_eq!(
        original, native[0],
        "verification must not mutate wire nodes"
    );
    let mut tampered = original.clone();
    let mut last = tampered.storage_proofs[0].proof.last().unwrap().to_vec();
    *last.last_mut().unwrap() = 3;
    *tampered.storage_proofs[0].proof.last_mut().unwrap() = last.into();
    assert!(verify_account_proof(&tampered, root).is_err());
    let mut unrelated = original.clone();
    unrelated.storage_proofs[0]
        .proof
        .push(original.proof[0].clone());
    assert!(verify_account_proof(&unrelated, root).is_err());
    let mut repeated_twice = original.clone();
    let extra = repeated_twice.storage_proofs[0]
        .proof
        .last()
        .unwrap()
        .clone();
    repeated_twice.storage_proofs[0].proof.push(extra);
    assert!(verify_account_proof(&repeated_twice, root).is_err());
    for length in [1, 2] {
        let mut truncated = original.clone();
        truncated.storage_proofs[0].proof.truncate(length);
        truncated.storage_proofs[0].value = U256::ZERO;
        assert!(
            verify_account_proof(&truncated, root).is_err(),
            "unfinished hash path cannot prove zero"
        );
    }
    if let Some(path) = std::env::var_os("MULTIPROOF_DIAGNOSTIC_OUT") {
        let errors = native
            .iter()
            .map(|proof| proof.verify(root).err().map(|error| format!("{error:?}")))
            .collect::<Vec<_>>();
        let responses = native
            .into_iter()
            .zip(&request.targets)
            .map(|(proof, target)| {
                proof.into_eip1186_response(
                    target
                        .storage_keys
                        .iter()
                        .copied()
                        .map(Into::into)
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "status": "diagnostic: native proof verification failed; not an accepted fixture",
                "reth_revision": "972366a0bfc11cf6a0d5dc79d5e779cd81e32232",
                "parent_header": header,
                "parent_hash": header.hash_slow(),
                "parent_state_root": root,
                "request": request,
                "proofs": responses,
                    "native_verification_errors": errors,
                    "ordinary_shared_exact_equality": true,
                    "independent_inline_path_checks": "root hash, root branch edge, extension path/hash, embedded leaf suffix/value checked directly from RLP",
                    "exact_redundant_inline_removal_verifies": true,
                    "inline_slot_numbers": [first, second],
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let proofs = assert_equivalent(&state, &request, root);
    assert!(
        proofs[0]
            .storage_proof
            .iter()
            .flat_map(|proof| &proof.proof)
            .any(|node| {
                let mut bytes = node.as_ref();
                match alloy_rlp::Header::decode_raw(&mut bytes).unwrap() {
                    alloy_rlp::PayloadView::List(items) if items.len() == 17 => {
                        items[..16].iter().any(|child| {
                            child.len() < 32 && child.first().is_some_and(|byte| *byte >= 0xc0)
                        })
                    }
                    _ => false,
                }
            }),
        "fixture must actually contain an inline child in an authenticated branch",
    );
    assert!(proofs[0].storage_proof[0].proof.len() >= 3);
    export_checked_fixture(
        Some("provider-inline-proof-fixture.json"),
        &header,
        &request,
        &proofs,
    );
}

fn assert_independent_inline_path(key: B256, value: U256, nodes: &[Bytes], root: B256) {
    fn list(raw: &[u8]) -> Vec<&[u8]> {
        let mut bytes = raw;
        let alloy_rlp::PayloadView::List(items) =
            alloy_rlp::Header::decode_raw(&mut bytes).unwrap()
        else {
            panic!("expected RLP list");
        };
        assert!(bytes.is_empty());
        items.to_vec()
    }
    fn string(raw: &[u8]) -> &[u8] {
        let mut bytes = raw;
        let alloy_rlp::PayloadView::String(value) =
            alloy_rlp::Header::decode_raw(&mut bytes).unwrap()
        else {
            panic!("expected RLP string");
        };
        assert!(bytes.is_empty());
        value
    }
    fn compact_path(raw: &[u8], is_leaf: bool) -> Vec<u8> {
        let bytes = string(raw);
        let flag = bytes[0] >> 4;
        assert_eq!(flag >> 1, u8::from(is_leaf));
        let mut path = Vec::new();
        if flag & 1 == 1 {
            path.push(bytes[0] & 15);
        } else {
            assert_eq!(bytes[0] & 15, 0);
        }
        for byte in &bytes[1..] {
            path.extend([byte >> 4, byte & 15]);
        }
        path
    }
    assert_eq!(nodes.len(), 4);
    let nibbles = keccak256(key)
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 15])
        .collect::<Vec<_>>();
    assert_eq!(keccak256(&nodes[0]), root);
    let first_branch = list(&nodes[0]);
    assert_eq!(first_branch.len(), 17);
    assert_eq!(
        string(first_branch[nibbles[0] as usize]),
        keccak256(&nodes[1]).as_slice()
    );
    let extension = list(&nodes[1]);
    assert_eq!(extension.len(), 2);
    let path = compact_path(extension[0], false);
    assert_eq!(path, nibbles[1..1 + path.len()]);
    assert_eq!(string(extension[1]), keccak256(&nodes[2]).as_slice());
    let offset = 1 + path.len();
    let branch = list(&nodes[2]);
    assert_eq!(branch.len(), 17);
    let embedded = branch[nibbles[offset] as usize];
    assert!(embedded.len() < 32);
    assert_eq!(embedded, nodes[3].as_ref());
    let leaf = list(embedded);
    assert_eq!(leaf.len(), 2);
    assert_eq!(compact_path(leaf[0], true), nibbles[offset + 1..]);
    assert_eq!(string(leaf[1]), alloy_rlp::encode(value));
}

fn export_checked_fixture(
    filename: Option<&str>,
    header: &Header,
    request: &GetProofsRequest,
    proofs: &[EIP1186AccountProofResponse],
) {
    // Opt-in export lets the core verifier consume proofs checked against the
    // real provider without adding generated fixtures to the source tree.
    if let Some(path) = std::env::var_os("MULTIPROOF_FIXTURE_OUT") {
        let mut path = std::path::PathBuf::from(path);
        if let Some(filename) = filename {
            path.set_file_name(filename);
        }
        let fixture = serde_json::json!({
            "format": 1,
            "reth_revision": "972366a0bfc11cf6a0d5dc79d5e779cd81e32232",
            "parent_header": header,
            "parent_hash": header.hash_slow(),
            "parent_state_root": header.state_root,
            "request": request,
            "proofs": proofs,
        });
        std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    }
}

fn insert_header(
    factory: &ProviderFactory<MockNodeTypesWithDB>,
    header: Header,
) -> RecoveredBlock<Block> {
    let block = RecoveredBlock::new_sealed(
        SealedBlock::<Block>::seal_parts(header, Default::default()),
        vec![],
    );
    let writer = factory.provider_rw().unwrap();
    writer.insert_block(&block).unwrap();
    writer.commit().unwrap();
    block
}

fn error_kind(error: &jsonrpsee::types::ErrorObjectOwned) -> String {
    serde_json::from_str::<serde_json::Value>(error.data().unwrap().get()).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn snapshot_binds_database_header_and_emits_checked_fixture() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let header = Header {
        state_root: root,
        gas_limit: 30_000_000,
        ..Default::default()
    };
    let hash = header.hash_slow();
    insert_header(&factory, header.clone());
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let (snapshot_root, state) = provider.proof_snapshot(hash, 0).unwrap();
    assert_eq!(snapshot_root, root);
    let mut request = request([
        (ABSENT, vec![slot(9)]),
        (OTHER_CONTRACT, vec![]),
        (CONTRACT, vec![slot(9), slot(0), slot(55)]),
        (EMPTY_ACCOUNT, vec![slot(0)]),
    ]);
    request.block_hash = hash;
    let proofs = assert_equivalent(state.as_ref(), &request, root);

    export_checked_fixture(None, &header, &request, &proofs);

    assert!(
        provider
            .proof_snapshot(B256::repeat_byte(0xee), 100)
            .is_err()
    );
    // A stale hash-to-number mapping must not authenticate the canonical header
    // at that height as a different requested hash.
    let stale_hash = B256::repeat_byte(0xdd);
    let writer = factory.provider_rw().unwrap();
    writer
        .tx_ref()
        .put::<tables::HeaderNumbers>(stale_hash, 0)
        .unwrap();
    writer.commit().unwrap();
    let error = provider.proof_snapshot(stale_hash, 100).err().unwrap();
    assert_eq!(error_kind(&error), "header_hash_mismatch");
}

fn memory_block(
    base: &dyn StateProvider,
    parent_hash: B256,
    balance: u64,
    value: u64,
) -> ExecutedBlock {
    let mut changes = HashedPostState::default();
    changes.accounts.insert(
        keccak256(CONTRACT),
        Some(Account::from(&contract(balance, &[]))),
    );
    changes.storages.insert(
        keccak256(CONTRACT),
        HashedStorage::from_iter(false, [(keccak256(slot(9)), U256::from(value))]),
    );
    let (root, updates) = base.state_root_with_updates(changes.clone()).unwrap();
    let block = RecoveredBlock::new_sealed(
        SealedBlock::<Block>::seal_parts(
            Header {
                parent_hash,
                number: 1,
                state_root: root,
                ..Default::default()
            },
            Default::default(),
        ),
        vec![],
    );
    ExecutedBlock::new(
        Arc::new(block),
        Arc::new(Default::default()),
        ComputedTrieData {
            hashed_state: Arc::new(changes.into_sorted()),
            trie_updates: Arc::new(updates.into_sorted()),
            ..Default::default()
        },
    )
}

#[test]
fn memory_snapshot_matches_independent_state_and_survives_later_reorg() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let genesis = insert_header(
        &factory,
        Header {
            state_root: root,
            ..Default::default()
        },
    );
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let base = LatestStateProvider::new(factory.provider().unwrap());
    let first = memory_block(&base, genesis.hash(), 111, 29);
    let first_hash = first.recovered_block().hash();
    provider
        .canonical_in_memory_state()
        .update_chain(NewCanonicalChain::Commit {
            new: vec![first.clone()],
        });
    let (first_root, captured) = provider.proof_snapshot(first_hash, 1).unwrap();
    let mut request = request([
        (CONTRACT, vec![slot(9), slot(0), slot(55)]),
        (ABSENT, vec![slot(0)]),
        (OTHER_CONTRACT, vec![]),
    ]);
    request.block_hash = first_hash;
    let proofs = assert_equivalent(captured.as_ref(), &request, first_root);
    assert_eq!(proofs[0].balance, U256::from(111));
    assert_eq!(proofs[0].storage_proof[0].value, U256::from(29));

    let oracle_spec = spec([
        (CONTRACT, contract(111, &[(0, 7), (1, 8), (9, 29)])),
        (OTHER_CONTRACT, contract(202, &[(0, 10)])),
        (
            EMPTY_ACCOUNT,
            GenesisAccount::default().with_balance(U256::from(303)),
        ),
    ]);
    let oracle_factory = create_test_provider_factory_with_chain_spec(oracle_spec.clone());
    let oracle_root = insert_genesis(&oracle_factory, oracle_spec).unwrap();
    assert_eq!(first_root, oracle_root);
    let oracle = LatestStateProvider::new(oracle_factory.provider().unwrap());
    assert_eq!(proofs, individual_proofs(&oracle, &request, oracle_root));
    // The same snapshot's head is also the basis for proof-window policy.
    assert!(provider.proof_snapshot(genesis.hash(), 0).is_err());

    let replacement = memory_block(&base, genesis.hash(), 222, 39);
    let replacement_hash = replacement.recovered_block().hash();
    provider
        .canonical_in_memory_state()
        .update_chain(NewCanonicalChain::Reorg {
            old: vec![first],
            new: vec![replacement],
        });
    assert!(provider.proof_snapshot(first_hash, 1).is_err());
    let (replacement_root, replacement_state) =
        provider.proof_snapshot(replacement_hash, 1).unwrap();
    assert_ne!(first_root, replacement_root);
    let replacement_proofs =
        assert_equivalent(replacement_state.as_ref(), &request, replacement_root);
    assert_eq!(replacement_proofs[0].balance, U256::from(222));
    assert_eq!(
        proofs,
        build_proofs(captured.as_ref(), &request, first_root).unwrap()
    );
    assert!(build_proofs(captured.as_ref(), &request, replacement_root).is_err());
}

#[test]
fn snapshot_rejects_memory_database_anchor_mismatch() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    insert_header(
        &factory,
        Header {
            state_root: root,
            ..Default::default()
        },
    );
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let base = LatestStateProvider::new(factory.provider().unwrap());
    let wrong_anchor = memory_block(&base, B256::repeat_byte(0xaa), 111, 29);
    let hash = wrong_anchor.recovered_block().hash();
    provider
        .canonical_in_memory_state()
        .update_chain(NewCanonicalChain::Commit {
            new: vec![wrong_anchor],
        });
    let error = provider.proof_snapshot(hash, 1).err().unwrap();
    assert_eq!(error_kind(&error), "snapshot_mismatch");
}

#[test]
fn captured_memory_snapshot_and_fresh_database_snapshot_agree_after_persistence() {
    let chain_spec = populated_spec();
    let factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    let root = insert_genesis(&factory, chain_spec).unwrap();
    let genesis = insert_header(
        &factory,
        Header {
            state_root: root,
            ..Default::default()
        },
    );
    let provider = BlockchainProvider::new(factory.clone()).unwrap();
    let base = LatestStateProvider::new(factory.provider().unwrap());
    let executed = memory_block(&base, genesis.hash(), 111, 29);
    let hash = executed.recovered_block().hash();
    let memory = provider.canonical_in_memory_state();
    memory.update_chain(NewCanonicalChain::Commit {
        new: vec![executed.clone()],
    });
    let (captured_root, captured) = provider.proof_snapshot(hash, 1).unwrap();
    let mut request = request([
        (CONTRACT, vec![slot(9), slot(0)]),
        (OTHER_CONTRACT, vec![]),
        (ABSENT, vec![slot(0)]),
    ]);
    request.block_hash = hash;
    let expected = assert_equivalent(captured.as_ref(), &request, captured_root);

    // Persist the same computed trie/state data plus real historical reverts,
    // then retire the memory block exactly as the provider's persistence path does.
    let writer = factory.provider_rw().unwrap();
    writer.insert_block(executed.recovered_block()).unwrap();
    let trie_data = executed.trie_data();
    writer.write_hashed_state(&trie_data.hashed_state).unwrap();
    writer
        .write_trie_updates_sorted(&trie_data.trie_updates)
        .unwrap();
    writer
        .tx_ref()
        .put::<tables::AccountChangeSets>(
            1,
            AccountBeforeTx {
                address: CONTRACT,
                info: Some(Account::from(&contract(101, &[]))),
            },
        )
        .unwrap();
    writer
        .tx_ref()
        .put::<tables::StorageChangeSets>(
            BlockNumberAddress((1, CONTRACT)),
            StorageEntry {
                key: slot(9),
                value: U256::from(19),
            },
        )
        .unwrap();
    writer.update_pipeline_stages(1, false).unwrap();
    writer.commit().unwrap();
    memory.remove_persisted_blocks(alloy_eips::BlockNumHash::new(1, hash));
    assert!(memory.head_state().is_none());

    let (persisted_root, persisted) = provider.proof_snapshot(hash, 1).unwrap();
    assert_eq!(persisted_root, captured_root);
    assert_eq!(
        expected,
        assert_equivalent(persisted.as_ref(), &request, persisted_root)
    );
    assert_eq!(
        expected,
        assert_equivalent(captured.as_ref(), &request, captured_root)
    );
    let (parent_root, parent) = provider.proof_snapshot(genesis.hash(), 1).unwrap();
    assert_eq!(parent_root, root);
    let parent_proofs = assert_equivalent(parent.as_ref(), &request, parent_root);
    assert_eq!(parent_proofs[0].balance, U256::from(101));
    assert_eq!(parent_proofs[0].storage_proof[0].value, U256::from(19));
}

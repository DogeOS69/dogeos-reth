//! Bounded, opt-in parent-state proofs. This is not a guest proof format.

use alloy_consensus::{BlockHeader, Sealable};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use alloy_rlp::Decodable;
use alloy_rpc_types_eth::EIP1186AccountProofResponse;
use jsonrpsee::{
    RpcModule,
    types::{ErrorObjectOwned, Params},
};
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use reth_rpc_eth_api::{
    RpcNodeCore,
    helpers::{EthState, SpawnBlocking},
};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{
    BlockHashReader, BlockNumReader, DatabaseProviderFactory, HeaderProvider, StateProvider,
    StateProviderBox, TryIntoHistoricalStateProvider,
};
use reth_trie_common::{
    AccountProof, EMPTY_ROOT_HASH, MultiProofTargets, Nibbles, RlpNode, TrieNode,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::future::Future;
use std::{
    collections::HashSet,
    io::{self, Write},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

/// A target's original keys are retained for response ordering and formatting.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProofTarget {
    pub address: Address,
    pub storage_keys: Vec<B256>,
}

/// The hash denotes post-state of this block, not its parent's state.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetProofsRequest {
    pub block_hash: B256,
    pub targets: Vec<ProofTarget>,
}

/// Operational limits may be tightened; the experimental ceilings cannot be raised.
/// Response bytes bound serialized output, not the intermediate proof working set.
#[derive(Clone, Debug)]
pub struct MultiProofLimits {
    pub max_params_bytes: usize,
    pub max_response_bytes: usize,
    pub max_jobs: usize,
    pub shared_permit_wait: Duration,
    pub deadline: Duration,
}

impl Default for MultiProofLimits {
    fn default() -> Self {
        Self {
            max_params_bytes: 64 * 1024,
            max_response_bytes: 4 * 1024 * 1024,
            max_jobs: 2,
            shared_permit_wait: Duration::from_secs(1),
            deadline: Duration::from_secs(30),
        }
    }
}

impl MultiProofLimits {
    fn bounded(mut self) -> Self {
        let ceiling = Self::default();
        self.max_params_bytes = self.max_params_bytes.min(ceiling.max_params_bytes);
        self.max_response_bytes = self.max_response_bytes.min(ceiling.max_response_bytes);
        self.max_jobs = self.max_jobs.min(ceiling.max_jobs);
        self.shared_permit_wait = self.shared_permit_wait.min(ceiling.shared_permit_wait);
        self.deadline = self.deadline.min(ceiling.deadline);
        self
    }
}

fn invalid(message: &'static str) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32602, message, None::<()>)
}

fn failure(kind: &'static str, message: &'static str) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32000, message, Some(serde_json::json!({"kind": kind})))
}

fn resource(kind: &'static str) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(
        -32005,
        "Multiproof resource limit",
        Some(serde_json::json!({"kind": kind})),
    )
}

fn provider_error(error: reth_provider::ProviderError) -> ErrorObjectOwned {
    EthApiError::from(error).into()
}

impl GetProofsRequest {
    fn validate(&self) -> Result<(), ErrorObjectOwned> {
        if self.targets.is_empty() || self.targets.len() > 4 {
            return Err(invalid("Expected 1 to 4 proof targets"));
        }
        let mut addresses = HashSet::new();
        let mut total = 0;
        for target in &self.targets {
            if !addresses.insert(target.address) {
                return Err(invalid("Duplicate proof address"));
            }
            if target.storage_keys.len() > 4 {
                return Err(invalid("At most 4 storage keys per account"));
            }
            let mut keys = HashSet::new();
            if target.storage_keys.iter().any(|key| !keys.insert(*key)) {
                return Err(invalid("Duplicate storage key"));
            }
            total += target.storage_keys.len();
        }
        if total > 8 {
            return Err(invalid("At most 8 storage keys total"));
        }
        Ok(())
    }
}

fn parse_request(
    params: &Params<'_>,
    limits: &MultiProofLimits,
) -> Result<GetProofsRequest, ErrorObjectOwned> {
    if params.len_bytes() > limits.max_params_bytes {
        return Err(invalid("Multiproof params too large"));
    }
    // Tuple parsing requires exactly one positional object, not arbitrary named params.
    let (request,): (GetProofsRequest,) = params.parse()?;
    request.validate()?;
    Ok(request)
}

/// A compact-local boundary for an authenticated, single-view state provider.
/// Implementations must not reopen a state view after authenticating the header.
pub trait MultiProofProvider: Send + Sync {
    fn proof_snapshot(
        &self,
        hash: B256,
        proof_window: u64,
    ) -> Result<(B256, StateProviderBox), ErrorObjectOwned>;
}

impl<N: ProviderNodeTypes> MultiProofProvider for BlockchainProvider<N> {
    fn proof_snapshot(
        &self,
        hash: B256,
        proof_window: u64,
    ) -> Result<(B256, StateProviderBox), ErrorObjectOwned> {
        // Match ConsistentProvider's ordering: a flush between these captures is safe;
        // the reverse order could leave a gap in both views. Never consult live memory again.
        let head = self.canonical_in_memory_state().head_state();
        let db = self.database_provider_ro().map_err(provider_error)?;
        if let Some(head) = &head {
            let anchor = head.anchor();
            if db.block_hash(anchor.number).map_err(provider_error)? != Some(anchor.hash) {
                return Err(failure(
                    "snapshot_mismatch",
                    "Canonical memory/database anchor mismatch",
                ));
            }
        }
        let memory = head
            .as_ref()
            .and_then(|head| head.block_on_chain(BlockHashOrNumber::Hash(hash)));
        let header = if let Some(block) = memory {
            block.block_ref().recovered_block().header().clone()
        } else {
            db.header(hash)
                .map_err(provider_error)?
                .ok_or_else(|| ErrorObjectOwned::from(EthApiError::HeaderNotFound(hash.into())))?
        };
        if header.hash_slow() != hash {
            return Err(failure(
                "header_hash_mismatch",
                "Requested header hash mismatch",
            ));
        }
        let number = header.number();
        let best = if let Some(head) = &head {
            head.number()
        } else {
            db.best_block_number().map_err(provider_error)?
        };
        let canonical_hash = if let Some(block) = head
            .as_ref()
            .and_then(|head| head.block_on_chain(BlockHashOrNumber::Number(number)))
        {
            Some(block.hash())
        } else {
            db.block_hash(number).map_err(provider_error)?
        };
        if number > best || canonical_hash != Some(hash) {
            return Err(failure(
                "noncanonical",
                "Requested block is not canonical in the captured view",
            ));
        }
        if best.saturating_sub(number) > proof_window {
            return Err(EthApiError::ExceedsMaxProofWindow.into());
        }
        let root = header.state_root();
        let state = if let Some(block) = memory {
            let historical = db
                .try_into_history_at_block(block.anchor().number)
                .map_err(provider_error)?;
            Box::new(block.state_provider(historical)) as StateProviderBox
        } else {
            db.try_into_history_at_block(number)
                .map_err(provider_error)?
        };
        Ok((root, state))
    }
}

/// Shared production path exposed for pinned-provider differential fixtures.
#[doc(hidden)]
pub fn build_proofs(
    state: &dyn StateProvider,
    request: &GetProofsRequest,
    root: B256,
) -> Result<Vec<EIP1186AccountProofResponse>, ErrorObjectOwned> {
    request.validate()?;
    let mut targets = MultiProofTargets::default();
    for target in &request.targets {
        targets
            .entry(keccak256(target.address))
            .or_default()
            .extend(target.storage_keys.iter().map(keccak256));
    }
    let multiproof = state
        .multiproof(Default::default(), targets)
        .map_err(provider_error)?;
    request
        .targets
        .iter()
        .map(|target| {
            // Do not silently use account_proof's EMPTY_ROOT fallback, including zero-key queries.
            if !multiproof.storages.contains_key(&keccak256(target.address)) {
                return Err(failure(
                    "proof_invariant",
                    "Missing target storage multiproof",
                ));
            }
            let proof = multiproof
                .account_proof(target.address, &target.storage_keys)
                .map_err(|_| failure("proof_invariant", "Could not extract account proof"))?;
            verify_account_proof(&proof, root)?;
            let response = proof.into_eip1186_response(
                target
                    .storage_keys
                    .iter()
                    .copied()
                    .map(Into::into)
                    .collect(),
            );
            if response.storage_proof.len() != target.storage_keys.len()
                || response
                    .storage_proof
                    .iter()
                    .zip(&target.storage_keys)
                    .any(|(proof, key)| proof.key.as_b256() != *key)
            {
                return Err(failure(
                    "proof_invariant",
                    "Incomplete or unordered storage proof",
                ));
            }
            Ok(response)
        })
        .collect()
}

/// Verify without changing the ordinary Reth proof representation returned to callers.
///
/// Reth can retain an inline child both inside its parent and as the next proof node.
/// The native verifier already walks that child inside the parent. Remove only that
/// exact, path-referenced repetition from a verification copy, never from wire output.
#[doc(hidden)]
pub fn verify_account_proof(proof: &AccountProof, root: B256) -> Result<(), ErrorObjectOwned> {
    let mut verification = proof.clone();
    verification.proof = verification_nodes(&proof.proof, root, keccak256(proof.address))?;
    for storage in &mut verification.storage_proofs {
        let key = keccak256(storage.key);
        if storage.nibbles != Nibbles::unpack(key) {
            return Err(failure("proof_invariant", "Storage key path mismatch"));
        }
        storage.proof = verification_nodes(&storage.proof, proof.storage_root, key)?;
    }
    verification.verify(root).map_err(|_| {
        failure(
            "proof_root_mismatch",
            "Proof does not match requested state root",
        )
    })
}

fn verification_nodes(
    nodes: &[Bytes],
    root: B256,
    key: B256,
) -> Result<Vec<Bytes>, ErrorObjectOwned> {
    let invalid = || {
        failure(
            "proof_invariant",
            "Invalid proof node path or redundant node",
        )
    };
    if nodes.is_empty() || nodes[0].as_ref() == [0x80] {
        if root != EMPTY_ROOT_HASH || nodes.len() > 1 {
            return Err(invalid());
        }
        return Ok(nodes.to_vec());
    }

    let key = Nibbles::unpack(key);
    let mut position = 0;
    let mut next = 0;
    let mut reference = RlpNode::word_rlp(&root);
    let mut result = Vec::with_capacity(nodes.len());
    loop {
        let encoded = if let Some(hash) = reference.as_hash() {
            let node = nodes.get(next).ok_or_else(invalid)?;
            if keccak256(node) != hash {
                return Err(invalid());
            }
            next += 1;
            result.push(node.clone());
            node.clone()
        } else {
            if reference.len() >= 32 {
                return Err(invalid());
            }
            // An omitted inline copy is valid too. No other node may be discarded:
            // a different next node must match a later hash reference or be rejected below.
            if nodes
                .get(next)
                .is_some_and(|node| node.as_ref() == reference.as_slice())
            {
                next += 1;
            }
            Bytes::copy_from_slice(&reference)
        };
        let mut input = encoded.as_ref();
        let node = TrieNode::decode(&mut input).map_err(|_| invalid())?;
        if !input.is_empty() {
            return Err(invalid());
        }
        match node {
            TrieNode::Branch(branch) => {
                let Some(nibble) = key.get(position) else {
                    break;
                };
                if !branch.state_mask.is_bit_set(nibble) {
                    break;
                }
                let index = (0..nibble)
                    .filter(|n| branch.state_mask.is_bit_set(*n))
                    .count();
                reference = branch.stack.get(index).ok_or_else(invalid)?.clone();
                position += 1;
            }
            TrieNode::Extension(extension) => {
                if extension.key.is_empty() {
                    return Err(invalid());
                }
                if !extension
                    .key
                    .iter()
                    .enumerate()
                    .all(|(i, nibble)| key.get(position + i) == Some(nibble))
                {
                    break;
                }
                position += extension.key.len();
                reference = extension.child;
            }
            TrieNode::Leaf(_) => break,
            TrieNode::EmptyRoot => return Err(invalid()),
        }
    }
    if next != nodes.len() {
        return Err(invalid());
    }
    Ok(result)
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("response limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_response(
    proofs: &[EIP1186AccountProofResponse],
    limit: usize,
) -> Result<Box<RawValue>, ErrorObjectOwned> {
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, proofs).map_err(|_| resource("response_too_large"))?;
    let json = String::from_utf8(writer.bytes)
        .map_err(|_| failure("proof_invariant", "Invalid proof serialization"))?;
    RawValue::from_string(json)
        .map_err(|_| failure("proof_invariant", "Invalid proof serialization"))
}

/// One adapter instance shares its admission budget across all selected transports.
#[derive(Clone, Debug)]
pub struct DogeosMultiProofApi<Eth> {
    eth: Eth,
    limits: MultiProofLimits,
    admission: Arc<Semaphore>,
}

impl<Eth> DogeosMultiProofApi<Eth> {
    pub fn new(eth: Eth, limits: MultiProofLimits) -> Self {
        let limits = limits.bounded();
        Self {
            eth,
            admission: Arc::new(Semaphore::new(limits.max_jobs)),
            limits,
        }
    }
}

impl<Eth> DogeosMultiProofApi<Eth>
where
    Eth: RpcNodeCore + EthState + SpawnBlocking,
    Eth::Provider: MultiProofProvider,
    Eth::Error: Into<ErrorObjectOwned>,
{
    pub fn into_rpc(self) -> Result<RpcModule<Self>, jsonrpsee::core::RegisterMethodError> {
        let mut module = RpcModule::new(self);
        module.register_async_method("dogeos_getProofs", |params, api, _| async move {
            let request = parse_request(&params, &api.limits)?;
            tokio::time::timeout(api.limits.deadline, api.get_proofs(request))
                .await
                .map_err(|_| resource("deadline"))?
        })?;
        Ok(module)
    }

    async fn get_proofs(
        &self,
        request: GetProofsRequest,
    ) -> Result<Box<RawValue>, ErrorObjectOwned> {
        let limit = self.limits.max_response_bytes;
        admit_and_spawn(
            self.admission.clone(),
            self.eth.acquire_owned_tracing(),
            self.limits.shared_permit_wait,
            |permits| async move {
                self.eth
                    .spawn_blocking_io_fut(move |eth| async move {
                        // The owned guards live in the detached worker, not the cancelled caller.
                        let _permits = permits;
                        let result = (|| {
                            let (root, state) = eth
                                .provider()
                                .proof_snapshot(request.block_hash, eth.max_proof_window())?;
                            let proofs = build_proofs(state.as_ref(), &request, root)?;
                            serialize_response(&proofs, limit)
                        })();
                        Ok(result)
                    })
                    .await
                    .map_err(Into::into)?
            },
        )
        .await
    }
}

struct JobPermits {
    _endpoint: OwnedSemaphorePermit,
    _shared: OwnedSemaphorePermit,
}

async fn admit_and_spawn<R, F, Fut>(
    admission: Arc<Semaphore>,
    shared: impl Future<Output = Result<OwnedSemaphorePermit, AcquireError>>,
    wait: Duration,
    spawn: F,
) -> Result<R, ErrorObjectOwned>
where
    F: FnOnce(JobPermits) -> Fut,
    Fut: Future<Output = Result<R, ErrorObjectOwned>>,
{
    let endpoint = admission
        .try_acquire_owned()
        .map_err(|_| resource("busy"))?;
    let shared = tokio::time::timeout(wait, shared)
        .await
        .map_err(|_| resource("busy"))?
        .map_err(|_| resource("busy"))?;
    spawn(JobPermits {
        _endpoint: endpoint,
        _shared: shared,
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn request() -> GetProofsRequest {
        GetProofsRequest {
            block_hash: B256::repeat_byte(42),
            targets: (1..=4)
                .map(|n| ProofTarget {
                    address: Address::repeat_byte(n),
                    storage_keys: match n {
                        1 => (9..=12).map(B256::with_last_byte).collect(),
                        2 => vec![B256::ZERO],
                        3 => vec![B256::ZERO, B256::with_last_byte(1)],
                        _ => vec![B256::with_last_byte(123)],
                    },
                })
                .collect(),
        }
    }

    fn kind(error: &ErrorObjectOwned) -> String {
        serde_json::from_str::<serde_json::Value>(error.data().unwrap().get()).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn exact_tuple_schema_and_raw_limit() {
        let request = request();
        let json = serde_json::to_string(&(&request,)).unwrap();
        let exact = MultiProofLimits {
            max_params_bytes: json.len(),
            ..Default::default()
        };
        assert_eq!(
            parse_request(&Params::new(Some(&json)), &exact).unwrap(),
            request
        );
        let too_small = MultiProofLimits {
            max_params_bytes: json.len() - 1,
            ..exact
        };
        assert_eq!(
            parse_request(&Params::new(Some(&json)), &too_small)
                .unwrap_err()
                .code(),
            -32602
        );
        // Raw byte cap takes precedence over deserializing malformed/oversized values.
        let malformed = "[".repeat(65 * 1024);
        assert_eq!(
            parse_request(&Params::new(Some(&malformed)), &MultiProofLimits::default())
                .unwrap_err()
                .message(),
            "Multiproof params too large"
        );
        let object = serde_json::to_string(&request).unwrap();
        assert!(parse_request(&Params::new(Some(&object)), &MultiProofLimits::default()).is_err());
        for invalid in [
            "[]",
            "[{},{}]",
            "[{\"blockHash\":\"latest\",\"targets\":[]}]",
            "null",
        ] {
            assert_eq!(
                parse_request(&Params::new(Some(invalid)), &MultiProofLimits::default())
                    .unwrap_err()
                    .code(),
                -32602
            );
        }
        let mut value = serde_json::to_value((&request,)).unwrap();
        value[0]["unknown"] = true.into();
        let json = value.to_string();
        assert!(parse_request(&Params::new(Some(&json)), &MultiProofLimits::default()).is_err());
    }

    #[test]
    fn counts_duplicates_and_zero_slots() {
        let original = request();
        original.validate().unwrap();
        let mut invalids = Vec::new();
        let mut item = original.clone();
        item.targets.clear();
        invalids.push(item);
        let mut item = original.clone();
        item.targets.push(ProofTarget {
            address: Address::ZERO,
            storage_keys: Vec::new(),
        });
        invalids.push(item);
        let mut item = original.clone();
        item.targets[1].address = item.targets[0].address;
        invalids.push(item);
        let mut item = original.clone();
        item.targets[0].storage_keys.push(B256::with_last_byte(13));
        invalids.push(item);
        let mut item = original.clone();
        item.targets[1].storage_keys.push(B256::with_last_byte(1));
        invalids.push(item);
        let mut item = original.clone();
        item.targets[0].storage_keys[1] = item.targets[0].storage_keys[0];
        invalids.push(item);
        for invalid in invalids {
            assert_eq!(invalid.validate().unwrap_err().code(), -32602);
        }
        let mut empty = original;
        empty.targets[0].storage_keys.clear();
        empty.validate().unwrap();
    }

    #[test]
    fn serialized_result_cap_is_exact_and_never_truncates() {
        let response = vec![EIP1186AccountProofResponse::default()];
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serialize_response(&response, json.len()).unwrap().get(),
            json
        );
        let error = serialize_response(&response, json.len() - 1).unwrap_err();
        assert_eq!(error.code(), -32005);
        assert_eq!(kind(&error), "response_too_large");
        assert_eq!(serialize_response(&[], 2).unwrap().get(), "[]");
    }

    #[tokio::test(start_paused = true)]
    async fn shared_wait_is_bounded_and_never_schedules_on_failure() {
        let admission = Arc::new(Semaphore::new(2));
        let shared = Arc::new(Semaphore::new(0));
        let error = admit_and_spawn(
            admission.clone(),
            shared.acquire_owned(),
            Duration::from_secs(1),
            |_| async {
                panic!("worker must not be scheduled");
                #[allow(unreachable_code)]
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(kind(&error), "busy");
        assert_eq!(admission.available_permits(), 2);
    }

    #[tokio::test]
    async fn cancelled_caller_keeps_detached_worker_admitted() {
        let admission = Arc::new(Semaphore::new(1));
        let shared = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let (exited_tx, exited_rx) = oneshot::channel();
        let caller = tokio::spawn(admit_and_spawn(
            admission.clone(),
            shared.clone().acquire_owned(),
            Duration::from_secs(1),
            |permits| async move {
                // Match SpawnBlocking's detached worker + result channel boundary.
                let (tx, rx) = oneshot::channel();
                tokio::spawn(async move {
                    let _permits = permits;
                    started_tx.send(()).unwrap();
                    finish_rx.await.unwrap();
                    drop(_permits);
                    let _ = tx.send(());
                    exited_tx.send(()).unwrap();
                });
                rx.await.map_err(|_| resource("deadline"))
            },
        ));
        started_rx.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(admission.available_permits(), 0);
        assert_eq!(shared.available_permits(), 0);
        let error = admit_and_spawn(
            admission.clone(),
            shared.clone().acquire_owned(),
            Duration::from_secs(1),
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert_eq!(kind(&error), "busy");
        finish_tx.send(()).unwrap();
        exited_rx.await.unwrap();
        assert_eq!(admission.available_permits(), 1);
        assert_eq!(shared.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_cancels_admission_wait_and_releases_endpoint() {
        let admission = Arc::new(Semaphore::new(2));
        let shared = Arc::new(Semaphore::new(0));
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            admit_and_spawn(
                admission.clone(),
                shared.acquire_owned(),
                Duration::from_secs(1),
                |_| async { Ok(()) },
            ),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(admission.available_permits(), 2);
    }
    #[tokio::test(start_paused = true)]
    async fn thirty_second_deadline_retains_running_worker_permits() {
        let admission = Arc::new(Semaphore::new(2));
        let shared = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let (exited_tx, exited_rx) = oneshot::channel();
        let endpoint = admission.clone();
        let guard = shared.clone();
        let caller = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_secs(30),
                admit_and_spawn(
                    endpoint,
                    guard.acquire_owned(),
                    Duration::from_secs(1),
                    |permits| async move {
                        let (tx, rx) = oneshot::channel();
                        tokio::spawn(async move {
                            let _permits = permits;
                            started_tx.send(()).unwrap();
                            finish_rx.await.unwrap();
                            drop(_permits);
                            let _ = tx.send(());
                            exited_tx.send(()).unwrap();
                        });
                        rx.await.map_err(|_| resource("deadline"))
                    },
                ),
            )
            .await
            .map_err(|_| resource("deadline"))?
        });
        started_rx.await.unwrap();
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(kind(&caller.await.unwrap().unwrap_err()), "deadline");
        assert_eq!(admission.available_permits(), 1);
        assert_eq!(shared.available_permits(), 0);
        finish_tx.send(()).unwrap();
        exited_rx.await.unwrap();
        assert_eq!(admission.available_permits(), 2);
        assert_eq!(shared.available_permits(), 1);
    }

    #[tokio::test]
    async fn at_most_two_waiters_and_cancelled_waiters_release_admission() {
        let admission = Arc::new(Semaphore::new(2));
        let shared = Arc::new(Semaphore::new(0));
        let mut callers = Vec::new();
        for _ in 0..2 {
            callers.push(tokio::spawn(admit_and_spawn(
                admission.clone(),
                shared.clone().acquire_owned(),
                Duration::from_secs(1),
                |_| async {
                    panic!("no shared permit");
                    #[allow(unreachable_code)]
                    Ok(())
                },
            )));
        }
        tokio::task::yield_now().await;
        assert_eq!(admission.available_permits(), 0);
        let error = admit_and_spawn(
            admission.clone(),
            shared.acquire_owned(),
            Duration::from_secs(1),
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert_eq!(kind(&error), "busy");
        for caller in callers {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        }
        assert_eq!(admission.available_permits(), 2);
    }
}

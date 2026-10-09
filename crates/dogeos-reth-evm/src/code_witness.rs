//! Host-side policy metering for contract bytecode in a candidate block.
//!
//! Loaded code is deduplicated by hash. Each EIP-7702 authorization additionally reserves 23 bytes
//! for an old delegation marker that pre-execution may overwrite before inspector hooks run. This
//! is a conservative allowance, including duplicate/invalid authorizations, not an exact witness
//! serializer. Reservations accumulate across accepted transactions and roll back with rejection.
//!
//! This inspector deliberately does not change consensus execution. When a policy limit is hit it
//! stops the interpreter with an ordinary halt, and the caller **must discard the entire transaction
//! result** after checking [`CodeWitnessHandle::exceeded`]. It must never commit that synthetic halt.

use alloy_primitives::{Address, B256};
use revm::{
    Inspector,
    bytecode::opcode,
    context_interface::{ContextTr, Transaction},
    inspector::JournalExt,
    interpreter::{
        CallInputs, CallOutcome, CreateInputs, CreateOutcome, InstructionResult, Interpreter,
        interpreter_types::{Jumps, LoopControl},
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

/// Initial operator policy, not a measured prover-capacity guarantee or a consensus limit.
pub const DEFAULT_MAX_CODE_WITNESS_BYTES: u64 = 32 * 1024 * 1024;

/// The policy that interrupted execution; work exhaustion does not prove code overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeWitnessLimit {
    /// The union of accepted and pending code exceeds the block code budget.
    CodeBytes,
    /// This transaction exhausted its opcode execution allowance.
    ExecutionSteps,
}

#[derive(Debug)]
struct Meter {
    max_code_bytes: u64,
    max_steps: u64,
    accepted: HashMap<B256, u64>,
    pending: HashMap<B256, u64>,
    accepted_bytes: u64,
    pending_bytes: u64,
    transaction_bytes: u64,
    steps: u64,
    exceeded: Option<CodeWitnessLimit>,
    initialized: bool,
}

impl Meter {
    fn reserve(&mut self, bytes: u64) {
        self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        self.transaction_bytes = self.transaction_bytes.saturating_add(bytes);
        if self.accepted_bytes.saturating_add(self.pending_bytes) > self.max_code_bytes {
            self.exceeded.get_or_insert(CodeWitnessLimit::CodeBytes);
        }
    }

    fn record(&mut self, hash: B256, bytes: usize) {
        if bytes == 0 || self.pending.contains_key(&hash) {
            return;
        }
        let bytes = bytes as u64;
        self.pending.insert(hash, bytes);
        self.transaction_bytes = self.transaction_bytes.saturating_add(bytes);
        if !self.accepted.contains_key(&hash) {
            self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        }
        if self.accepted_bytes.saturating_add(self.pending_bytes) > self.max_code_bytes {
            self.exceeded.get_or_insert(CodeWitnessLimit::CodeBytes);
        }
    }

    fn reset_transaction(&mut self) {
        self.pending.clear();
        self.pending_bytes = 0;
        self.transaction_bytes = 0;
        self.steps = 0;
        self.exceeded = None;
        self.initialized = false;
    }
}

/// Shared transaction checkpoint and block-level code union, independent of database caches.
#[derive(Debug, Clone)]
pub struct CodeWitnessHandle(Arc<Mutex<Meter>>);

impl CodeWitnessHandle {
    /// Creates an empty block meter. Use `u64::MAX` for an effectively unlimited step allowance.
    pub fn new(max_code_bytes: u64, max_steps: u64) -> Self {
        Self(Arc::new(Mutex::new(Meter {
            max_code_bytes,
            max_steps,
            accepted: HashMap::new(),
            pending: HashMap::new(),
            accepted_bytes: 0,
            pending_bytes: 0,
            transaction_bytes: 0,
            steps: 0,
            exceeded: None,
            initialized: false,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Meter> {
        self.0.lock().expect("code witness meter lock poisoned")
    }

    /// Creates the EVM inspector sharing this handle's transaction and block accounting.
    pub fn inspector(&self) -> CodeWitnessInspector {
        CodeWitnessInspector {
            handle: self.clone(),
            pending_access: None,
        }
    }

    /// Starts a candidate transaction, preserving only previously accepted code.
    pub fn begin_transaction(&self) {
        self.lock().reset_transaction();
    }

    /// Accepts the pending union. Returns false if execution exceeded a policy limit.
    pub fn accept_transaction(&self) -> bool {
        let mut meter = self.lock();
        if meter.exceeded.is_some() {
            return false;
        }
        let pending = std::mem::take(&mut meter.pending);
        meter.accepted.extend(pending);
        meter.accepted_bytes = meter.accepted_bytes.saturating_add(meter.pending_bytes);
        meter.pending_bytes = 0;
        true
    }

    /// Drops candidate accounting. Database cache contents do not affect the next transaction.
    pub fn reject_transaction(&self) {
        self.lock().reset_transaction();
    }

    /// Returns the reason execution must be discarded, if any.
    pub fn exceeded(&self) -> Option<CodeWitnessLimit> {
        self.lock().exceeded
    }
    /// Returns this transaction's distinct code bytes plus its conservative authorization reserve.
    pub fn transaction_code_bytes(&self) -> u64 {
        self.lock().transaction_bytes
    }
    /// Returns accepted code union bytes plus accumulated authorization reserves.
    pub fn block_code_bytes(&self) -> u64 {
        self.lock().accepted_bytes
    }
    /// Returns accepted plus newly accessed candidate code bytes.
    pub fn candidate_code_bytes(&self) -> u64 {
        let meter = self.lock();
        meter.accepted_bytes.saturating_add(meter.pending_bytes)
    }
    /// Returns opcodes executed in this transaction.
    pub fn steps(&self) -> u64 {
        self.lock().steps
    }
}

/// Observes already-loaded journal code without extra database reads or opcode-level state scans.
#[derive(Debug, Clone)]
pub struct CodeWitnessInspector {
    handle: CodeWitnessHandle,
    pending_access: Option<(Address, bool)>,
}

impl CodeWitnessInspector {
    fn initial_codes<CTX: ContextTr<Journal: JournalExt>>(&self, context: &CTX) {
        let mut meter = self.handle.lock();
        if meter.initialized {
            return;
        }
        meter.initialized = true;
        // A valid authorization can replace only empty code or a 23-byte delegation marker.
        // The old marker is no longer visible below; reserve its maximum size without DB reads.
        meter.reserve((context.tx().authorization_list_len() as u64).saturating_mul(23));
        // Once per transaction, cover codes loaded by sender/auth validation before frame entry.
        // The journal is transaction-local; the State database cache is deliberately not scanned.
        for account in context.journal().evm_state().values() {
            if let Some(code) = &account.info.code {
                meter.record(account.info.code_hash, code.original_byte_slice().len());
            }
        }
    }

    fn account_code<CTX: ContextTr<Journal: JournalExt>>(
        &self,
        context: &CTX,
        address: Address,
        delegated: bool,
    ) {
        let state = context.journal().evm_state();
        let Some(account) = state.get(&address) else {
            return;
        };
        let Some(code) = &account.info.code else {
            return;
        };
        let mut meter = self.handle.lock();
        meter.record(account.info.code_hash, code.original_byte_slice().len());
        if delegated
            && let Some(account) = code
                .eip7702_address()
                .and_then(|address| state.get(&address))
            && let Some(code) = &account.info.code
        {
            meter.record(account.info.code_hash, code.original_byte_slice().len());
        }
    }

    fn stop_if_exceeded(&self, interp: &mut Interpreter) {
        if self.handle.exceeded().is_some() {
            Self::stop(interp);
        }
    }

    fn stop(interp: &mut Interpreter) {
        // An opcode may already have queued a CALL/RETURN, or initialize_interp may have halted.
        // Replace that action rather than trying to enqueue a second interpreter action.
        interp.bytecode.action = None;
        interp.bytecode.reset_action();
        interp.halt(InstructionResult::OutOfGas);
    }
}

impl<CTX: ContextTr<Journal: JournalExt>> Inspector<CTX> for CodeWitnessInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        self.initial_codes(context);
        if let Some(address) = interp.input.bytecode_address {
            self.account_code(context, address, true);
            // known_bytecode may refer to delegated execution rather than the target account.
            let hash = interp.bytecode.get_or_calculate_hash();
            self.handle
                .lock()
                .record(hash, interp.bytecode.original_byte_slice().len());
        }
        self.stop_if_exceeded(interp);
    }

    fn step(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        let mut meter = self.handle.lock();
        if meter.steps >= meter.max_steps {
            meter
                .exceeded
                .get_or_insert(CodeWitnessLimit::ExecutionSteps);
        }
        if meter.exceeded.is_some() {
            Self::stop(interp);
            return;
        }
        meter.steps = meter.steps.saturating_add(1);
        self.pending_access = match interp.bytecode.opcode() {
            opcode::EXTCODESIZE | opcode::EXTCODECOPY => interp
                .stack
                .peek(0)
                .ok()
                .map(|v| (Address::from_word(B256::from(v)), false)),
            opcode::CALL | opcode::CALLCODE | opcode::DELEGATECALL | opcode::STATICCALL => interp
                .stack
                .peek(1)
                .ok()
                .map(|v| (Address::from_word(B256::from(v)), true)),
            _ => None,
        };
    }

    fn step_end(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        if let Some((address, delegated)) = self.pending_access.take() {
            self.account_code(context, address, delegated);
        }
        self.stop_if_exceeded(interp);
    }

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        self.initial_codes(context);
        self.account_code(context, inputs.bytecode_address, true);
        None
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, _outcome: &mut CallOutcome) {
        // Includes step-less calls and frame-creation failures after loading a code.
        self.account_code(context, inputs.bytecode_address, true);
    }

    fn create(&mut self, context: &mut CTX, _inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.initial_codes(context);
        None
    }

    fn create_end(
        &mut self,
        context: &mut CTX,
        _inputs: &CreateInputs,
        outcome: &mut CreateOutcome,
    ) {
        if outcome.result.result.is_ok()
            && let Some(address) = outcome.address
        {
            self.account_code(context, address, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollDefaultPrecompilesFactory, ScrollEvmFactory, ScrollTransactionIntoTxEnv};
    use alloy_evm::{Evm, EvmEnv, EvmFactory};
    use alloy_primitives::{Bytes, TxKind, U256};
    use revm::{
        context::{BlockEnv, CfgEnv, TxEnv, result::ExecutionResult},
        database::InMemoryDB,
        state::{AccountInfo, Bytecode},
    };
    use revm_scroll::ScrollSpecId;

    fn address(last: u8) -> Address {
        Address::with_last_byte(last)
    }

    fn insert(db: &mut InMemoryDB, address: Address, bytes: Vec<u8>) {
        let code = Bytecode::new_raw(bytes.into());
        db.insert_account_info(
            address,
            AccountInfo::new(U256::ZERO, 1, code.hash_slow(), code),
        );
    }

    fn extcode(address: Address, opcode: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        if opcode == opcode::EXTCODECOPY {
            // Zero length must still charge the entire loaded code.
            bytes.extend([opcode::PUSH0, opcode::PUSH0, opcode::PUSH0]);
        }
        bytes.push(opcode::PUSH20);
        bytes.extend_from_slice(address.as_slice());
        bytes.push(opcode);
        if opcode != opcode::EXTCODECOPY {
            bytes.push(opcode::POP);
        }
        bytes
    }

    fn run(db: &mut InMemoryDB, handle: &CodeWitnessHandle, target: Address) -> ExecutionResult {
        handle.begin_transaction();
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default()
            .create_evm_with_inspector(
                db,
                EvmEnv::new(
                    CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                    BlockEnv::default(),
                ),
                handle.inspector(),
            );
        evm.transact_raw(ScrollTransactionIntoTxEnv::new(
            TxEnv::builder()
                .caller(address(0xf0))
                .kind(TxKind::Call(target))
                .gas_limit(1_000_000)
                .build()
                .unwrap(),
            Some(Bytes::new()),
            Some(U256::ONE),
            Some(0),
        ))
        .unwrap()
        .result
    }

    #[test]
    fn code_witness_extcode_size_copy_hash_and_revert() {
        for op in [
            opcode::EXTCODESIZE,
            opcode::EXTCODECOPY,
            opcode::EXTCODEHASH,
        ] {
            let mut db = InMemoryDB::default();
            let target = address(0x80);
            let entry = address(0x81);
            insert(&mut db, target, vec![opcode::STOP; 1000]);
            let mut code = extcode(target, op);
            code.extend(extcode(target, op));
            code.extend([opcode::PUSH0, opcode::PUSH0, opcode::REVERT]);
            let expected = code.len() as u64 + if op == opcode::EXTCODEHASH { 0 } else { 1000 };
            insert(&mut db, entry, code);
            let handle = CodeWitnessHandle::new(10_000, u64::MAX);
            assert!(matches!(
                run(&mut db, &handle, entry),
                ExecutionResult::Revert { .. }
            ));
            assert_eq!(handle.transaction_code_bytes(), expected);
            assert_eq!(handle.exceeded(), None);
            assert!(handle.accept_transaction());
            assert_eq!(handle.block_code_bytes(), expected);
        }
    }

    #[test]
    fn code_witness_cache_hits_and_rejected_candidates_do_not_bypass_union() {
        let mut db = InMemoryDB::default();
        let target = address(0x80);
        let entry = address(0x81);
        insert(&mut db, target, vec![opcode::STOP; 1000]);
        let code = extcode(target, opcode::EXTCODESIZE);
        let size = code.len() as u64 + 1000;
        insert(&mut db, entry, code);
        let handle = CodeWitnessHandle::new(size - 1, u64::MAX);
        for _ in 0..2 {
            run(&mut db, &handle, entry);
            assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::CodeBytes));
            assert_eq!(handle.transaction_code_bytes(), size);
            assert!(!handle.accept_transaction());
            handle.reject_transaction();
            assert_eq!(handle.block_code_bytes(), 0);
        }
        let handle = CodeWitnessHandle::new(size, u64::MAX);
        for _ in 0..2 {
            run(&mut db, &handle, entry);
            assert_eq!(handle.exceeded(), None);
            assert_eq!(handle.transaction_code_bytes(), size);
            assert!(handle.accept_transaction());
            assert_eq!(handle.block_code_bytes(), size);
        }
    }

    #[test]
    fn code_witness_split_transactions_and_step_exhaustion() {
        let mut db = InMemoryDB::default();
        insert(&mut db, address(0x80), vec![opcode::STOP; 100]);
        insert(&mut db, address(0x81), vec![opcode::STOP; 101]);
        let handle = CodeWitnessHandle::new(150, u64::MAX);
        run(&mut db, &handle, address(0x80));
        assert!(handle.accept_transaction());
        run(&mut db, &handle, address(0x81));
        assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::CodeBytes));
        assert_eq!(handle.transaction_code_bytes(), 101);
        assert_eq!(handle.candidate_code_bytes(), 201);
        handle.reject_transaction();
        assert_eq!(handle.block_code_bytes(), 100);

        // Infinite loop is interrupted at the work allowance, independently of code size.
        insert(
            &mut db,
            address(0x82),
            vec![opcode::JUMPDEST, opcode::PUSH0, opcode::JUMP],
        );
        let handle = CodeWitnessHandle::new(1000, 7);
        run(&mut db, &handle, address(0x82));
        assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::ExecutionSteps));
        assert_eq!(handle.steps(), 7);
        assert_eq!(handle.transaction_code_bytes(), 3);
    }

    #[test]
    fn code_witness_delegated_call_counts_marker_and_target() {
        let mut db = InMemoryDB::default();
        let delegation = Bytecode::new_eip7702(address(0x80));
        insert(&mut db, address(0x80), vec![opcode::STOP; 100]);
        db.insert_account_info(
            address(0x81),
            AccountInfo::new(U256::ZERO, 1, delegation.hash_slow(), delegation),
        );
        let handle = CodeWitnessHandle::new(1000, u64::MAX);
        run(&mut db, &handle, address(0x81));
        assert_eq!(handle.exceeded(), None);
        assert_eq!(handle.transaction_code_bytes(), 123);
    }

    #[test]
    fn code_witness_replaced_authorization_markers_have_transactional_reserve() {
        use revm::context::{
            either::Either,
            transaction::{Authorization, RecoveredAuthority, RecoveredAuthorization},
        };

        let mut db = InMemoryDB::default();
        let old_marker = Bytecode::new_eip7702(address(0x83));
        db.insert_account_info(
            address(0x81),
            AccountInfo::new(U256::ZERO, 1, old_marker.hash_slow(), old_marker),
        );
        insert(&mut db, address(0x80), vec![opcode::STOP; 100]);
        let tx = TxEnv::builder()
            .caller(address(0xf0))
            .kind(TxKind::Call(address(0x81)))
            .gas_limit(1_000_000)
            .authorization_list(vec![Either::Right(RecoveredAuthorization::new_unchecked(
                Authorization {
                    chain_id: U256::ZERO,
                    address: address(0x80),
                    nonce: 1,
                },
                RecoveredAuthority::Valid(address(0x81)),
            ))])
            .build()
            .unwrap();
        let mut simulate = |handle: &CodeWitnessHandle| {
            handle.begin_transaction();
            let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default()
                .create_evm_with_inspector(
                    &mut db,
                    EvmEnv::new(
                        CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                        BlockEnv::default(),
                    ),
                    handle.inspector(),
                );
            evm.transact_raw(ScrollTransactionIntoTxEnv::new(
                tx.clone(),
                Some(Bytes::new()),
                Some(U256::ONE),
                Some(0),
            ))
            .unwrap()
        };

        // 100 runtime + 23 new marker + 23 old-marker allowance, even though the old code is
        // already overwritten before the first call hook. The state result proves replacement.
        let handle = CodeWitnessHandle::new(146, u64::MAX);
        let output = simulate(&handle);
        assert!(output.result.is_success());
        assert_eq!(
            output.state[&address(0x81)]
                .info
                .code
                .as_ref()
                .unwrap()
                .eip7702_address(),
            Some(address(0x80))
        );
        assert_eq!(handle.transaction_code_bytes(), 146);
        assert!(handle.accept_transaction());
        assert_eq!(handle.block_code_bytes(), 146);
        simulate(&handle);
        assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::CodeBytes));
        assert_eq!(handle.candidate_code_bytes(), 169);
        handle.reject_transaction();
        assert_eq!(handle.block_code_bytes(), 146);
        assert_eq!(handle.candidate_code_bytes(), 146);

        let handle = CodeWitnessHandle::new(145, u64::MAX);
        simulate(&handle);
        assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::CodeBytes));
        assert!(!handle.accept_transaction());
        handle.reject_transaction();
        assert_eq!(handle.candidate_code_bytes(), 0);
    }

    #[test]
    fn code_witness_nested_revert_and_early_call_abort() {
        let mut db = InMemoryDB::default();
        let mut child = extcode(address(0x80), opcode::EXTCODECOPY);
        child.extend([opcode::PUSH0, opcode::PUSH0, opcode::REVERT]);
        let child_len = child.len() as u64;
        insert(&mut db, address(0x80), vec![opcode::STOP; 1000]);
        insert(&mut db, address(0x81), child);
        let mut parent = vec![opcode::PUSH0; 5];
        parent.push(opcode::PUSH20);
        parent.extend_from_slice(address(0x81).as_slice());
        parent.extend([
            opcode::PUSH3,
            0x01,
            0x00,
            0x00,
            opcode::CALL,
            opcode::POP,
            opcode::STOP,
        ]);
        let parent_len = parent.len() as u64;
        insert(&mut db, address(0x82), parent);

        let handle = CodeWitnessHandle::new(10_000, u64::MAX);
        assert!(run(&mut db, &handle, address(0x82)).is_success());
        assert_eq!(
            handle.transaction_code_bytes(),
            parent_len + child_len + 1000
        );

        // The CALL opcode has queued a frame when its code loads exceed the limit. Replacing
        // that action must safely halt before executing the child rather than panic or bypass it.
        let handle = CodeWitnessHandle::new(parent_len, u64::MAX);
        run(&mut db, &handle, address(0x82));
        assert_eq!(handle.exceeded(), Some(CodeWitnessLimit::CodeBytes));
        assert_eq!(handle.transaction_code_bytes(), parent_len + child_len);
    }

    #[test]
    fn code_witness_created_runtime_and_system_calls() {
        let mut db = InMemoryDB::default();
        let handle = CodeWitnessHandle::new(1000, u64::MAX);
        handle.begin_transaction();
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default()
            .create_evm_with_inspector(
                &mut db,
                EvmEnv::new(
                    CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                    BlockEnv::default(),
                ),
                handle.inspector(),
            );
        // Initcode returns 100 zero bytes. Initcode comes from transaction data, while the
        // returned runtime code is recorded as created code for the block policy.
        let tx = TxEnv::builder()
            .caller(address(0xf0))
            .kind(TxKind::Create)
            .gas_limit(1_000_000)
            .data(Bytes::from(vec![
                opcode::PUSH1,
                100,
                opcode::PUSH0,
                opcode::RETURN,
            ]))
            .build()
            .unwrap();
        assert!(
            evm.transact_raw(ScrollTransactionIntoTxEnv::new(
                tx,
                Some(Bytes::new()),
                Some(U256::ONE),
                Some(0),
            ))
            .unwrap()
            .result
            .is_success()
        );
        assert_eq!(handle.transaction_code_bytes(), 100);
        assert!(handle.accept_transaction());
        drop(evm);

        insert(&mut db, address(0x80), vec![opcode::STOP; 101]);
        handle.begin_transaction();
        let mut evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default()
            .create_evm_with_inspector(
                &mut db,
                EvmEnv::new(
                    CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                    BlockEnv::default(),
                ),
                handle.inspector(),
            );
        assert!(
            evm.transact_system_call(address(0xff), address(0x80), Bytes::new())
                .unwrap()
                .result
                .is_success()
        );
        assert_eq!(handle.transaction_code_bytes(), 101);
        assert!(handle.accept_transaction());
        assert_eq!(handle.block_code_bytes(), 201);
    }
}

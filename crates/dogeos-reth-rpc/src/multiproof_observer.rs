//! Optional fixed-cardinality proof attribution. No clocks are read without an observer.

use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering::{AcqRel, Acquire, Relaxed},
        },
    },
    time::{Duration, Instant},
};

/// Stable attribution boundaries; enclosing stages overlap their children.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum ProofStage {
    Request,
    SharedAdmissionWait,
    WorkerDispatchWait,
    WorkerService,
    Snapshot,
    ProviderProofReconstruction,
    AccountExtraction,
    AccountVerification,
    AccountConversion,
    Serialization,
    /// Fixture-only paired baseline: one ordinary `state.proof` call.
    OrdinaryProofReconstruction,
}

impl ProofStage {
    const ALL: [Self; 11] = [
        Self::Request,
        Self::SharedAdmissionWait,
        Self::WorkerDispatchWait,
        Self::WorkerService,
        Self::Snapshot,
        Self::ProviderProofReconstruction,
        Self::AccountExtraction,
        Self::AccountVerification,
        Self::AccountConversion,
        Self::Serialization,
        Self::OrdinaryProofReconstruction,
    ];
}

/// Sums for completed observations of one outcome. CPU is unavailable when
/// `thread_cpu_samples == 0`; partial clock availability is explicit in that count.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ProofTiming {
    pub samples: u64,
    pub wall_ns: u64,
    pub thread_cpu_samples: u64,
    pub thread_cpu_ns: u64,
}

/// Concurrent reads are approximate. Take before/after snapshots only after stopping
/// ingress and observing all active counts at zero, including detached workers.
#[derive(Clone, Debug, Serialize)]
pub struct ProofStageSnapshot {
    pub stage: ProofStage,
    pub active: u64,
    pub success: ProofTiming,
    pub error: ProofTiming,
    /// Dropped before a result (caller cancellation, dropped dispatch, or panic).
    pub abandoned: ProofTiming,
}

#[derive(Debug, Default)]
struct Timing {
    samples: AtomicU64,
    wall_ns: AtomicU64,
    cpu_samples: AtomicU64,
    cpu_ns: AtomicU64,
}

impl Timing {
    fn snapshot(&self) -> ProofTiming {
        ProofTiming {
            samples: self.samples.load(Relaxed),
            wall_ns: self.wall_ns.load(Relaxed),
            thread_cpu_samples: self.cpu_samples.load(Relaxed),
            thread_cpu_ns: self.cpu_ns.load(Relaxed),
        }
    }
}

#[derive(Debug, Default)]
struct Stage {
    active: AtomicU64,
    outcomes: [Timing; 3],
}

/// Opt-in observer shared by an adapter and its fixture. Counters never reset.
/// The optional clock must cheaply return monotonic CPU consumed by the calling
/// thread, return `None` when unavailable, and never panic. It is called only around
/// synchronous operations, never across `.await`. Worker CPU excludes other threads.
#[derive(Debug, Default)]
pub struct MultiProofObserver {
    inner: Arc<Inner>,
    thread_cpu_clock: Option<fn() -> Option<Duration>>,
}

#[derive(Debug, Default)]
struct Inner {
    stages: [Stage; 11],
    active: AtomicU64,
    idle: tokio::sync::Notify,
}

impl MultiProofObserver {
    pub fn with_thread_cpu_clock(clock: fn() -> Option<Duration>) -> Self {
        Self {
            thread_cpu_clock: Some(clock),
            ..Self::default()
        }
    }

    pub fn snapshot(&self) -> Vec<ProofStageSnapshot> {
        ProofStage::ALL
            .into_iter()
            .map(|stage| {
                let counters = &self.inner.stages[stage as usize];
                ProofStageSnapshot {
                    stage,
                    active: counters.active.load(Relaxed),
                    success: counters.outcomes[0].snapshot(),
                    error: counters.outcomes[1].snapshot(),
                    abandoned: counters.outcomes[2].snapshot(),
                }
            })
            .collect()
    }

    /// Wait for all observed scopes, including queued and detached shared workers.
    /// The fixture must stop ingress first and impose its own drain timeout. This is
    /// NOT an ordinary RPC worker barrier: ordinary requests are not observed here.
    pub async fn await_idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            if self.inner.active.load(Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Also usable for the fixture's ordinary proof baseline. This measures only the
    /// supplied synchronous operation; callers must use the named stage's boundary.
    pub fn measure<T, E>(
        &self,
        stage: ProofStage,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let mut guard = self.start(stage, true);
        let result = operation();
        guard.finish(result.is_ok());
        result
    }

    pub(crate) fn start(&self, stage: ProofStage, cpu: bool) -> Observation {
        let clock = if cpu { self.thread_cpu_clock } else { None };
        let counters = &self.inner.stages[stage as usize];
        self.inner.active.fetch_add(1, AcqRel);
        counters.active.fetch_add(1, Relaxed);
        Observation {
            inner: self.inner.clone(),
            stage,
            wall: Instant::now(),
            cpu: clock.and_then(|f| f()),
            clock,
            finished: false,
        }
    }
}

pub(crate) fn measure<T, E>(
    observer: Option<&MultiProofObserver>,
    stage: ProofStage,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    match observer {
        Some(observer) => observer.measure(stage, operation),
        None => operation(),
    }
}

pub(crate) struct Observation {
    inner: Arc<Inner>,
    stage: ProofStage,
    wall: Instant,
    cpu: Option<Duration>,
    clock: Option<fn() -> Option<Duration>>,
    finished: bool,
}

impl Observation {
    pub(crate) fn finish(&mut self, success: bool) {
        self.record(if success { 0 } else { 1 });
    }

    fn record(&mut self, outcome: usize) {
        if self.finished {
            return;
        }
        let wall = self.wall.elapsed();
        let cpu = self
            .cpu
            .zip(self.clock.and_then(|f| f()))
            .and_then(|(start, end)| end.checked_sub(start));
        let counters = &self.inner.stages[self.stage as usize];
        let timing = &counters.outcomes[outcome];
        timing.wall_ns.fetch_add(nanos(wall), Relaxed);
        if let Some(cpu) = cpu {
            timing.cpu_ns.fetch_add(nanos(cpu), Relaxed);
            timing.cpu_samples.fetch_add(1, Relaxed);
        }
        timing.samples.fetch_add(1, Relaxed);
        counters.active.fetch_sub(1, Relaxed);
        self.finished = true;
        if self.inner.active.fetch_sub(1, AcqRel) == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        self.record(2);
    }
}

fn nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! { static CPU: Cell<u64> = const { Cell::new(0) }; }
    fn cpu() -> Option<Duration> {
        Some(Duration::from_nanos(CPU.with(|cpu| {
            let value = cpu.get();
            cpu.set(value + 10);
            value
        })))
    }

    #[test]
    fn synchronous_outcomes_cpu_and_nested_counts_are_explicit() {
        let observer = MultiProofObserver::with_thread_cpu_clock(cpu);
        observer
            .measure(ProofStage::WorkerService, || {
                observer.measure(ProofStage::Snapshot, || Ok::<_, ()>(42))
            })
            .unwrap();
        observer
            .measure(ProofStage::Snapshot, || Err::<(), _>("provider error"))
            .unwrap_err();
        let before = observer.snapshot();
        let timing = &before[ProofStage::Snapshot as usize];
        assert_eq!(timing.active, 0);
        assert_eq!(timing.success.samples, 1);
        assert_eq!(timing.error.samples, 1);
        assert_eq!(timing.success.thread_cpu_samples, 1);
        assert_eq!(timing.success.thread_cpu_ns, 10);
        assert_eq!(
            before[ProofStage::WorkerService as usize]
                .success
                .thread_cpu_ns,
            30
        );
        let panic = std::panic::catch_unwind(|| {
            observer.measure(ProofStage::Snapshot, || -> Result<(), ()> {
                panic!("provider panic")
            })
        });
        assert!(panic.is_err());
        let after = observer.snapshot();
        assert_eq!(after.len(), 11);
        assert_eq!(after[ProofStage::Snapshot as usize].abandoned.samples, 1);
        assert_eq!(
            after[ProofStage::Snapshot as usize].success.samples,
            timing.success.samples
        );
        assert!(after.iter().all(|stage| stage.active == 0));
    }

    #[test]
    fn unavailable_cpu_and_disabled_observation_preserve_results() {
        let observer = MultiProofObserver::with_thread_cpu_clock(|| None);
        let error = observer.measure(ProofStage::Snapshot, || Err::<(), _>(123));
        assert_eq!(
            error,
            measure(None, ProofStage::Snapshot, || Err::<(), _>(123))
        );
        let timing = observer.snapshot()[ProofStage::Snapshot as usize].error;
        assert_eq!(timing.samples, 1);
        assert_eq!(timing.thread_cpu_samples, 0);
        assert_eq!(timing.thread_cpu_ns, 0);
        assert_eq!(
            measure(None, ProofStage::Snapshot, || Ok::<_, ()>(42)),
            Ok(42)
        );
    }

    #[tokio::test]
    async fn idle_waits_for_dispatch_and_service_after_caller_cancellation() {
        let observer = Arc::new(MultiProofObserver::with_thread_cpu_clock(|| {
            panic!("async clock")
        }));
        let caller = observer.start(ProofStage::Request, false);
        let mut dispatch = observer.start(ProofStage::WorkerDispatchWait, false);
        drop(caller);
        let idle_observer = observer.clone();
        let idle = tokio::spawn(async move { idle_observer.await_idle().await });
        tokio::task::yield_now().await;
        assert!(!idle.is_finished());
        // Model the production no-gap transition from queued dispatch to worker service.
        let mut service = observer.start(ProofStage::WorkerService, false);
        dispatch.finish(true);
        tokio::task::yield_now().await;
        assert!(!idle.is_finished());
        service.finish(true);
        idle.await.unwrap();
        let snapshot = observer.snapshot();
        assert_eq!(snapshot[ProofStage::Request as usize].abandoned.samples, 1);
        assert_eq!(
            snapshot[ProofStage::WorkerDispatchWait as usize]
                .success
                .samples,
            1
        );
        assert_eq!(
            snapshot[ProofStage::WorkerService as usize].success.samples,
            1
        );
        assert!(
            snapshot
                .iter()
                .all(|stage| stage.active == 0 && stage.success.thread_cpu_samples == 0)
        );
    }

    #[tokio::test]
    async fn discarded_dispatch_wakes_all_idle_waiters() {
        let observer = Arc::new(MultiProofObserver::default());
        let dispatch = observer.start(ProofStage::WorkerDispatchWait, false);
        let mut waiters = Vec::new();
        for _ in 0..2 {
            let observer = observer.clone();
            waiters.push(tokio::spawn(async move { observer.await_idle().await }));
        }
        tokio::task::yield_now().await;
        assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
        drop(dispatch);
        for waiter in waiters {
            waiter.await.unwrap();
        }
        assert_eq!(
            observer.snapshot()[ProofStage::WorkerDispatchWait as usize]
                .abandoned
                .samples,
            1
        );
    }

    #[tokio::test]
    async fn idle_notification_before_first_poll_is_retained() {
        let observer = MultiProofObserver::default();
        let scope = observer.start(ProofStage::Request, false);
        // Exactly await_idle's register/check/wait sequence, with completion in the gap.
        let notified = observer.inner.idle.notified();
        assert_eq!(observer.inner.active.load(Acquire), 1);
        drop(scope);
        tokio::time::timeout(Duration::from_secs(1), notified)
            .await
            .unwrap();
        observer.await_idle().await;
    }
}

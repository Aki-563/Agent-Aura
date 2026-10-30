//! Parallel window workers (HARNESS_COMPLETION_PLAN.md H5).
//!
//! One desktop has one mouse and one keyboard, so "parallel" means this: every window gets its own
//! worker (its own task, history and model conversation), the slow part (the model call) runs for
//! all workers at the same time, and the fast part (looking at the screen, then clicking or typing)
//! takes turns through the **input arbiter**. Nothing a worker sends can interleave with another
//! worker's input, and each turn re-checks the screen under the lock, so a worker never acts on a
//! picture that another worker has already changed.
//!
//! The **model scheduler** caps how many requests are in flight (a small local model serves one at
//! a time; a hosted one can take several) and serves waiting workers in arrival order.
//!
//! The engine here is generic over a [`WorkerBackend`] so its guarantees (no overlapping input,
//! fairness, overlapped model calls, stop, a failing worker not taking the others down) are tested
//! with fakes; `agent::run_window_workers` is the desktop backend.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Mutex, Semaphore};

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What one worker turn decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    Continue,
    Finished { completed: bool, message: String },
}

pub trait WorkerBackend: Send + Sync + 'static {
    type Obs: Send + 'static;
    type Plan: Send + 'static;
    /// Look at the screen for this worker (runs while holding the input arbiter).
    fn observe(&self, worker: usize) -> Fut<'_, Result<Self::Obs, String>>;
    /// Ask the model (runs under the model scheduler; no input is held).
    fn think<'a>(&'a self, worker: usize, obs: &'a Self::Obs) -> Fut<'a, Result<Self::Plan, String>>;
    /// Re-check the screen and act (runs while holding the input arbiter).
    fn act(&self, worker: usize, obs: Self::Obs, plan: Self::Plan) -> Fut<'_, Result<Turn, String>>;
}

/// First come, first served access to the mouse and keyboard. Counts how often two workers would
/// have overlapped (`overlaps` stays 0 by construction; the tests assert it).
#[derive(Default)]
pub struct InputArbiter {
    lock: Mutex<()>,
    inside: AtomicU32,
    pub overlaps: AtomicU32,
    pub waited_ms: AtomicU64,
    pub turns: AtomicU32,
}

impl InputArbiter {
    pub async fn with<T>(&self, work: impl Future<Output = T>) -> T {
        let queued = Instant::now();
        let _guard = self.lock.lock().await; // tokio's mutex queues waiters in order
        self.waited_ms.fetch_add(queued.elapsed().as_millis() as u64, Ordering::SeqCst);
        if self.inside.fetch_add(1, Ordering::SeqCst) != 0 {
            self.overlaps.fetch_add(1, Ordering::SeqCst);
        }
        self.turns.fetch_add(1, Ordering::SeqCst);
        let result = work.await;
        self.inside.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

/// Limits concurrent model requests and records the peak.
pub struct ModelScheduler {
    permits: Semaphore,
    active: AtomicU32,
    pub peak: AtomicU32,
    pub requests: AtomicU32,
}

impl ModelScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self { permits: Semaphore::new(max_concurrent.max(1)), active: AtomicU32::new(0), peak: AtomicU32::new(0), requests: AtomicU32::new(0) }
    }

    pub async fn run<T>(&self, work: impl Future<Output = T>) -> T {
        let _permit = self.permits.acquire().await.expect("scheduler is never closed");
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.requests.fetch_add(1, Ordering::SeqCst);
        let result = work.await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkerResult {
    pub worker: usize,
    pub completed: bool,
    pub steps: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkersReport {
    pub results: Vec<WorkerResult>,
    pub input_overlaps: u32,
    pub input_turns: u32,
    pub input_wait_ms: u64,
    pub model_requests: u32,
    pub model_peak_concurrency: u32,
}

/// Runs `workers` workers to completion (or `max_steps` each, or until `stop` says so).
pub async fn run_workers<B: WorkerBackend>(
    backend: Arc<B>,
    workers: usize,
    max_steps: u32,
    max_concurrent_model_calls: usize,
    stop: Arc<dyn Fn() -> bool + Send + Sync>,
) -> WorkersReport {
    let arbiter = Arc::new(InputArbiter::default());
    let scheduler = Arc::new(ModelScheduler::new(max_concurrent_model_calls));
    let mut handles = Vec::new();
    for worker in 0..workers {
        let (backend, arbiter, scheduler, stop) = (Arc::clone(&backend), Arc::clone(&arbiter), Arc::clone(&scheduler), Arc::clone(&stop));
        handles.push(tokio::spawn(async move {
            let mut steps = 0u32;
            for _ in 0..max_steps {
                if stop() {
                    return WorkerResult { worker, completed: false, steps, message: "stopped".to_owned() };
                }
                let obs = match arbiter.with(backend.observe(worker)).await {
                    Ok(obs) => obs,
                    Err(error) => return WorkerResult { worker, completed: false, steps, message: format!("could not look at the screen: {error}") },
                };
                let plan = match scheduler.run(backend.think(worker, &obs)).await {
                    Ok(plan) => plan,
                    Err(error) => return WorkerResult { worker, completed: false, steps, message: format!("the model step failed: {error}") },
                };
                if stop() {
                    return WorkerResult { worker, completed: false, steps, message: "stopped".to_owned() };
                }
                steps += 1;
                match arbiter.with(backend.act(worker, obs, plan)).await {
                    Ok(Turn::Continue) => {}
                    Ok(Turn::Finished { completed, message }) => return WorkerResult { worker, completed, steps, message },
                    Err(error) => return WorkerResult { worker, completed: false, steps, message: format!("the action failed: {error}") },
                }
            }
            WorkerResult { worker, completed: false, steps, message: format!("reached the {max_steps}-step limit") }
        }));
    }
    let mut results = Vec::new();
    for (worker, handle) in handles.into_iter().enumerate() {
        results.push(handle.await.unwrap_or(WorkerResult { worker, completed: false, steps: 0, message: "the worker crashed".to_owned() }));
    }
    WorkersReport {
        results,
        input_overlaps: arbiter.overlaps.load(Ordering::SeqCst),
        input_turns: arbiter.turns.load(Ordering::SeqCst),
        input_wait_ms: arbiter.waited_ms.load(Ordering::SeqCst),
        model_requests: scheduler.requests.load(Ordering::SeqCst),
        model_peak_concurrency: scheduler.peak.load(Ordering::SeqCst),
    }
}

#[allow(dead_code)]
fn _assert_send(_: Duration) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Each worker needs `steps` turns; thinking takes `think_ms`, acting `act_ms`.
    struct Fake {
        steps: u32,
        think_ms: u64,
        act_ms: u64,
        fail_worker: Option<usize>,
        progress: StdMutex<Vec<u32>>,
        order: StdMutex<Vec<usize>>,
    }

    impl Fake {
        fn new(workers: usize, steps: u32, think_ms: u64, act_ms: u64) -> Self {
            Self { steps, think_ms, act_ms, fail_worker: None, progress: StdMutex::new(vec![0; workers]), order: StdMutex::new(Vec::new()) }
        }
    }

    impl WorkerBackend for Fake {
        type Obs = u32;
        type Plan = ();
        fn observe(&self, worker: usize) -> Fut<'_, Result<u32, String>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok(self.progress.lock().unwrap()[worker])
            })
        }
        fn think<'a>(&'a self, worker: usize, _obs: &'a u32) -> Fut<'a, Result<(), String>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(self.think_ms)).await;
                if self.fail_worker == Some(worker) {
                    return Err("model unavailable".to_owned());
                }
                Ok(())
            })
        }
        fn act(&self, worker: usize, seen: u32, _plan: ()) -> Fut<'_, Result<Turn, String>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(self.act_ms)).await;
                self.order.lock().unwrap().push(worker);
                let mut progress = self.progress.lock().unwrap();
                assert_eq!(progress[worker], seen, "a worker only acts on what it saw");
                progress[worker] += 1;
                Ok(if progress[worker] >= self.steps { Turn::Finished { completed: true, message: "done".to_owned() } } else { Turn::Continue })
            })
        }
    }

    fn never() -> Arc<dyn Fn() -> bool + Send + Sync> {
        Arc::new(|| false)
    }

    #[tokio::test]
    async fn input_never_overlaps_and_every_worker_finishes() {
        let report = run_workers(Arc::new(Fake::new(4, 5, 20, 10)), 4, 20, 4, never()).await;
        assert_eq!(report.input_overlaps, 0);
        assert!(report.results.iter().all(|r| r.completed && r.steps == 5), "{:?}", report.results);
        assert_eq!(report.input_turns, 4 * 5 * 2, "observe and act of every step went through the arbiter");
    }

    #[tokio::test]
    async fn model_calls_overlap_so_four_windows_are_much_faster_than_one_after_another() {
        let started = Instant::now();
        let report = run_workers(Arc::new(Fake::new(4, 4, 80, 5)), 4, 10, 4, never()).await;
        let parallel = started.elapsed();
        assert!(report.model_peak_concurrency >= 3, "peak {}", report.model_peak_concurrency);
        // One after another it would be at least 4 workers x 4 steps x 80 ms = 1280 ms of thinking alone.
        assert!(parallel < Duration::from_millis(900), "took {parallel:?}");
    }

    #[tokio::test]
    async fn the_scheduler_cap_is_respected() {
        let report = run_workers(Arc::new(Fake::new(5, 3, 30, 2)), 5, 10, 2, never()).await;
        assert_eq!(report.model_peak_concurrency, 2);
        assert_eq!(report.model_requests, 15);
    }

    #[tokio::test]
    async fn turns_are_shared_fairly() {
        let fake = Arc::new(Fake::new(3, 6, 10, 10));
        run_workers(Arc::clone(&fake), 3, 20, 3, never()).await;
        let order = fake.order.lock().unwrap().clone();
        // No worker gets three turns in a row while the others are waiting.
        let mut run = 1;
        let mut longest = 1;
        for pair in order.windows(2) {
            run = if pair[0] == pair[1] { run + 1 } else { 1 };
            longest = longest.max(run);
        }
        assert!(longest <= 2, "order {order:?}");
    }

    #[tokio::test]
    async fn one_failing_worker_does_not_take_the_others_down() {
        let mut fake = Fake::new(3, 3, 10, 5);
        fake.fail_worker = Some(1);
        let report = run_workers(Arc::new(fake), 3, 10, 3, never()).await;
        assert!(report.results[0].completed && report.results[2].completed);
        assert!(!report.results[1].completed && report.results[1].message.contains("model unavailable"));
    }

    #[tokio::test]
    async fn stop_ends_every_worker_quickly() {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop: Arc<dyn Fn() -> bool + Send + Sync> = { let flag = Arc::clone(&flag); Arc::new(move || flag.load(Ordering::SeqCst)) };
        let started = Instant::now();
        let handle = tokio::spawn(run_workers(Arc::new(Fake::new(3, 1000, 30, 5)), 3, 1000, 3, stop));
        tokio::time::sleep(Duration::from_millis(150)).await;
        flag.store(true, Ordering::SeqCst);
        let report = handle.await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(600), "took {:?}", started.elapsed());
        assert!(report.results.iter().all(|r| r.message == "stopped"));
        assert_eq!(report.input_overlaps, 0);
    }
}

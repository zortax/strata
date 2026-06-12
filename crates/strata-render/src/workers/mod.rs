//! Worker-thread infrastructure for decode / tessellation jobs.
//!
//! Layers submit closures to the shared [`WorkerPool`] through a typed
//! [`JobQueue`] and drain finished results in `prepare`. Each queue carries a
//! [`Generation`] counter; bumping it (camera moved on, data replaced)
//! invalidates all in-flight jobs — queued-but-not-started jobs are skipped
//! entirely, and results of jobs that had already started are dropped on
//! drain.

use crossbeam_channel::{Receiver, Sender};

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A small fixed-size thread pool. Cloned `Sender`s feed a shared unbounded
/// channel; dropping the pool closes the channel and joins all threads.
pub struct WorkerPool {
    tx: Option<Sender<Job>>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    /// Spawns `threads.max(1)` worker threads.
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        let handles = (0..threads.max(1))
            .map(|i| {
                let rx: Receiver<Job> = rx.clone();
                std::thread::Builder::new()
                    .name(format!("strata-render-worker-{i}"))
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            if catch_unwind(AssertUnwindSafe(job)).is_err() {
                                tracing::error!("render worker job panicked");
                            }
                        }
                    })
                    .unwrap_or_else(|e| panic!("failed to spawn render worker thread: {e}"))
            })
            .collect();
        Self {
            tx: Some(tx),
            handles,
        }
    }

    /// Number of worker threads.
    pub fn thread_count(&self) -> usize {
        self.handles.len()
    }

    /// Run `job` on a worker thread. Prefer [`JobQueue::submit`] so results
    /// flow back with generation filtering.
    pub fn execute(&self, job: impl FnOnce() + Send + 'static) {
        if let Some(tx) = &self.tx {
            // Send only fails when the pool is being torn down.
            let _ = tx.send(Box::new(job));
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.tx = None; // close the channel, workers drain and exit
        for handle in self.handles.drain(..) {
            if handle.join().is_err() {
                tracing::error!("render worker thread panicked during shutdown");
            }
        }
    }
}

/// Monotonic job-validity counter. Results tagged with an older generation
/// than their queue's current one are stale and dropped on drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

/// A typed result queue for one consumer (typically one layer).
pub struct JobQueue<T> {
    tx: Sender<(Generation, T)>,
    rx: Receiver<(Generation, T)>,
    /// Current generation, shared with submitted jobs so superseded jobs can
    /// bail out *before* doing their (potentially expensive) work.
    current: Arc<AtomicU64>,
}

impl<T: Send + 'static> JobQueue<T> {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            current: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn generation(&self) -> Generation {
        Generation(self.current.load(Ordering::Relaxed))
    }

    /// Invalidate all in-flight jobs: queued-but-not-started jobs are
    /// skipped without running; results of already-started jobs are dropped.
    /// Returns the new current generation.
    pub fn invalidate(&mut self) -> Generation {
        Generation(self.current.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Run `job` on the pool; its result is tagged with the *current*
    /// generation and delivered through [`drain`](Self::drain). Jobs whose
    /// generation was invalidated before they started are skipped entirely —
    /// a fast zoom or feed replacement must not grind the FIFO pool through
    /// stale decodes/tessellations ahead of the wanted ones.
    pub fn submit<F>(&self, pool: &WorkerPool, job: F)
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let tx = self.tx.clone();
        let current = Arc::clone(&self.current);
        let generation = self.generation();
        pool.execute(move || {
            if current.load(Ordering::Relaxed) != generation.0 {
                return; // superseded before it started — skip the work
            }
            // Send only fails when the queue owner is gone.
            let _ = tx.send((generation, job()));
        });
    }

    /// All finished, still-current results. Stale results (started before an
    /// invalidation, finished after) are dropped here.
    pub fn drain(&mut self) -> Vec<T> {
        let current = self.generation();
        self.rx
            .try_iter()
            .filter_map(|(generation, value)| (generation == current).then_some(value))
            .collect()
    }
}

impl<T: Send + 'static> Default for JobQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    fn drain_until<T: Send + 'static>(queue: &mut JobQueue<T>, n: usize) -> Vec<T> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < deadline {
            out.extend(queue.drain());
            std::thread::sleep(Duration::from_millis(1));
        }
        out
    }

    #[test]
    fn jobs_run_and_results_drain() {
        let pool = WorkerPool::new(2);
        let mut queue = JobQueue::new();
        for i in 0..8u32 {
            queue.submit(&pool, move || i * 2);
        }
        let mut results = drain_until(&mut queue, 8);
        results.sort_unstable();
        assert_eq!(results, vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn stale_generations_are_dropped() {
        let pool = WorkerPool::new(1);
        let mut queue = JobQueue::new();
        queue.submit(&pool, || "stale");
        queue.invalidate();
        queue.submit(&pool, || "fresh");
        let results = drain_until(&mut queue, 1);
        assert_eq!(results, vec!["fresh"]);
        // Nothing else may arrive.
        std::thread::sleep(Duration::from_millis(20));
        assert!(queue.drain().is_empty());
    }

    /// Invalidation must skip queued jobs *before they start*, not merely
    /// drop their results: a blocked 1-thread pool holds the job in the
    /// queue, the invalidate happens, and the closure must never run.
    #[test]
    fn invalidated_jobs_are_skipped_before_they_start() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let pool = WorkerPool::new(1);
        let mut queue: JobQueue<()> = JobQueue::new();

        // Occupy the single worker so the next submit stays queued.
        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        pool.execute(move || {
            let _ = gate_rx.recv();
        });

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        queue.submit(&pool, move || flag.store(true, Ordering::SeqCst));
        queue.invalidate();

        gate_tx.send(()).expect("worker holds the gate receiver");
        drop(pool); // joins the worker, so the queued job has been processed

        assert!(!ran.load(Ordering::SeqCst), "superseded job must not run");
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn panicking_job_does_not_kill_the_pool() {
        let pool = WorkerPool::new(1);
        let mut queue = JobQueue::new();
        pool.execute(|| panic!("boom"));
        queue.submit(&pool, || 42);
        assert_eq!(drain_until(&mut queue, 1), vec![42]);
    }
}

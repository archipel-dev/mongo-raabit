//! Background work for the views: futures run on a private Tokio runtime and their
//! results are applied to the view state on the UI thread (`drain`), never blocking it.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

type Apply<S> = Box<dyn FnOnce(&mut S) + Send>;

pub struct Jobs<S> {
    runtime: Option<tokio::runtime::Runtime>,
    tx: Sender<Apply<S>>,
    rx: Receiver<Apply<S>>,
    pending: Arc<AtomicUsize>,
    repaint: Arc<Mutex<Option<egui::Context>>>,
}

impl<S: 'static> Default for Jobs<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: 'static> Jobs<S> {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mongo-raabit")
            .enable_all()
            .build()
            .ok();
        Self {
            runtime,
            tx,
            rx,
            pending: Arc::new(AtomicUsize::new(0)),
            repaint: Arc::new(Mutex::new(None)),
        }
    }

    /// Remember the context so finished jobs can wake the UI.
    pub fn set_context(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.repaint.lock()
            && slot.is_none()
        {
            *slot = Some(ctx.clone());
        }
    }

    /// Run `future` in the background and hand its output to `apply` on the UI thread.
    pub fn spawn<T, F, A>(&self, future: F, apply: A)
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
        A: FnOnce(&mut S, T) + Send + 'static,
    {
        let tx = self.tx.clone();
        let pending = self.pending.clone();
        let repaint = self.repaint.clone();
        pending.fetch_add(1, Ordering::SeqCst);
        let task = async move {
            let output = future.await;
            let _ = tx.send(Box::new(move |state: &mut S| apply(state, output)));
            pending.fetch_sub(1, Ordering::SeqCst);
            if let Ok(slot) = repaint.lock()
                && let Some(ctx) = slot.as_ref()
            {
                ctx.request_repaint();
            }
        };
        match &self.runtime {
            Some(runtime) => {
                runtime.spawn(task);
            }
            None => {
                // No runtime could be created (resource limits); run on a thread.
                std::thread::spawn(move || {
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        rt.block_on(task);
                    }
                });
            }
        }
    }

    /// Pending results, to be applied by the owner (`for f in jobs.take() { f(state) }`).
    pub fn take(&self) -> Vec<Apply<S>> {
        self.rx.try_iter().collect()
    }

    pub fn busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// Block until all jobs finished and apply them (tests and screenshots only).
    pub fn wait_idle(&self, state: &mut S, timeout: std::time::Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            for apply in self.take() {
                apply(state);
            }
            if !self.busy() || std::time::Instant::now() > deadline {
                for apply in self.take() {
                    apply(state);
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl<S> Drop for Jobs<S> {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_applied_on_drain() {
        let jobs: Jobs<Vec<u32>> = Jobs::new();
        let mut state = Vec::new();
        for i in 0..3 {
            jobs.spawn(async move { i * 2 }, |s: &mut Vec<u32>, v| s.push(v));
        }
        jobs.wait_idle(&mut state, std::time::Duration::from_secs(5));
        state.sort();
        assert_eq!(state, [0, 2, 4]);
        assert!(!jobs.busy());
    }
}

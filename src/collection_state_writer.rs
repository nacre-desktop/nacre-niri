//! A single coalescing worker for collection-state persistence.
//!
//! Compositor/event-loop callers only replace one pending logical snapshot and signal the worker.
//! The worker owns all filesystem work and never holds the queue lock while saving.

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use crate::layout::collection_persistence::PersistedCollections;

type Saver = Box<dyn Fn(PersistedCollections) -> Result<(), String> + Send + 'static>;

#[derive(Default)]
struct Shared {
    pending: Option<PersistedCollections>,
    stopping: bool,
    last_error: Option<String>,
}

pub(crate) struct CollectionStateWriter {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl CollectionStateWriter {
    /// Start the named persistence worker using the versioned atomic state writer.
    pub(crate) fn new() -> io::Result<Self> {
        Self::with_saver(|state| state.save_atomic().map_err(|error| error.to_string()))
    }

    /// Construct a writer with a testable saver. The saver runs only on the worker thread.
    pub(crate) fn with_saver(
        saver: impl Fn(PersistedCollections) -> Result<(), String> + Send + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let saver: Saver = Box::new(saver);
        let worker = thread::Builder::new()
            .name("nacre-collection-state".into())
            .spawn(move || run_worker(worker_shared, saver))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Replace the pending snapshot without doing filesystem work or waiting for a save.
    /// Returns false only after shutdown has begun.
    pub(crate) fn enqueue(&self, snapshot: PersistedCollections) -> bool {
        let (lock, wake) = &*self.shared;
        let mut shared = recover_lock(lock);
        if shared.stopping {
            return false;
        }
        let replaced = shared.pending.replace(snapshot);
        drop(shared);
        wake.notify_one();
        drop(replaced);
        true
    }

    /// Save the latest pending snapshot and join the worker.
    pub(crate) fn finish(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        if let Some(worker) = self.worker.take() {
            let (lock, wake) = &*self.shared;
            recover_lock(lock).stopping = true;
            wake.notify_one();
            if worker.join().is_err() {
                return Err("collection-state worker panicked".into());
            }
        }
        recover_lock(&self.shared.0)
            .last_error
            .clone()
            .map_or(Ok(()), Err)
    }
}

impl Drop for CollectionStateWriter {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn run_worker(shared: Arc<(Mutex<Shared>, Condvar)>, saver: Saver) {
    loop {
        let snapshot = {
            let (lock, wake) = &*shared;
            let mut state = recover_lock(lock);
            while state.pending.is_none() && !state.stopping {
                state = wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            match state.pending.take() {
                Some(snapshot) => Some(snapshot),
                None if state.stopping => None,
                None => continue,
            }
        };

        let Some(snapshot) = snapshot else {
            return;
        };
        let result = panic::catch_unwind(AssertUnwindSafe(|| saver(snapshot)))
            .unwrap_or_else(|_| Err("collection-state saver panicked".into()));
        if let Err(error) = &result {
            log::error!("Could not save collection state: {error}");
        }
        recover_lock(&shared.0).last_error = result.err();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::collection_persistence::{PersistedCollection, PersistedWorkspace};
    use std::sync::mpsc;
    use std::time::Duration;

    fn snapshot(name: &str) -> PersistedCollections {
        let mut state = PersistedCollections::empty();
        state.collections.push(PersistedCollection {
            name: name.into(),
            configured_origin: None,
            original_output: None,
            workspaces: vec![PersistedWorkspace {
                name: None,
                retain_empty: false,
            }],
            active_workspace: 0,
        });
        state
    }

    fn snapshot_name(state: &PersistedCollections) -> String {
        state.collections[0].name.clone()
    }

    #[test]
    fn pending_snapshots_coalesce_to_latest() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (saved_tx, saved_rx) = mpsc::channel();
        let writer = CollectionStateWriter::with_saver(move |state| {
            let name = snapshot_name(&state);
            if name == "first" {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            saved_tx.send(name).unwrap();
            Ok(())
        })
        .unwrap();

        assert!(writer.enqueue(snapshot("first")));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(writer.enqueue(snapshot("superseded")));
        assert!(writer.enqueue(snapshot("latest")));
        release_tx.send(()).unwrap();
        writer.finish().unwrap();

        assert_eq!(saved_rx.recv().unwrap(), "first");
        assert_eq!(saved_rx.recv().unwrap(), "latest");
        assert!(saved_rx.try_recv().is_err());
    }

    #[test]
    fn enqueue_does_not_wait_for_active_save() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (saved_tx, saved_rx) = mpsc::channel();
        let writer = Arc::new(
            CollectionStateWriter::with_saver(move |state| {
                let name = snapshot_name(&state);
                if name == "blocked" {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                saved_tx.send(name).unwrap();
                Ok(())
            })
            .unwrap(),
        );

        assert!(writer.enqueue(snapshot("blocked")));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let enqueue_writer = writer.clone();
        let (enqueued_tx, enqueued_rx) = mpsc::channel();
        let enqueuer = thread::spawn(move || {
            let accepted = enqueue_writer.enqueue(snapshot("while-blocked"));
            let _ = enqueued_tx.send(accepted);
        });
        let result = enqueued_rx.recv_timeout(Duration::from_secs(2));
        // Always release the saver before asserting, so a regression cannot strand the worker.
        release_tx.send(()).unwrap();
        enqueuer.join().unwrap();
        assert!(result.unwrap());
        drop(writer);

        assert_eq!(saved_rx.recv().unwrap(), "blocked");
        assert_eq!(saved_rx.recv().unwrap(), "while-blocked");
    }

    #[test]
    fn finish_flushes_the_latest_pending_snapshot() {
        let (saved_tx, saved_rx) = mpsc::channel();
        let writer = CollectionStateWriter::with_saver(move |state| {
            saved_tx.send(snapshot_name(&state)).unwrap();
            Ok(())
        })
        .unwrap();

        assert!(writer.enqueue(snapshot("pending")));
        writer.finish().unwrap();
        assert_eq!(saved_rx.recv().unwrap(), "pending");
        assert!(saved_rx.try_recv().is_err());
    }
}

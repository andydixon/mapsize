//! A clonable cancellation token: a flag to poll plus a channel that is
//! closed on cancellation, so it can also be waited on in a select.

use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Cancel(Arc<Inner>);

struct Inner {
    flag: AtomicBool,
    tx: Mutex<Option<Sender<()>>>,
    rx: Receiver<()>,
}

impl Default for Cancel {
    fn default() -> Self {
        Self::new()
    }
}

impl Cancel {
    pub fn new() -> Cancel {
        let (tx, rx) = crossbeam_channel::bounded(0);
        Cancel(Arc::new(Inner {
            flag: AtomicBool::new(false),
            tx: Mutex::new(Some(tx)),
            rx,
        }))
    }

    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::Relaxed);
        self.0.tx.lock().unwrap().take();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::Relaxed)
    }

    /// Becomes ready (disconnected) once cancelled; for use in select.
    pub fn done(&self) -> &Receiver<()> {
        &self.0.rx
    }
}

impl Cancel {
    /// Async-signal-safe half of cancel(): sets the flag only. Pollers must
    /// follow up with cancel() to wake select-based waiters.
    pub fn cancel_flag_only(&self) {
        self.0.flag.store(true, Ordering::Relaxed);
    }
}

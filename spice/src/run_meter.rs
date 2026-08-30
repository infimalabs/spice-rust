//! Cancellable session-meter refresh around one supervised child command.

use std::io::{self, Write as _};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};

use spice::session_meter::{Decision, LiveMeter};

pub(super) struct Runtime {
    cancel: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    pub(super) fn start(
        root: &Path,
        identities: Vec<(&'static spice::extension::Vendor, Option<String>)>,
    ) -> Self {
        let Ok(meter) = LiveMeter::open(root, identities) else {
            return Self::empty();
        };
        let Ok(decision) = meter.evaluate() else {
            return Self::empty();
        };
        emit(&decision);
        let Some(mut delay) = decision.refresh_after else {
            return Self::empty();
        };
        let (cancel, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("spice-session-meter".to_owned())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(delay) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    let Ok(decision) = meter.evaluate() else {
                        break;
                    };
                    emit(&decision);
                    let Some(next) = decision.refresh_after else {
                        break;
                    };
                    delay = next;
                }
            })
            .ok();
        match worker {
            Some(worker) => Self {
                cancel: Some(cancel),
                worker: Some(worker),
            },
            None => Self::empty(),
        }
    }

    pub(super) fn stop(mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    const fn empty() -> Self {
        Self {
            cancel: None,
            worker: None,
        }
    }
}

fn emit(decision: &Decision) {
    if let Some(announcement) = decision.announcement.as_deref() {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{announcement}");
    }
}

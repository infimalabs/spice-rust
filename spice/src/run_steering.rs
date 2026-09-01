//! Projection-driven steering delivery around one supervised child command.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SCAN_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct Runtime {
    stop: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    pub(super) fn start(rendering: Option<&Path>) -> Self {
        let Some(rendering) = rendering else {
            return Self::empty();
        };
        let rendering = rendering.to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let mut last = None;
        emit_current(&rendering, &mut last);
        let worker = match thread::Builder::new()
            .name("spice-run-steering".to_owned())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(SCAN_INTERVAL) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {
                            emit_current(&rendering, &mut last);
                        }
                    }
                }
            }) {
            Ok(worker) => worker,
            Err(_) => return Self::empty(),
        };
        Self {
            stop: Some(sender),
            worker: Some(worker),
        }
    }

    pub(super) fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    const fn empty() -> Self {
        Self {
            stop: None,
            worker: None,
        }
    }
}

fn emit_current(rendering: &Path, last: &mut Option<Vec<u8>>) {
    let bytes = match fs::read(rendering) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return,
    };
    if last.as_deref() == Some(bytes.as_slice()) {
        return;
    }
    *last = Some(bytes.clone());
    if bytes.is_empty() {
        return;
    }
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(&bytes).and_then(|()| stderr.flush());
}

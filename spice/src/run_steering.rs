//! Projection-driven steering delivery around one supervised child command.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

use notify::{RecursiveMode, Watcher as _};

enum Signal {
    Changed,
    Stop,
}

pub(super) struct Runtime {
    stop: Option<SyncSender<Signal>>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    pub(super) fn start(rendering: Option<&Path>) -> Self {
        let Some(rendering) = rendering else {
            return Self::empty();
        };
        let rendering = rendering.to_path_buf();
        let (sender, receiver) = mpsc::sync_channel(1);
        let changed = sender.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok() {
                let _ = changed.try_send(Signal::Changed);
            }
        })
        .and_then(|mut watcher| {
            let parent = rendering
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });
        let mut last = None;
        emit_current(&rendering, &mut last);
        let Ok(watcher) = watcher else {
            return Self::empty();
        };
        let worker = match thread::Builder::new()
            .name("spice-run-steering".to_owned())
            .spawn(move || {
                let _watcher = watcher;
                while let Ok(signal) = receiver.recv() {
                    match signal {
                        Signal::Changed => emit_current(&rendering, &mut last),
                        Signal::Stop => break,
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
            let _ = stop.send(Signal::Stop);
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

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
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

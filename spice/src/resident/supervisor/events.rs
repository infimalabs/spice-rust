//! Blocking state and process-exit observation for detached supervisors.

use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant as Monotonic};

use notify::{RecursiveMode, Watcher as _};
use sill::{WaitError, WaitOutcome};

use super::Handle;
use crate::resident::daemon::observer;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum StateSignal {
    Changed,
    SupervisorExited,
    ObserverFailed(String),
}

impl observer::ExitSender for mpsc::Sender<StateSignal> {
    fn exited(self) {
        let _ = self.send(StateSignal::SupervisorExited);
    }

    fn failed(self, error: String) {
        let _ = self.send(StateSignal::ObserverFailed(error));
    }
}

pub(super) struct StateEvents {
    _watcher: notify::RecommendedWatcher,
    sender: mpsc::Sender<StateSignal>,
    receiver: mpsc::Receiver<StateSignal>,
}

impl StateEvents {
    pub(super) fn arm(path: &Path, supervisor: u32) -> io::Result<Self> {
        let events = Self::watch(path)?;
        events.observe_exit(supervisor);
        Ok(events)
    }

    pub(super) fn watch(path: &Path) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "supervisor state path has no parent directory",
            )
        })?;
        let parent = parent.canonicalize()?;
        let target = parent.join(path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "supervisor state path has no file name",
            )
        })?);
        let (sender, receiver) = mpsc::channel();
        let changed = sender.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event)
                    if event.need_rescan() || event.paths.iter().any(|path| path == &target) =>
                {
                    let _ = changed.send(StateSignal::Changed);
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = changed.send(StateSignal::ObserverFailed(error.to_string()));
                }
            })
            .map_err(io::Error::other)?;
        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;
        Ok(Self {
            _watcher: watcher,
            sender,
            receiver,
        })
    }

    pub(super) fn observe_exit(&self, supervisor: u32) {
        observer::wait_for_exit(supervisor, self.sender.clone());
    }

    fn wait(&self) -> io::Result<StateSignal> {
        self.receiver.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "supervisor state observer disconnected",
            )
        })
    }

    pub(super) fn wait_for(&self, duration: Duration) -> io::Result<Option<StateSignal>> {
        match self.receiver.recv_timeout(duration) {
            Ok(signal) => Ok(Some(signal)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "supervisor state observer disconnected",
            )),
        }
    }
}

impl Handle {
    fn wait_event(&self, duration: Option<Duration>) -> Result<Option<StateSignal>, WaitError> {
        let events = self.events.as_ref().ok_or_else(|| {
            self.wait_error(
                "waiting for supervisor state change",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "running supervisor handle has no state observer",
                ),
            )
        })?;
        match duration {
            Some(duration) => events.wait_for(duration),
            None => events.wait().map(Some),
        }
        .map_err(|error| self.wait_error("waiting for supervisor state change", error))
    }

    pub(super) fn observe_until(
        &self,
        duration: Option<Duration>,
    ) -> Result<Option<WaitOutcome>, WaitError> {
        let started = Monotonic::now();
        loop {
            if let Some(outcome) = self.terminal()? {
                return Ok(Some(outcome));
            }
            let remaining = duration.map(|duration| duration.saturating_sub(started.elapsed()));
            if remaining.is_some_and(|remaining| remaining.is_zero()) {
                return Ok(None);
            }
            match self.wait_event(remaining)? {
                Some(StateSignal::Changed) => {}
                Some(StateSignal::SupervisorExited) => {
                    if let Some(outcome) = self.terminal()? {
                        return Ok(Some(outcome));
                    }
                    return Err(self.wait_error(
                        "observing supervisor",
                        io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "supervisor exited before publishing an ending",
                        ),
                    ));
                }
                Some(StateSignal::ObserverFailed(error)) => {
                    return Err(self.wait_error(
                        "waiting for supervisor state change",
                        io::Error::other(error),
                    ));
                }
                None => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::resident::supervisor::{now_millis, publish_json};

    #[test]
    fn atomic_state_publication_wakes_the_exact_path_observer() {
        let root = std::env::temp_dir().join(format!(
            "spice-supervisor-state-events-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        fs::create_dir_all(&root).expect("fixture directory creates");
        let path = root.join("state.json");
        let events = StateEvents::watch(&path).expect("state observer arms");

        publish_json(&path, &serde_json::json!({ "phase": "running" }))
            .expect("atomic state fixture publishes");

        assert_eq!(
            events
                .wait_for(Duration::from_secs(5))
                .expect("state observer remains connected"),
            Some(StateSignal::Changed)
        );
        drop(events);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}

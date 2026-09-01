//! Projection-driven steering delivery around one supervised child command.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use notify::{RecursiveMode, Watcher as _};

#[derive(Debug)]
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
        Self::start_with(rendering, watch_parent, read_current, write_current)
    }

    fn start_with<W, F, R, E>(rendering: Option<&Path>, watch: F, mut read: R, mut emit: E) -> Self
    where
        W: Send + 'static,
        F: FnOnce(&Path, SyncSender<Signal>) -> Option<W>,
        R: FnMut(&Path) -> Option<Vec<u8>> + Send + 'static,
        E: FnMut(&[u8]) + Send + 'static,
    {
        let Some(rendering) = rendering else {
            return Self::empty();
        };
        let rendering = rendering.to_path_buf();
        let (sender, receiver) = mpsc::sync_channel(1);
        let parent = rendering
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let watcher = watch(parent, sender.clone());
        let mut last = None;
        reconcile(&rendering, &mut last, &mut read, &mut emit);
        let Some(watcher) = watcher else {
            return Self::empty();
        };
        let worker = match thread::Builder::new()
            .name("spice-run-steering".to_owned())
            .spawn(move || drive(watcher, receiver, rendering, last, read, emit))
        {
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

fn watch_parent(parent: &Path, changed: SyncSender<Signal>) -> Option<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = changed.try_send(Signal::Changed);
        }
    })
    .ok()?;
    watcher.watch(parent, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}

fn drive<W, R, E>(
    watcher: W,
    receiver: Receiver<Signal>,
    rendering: PathBuf,
    mut last: Option<Vec<u8>>,
    mut read: R,
    mut emit: E,
) where
    R: FnMut(&Path) -> Option<Vec<u8>>,
    E: FnMut(&[u8]),
{
    let _watcher = watcher;
    while let Ok(signal) = receiver.recv() {
        match signal {
            Signal::Changed => reconcile(&rendering, &mut last, &mut read, &mut emit),
            Signal::Stop => break,
        }
    }
}

fn read_current(rendering: &Path) -> Option<Vec<u8>> {
    Some(match fs::read(rendering) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return None,
    })
}

fn write_current(bytes: &[u8]) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(bytes).and_then(|()| stderr.flush());
}

fn reconcile<R, E>(rendering: &Path, last: &mut Option<Vec<u8>>, read: &mut R, emit: &mut E)
where
    R: FnMut(&Path) -> Option<Vec<u8>>,
    E: FnMut(&[u8]),
{
    let Some(bytes) = read(rendering) else {
        return;
    };
    if last.as_deref() == Some(bytes.as_slice()) {
        return;
    }
    if !bytes.is_empty() {
        emit(&bytes);
    }
    *last = Some(bytes);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    struct FakeWatcher {
        dropped: Arc<AtomicUsize>,
        _changed: SyncSender<Signal>,
    }

    impl Drop for FakeWatcher {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn watcher_before_read_queues_the_race_and_deduplicates_every_notification() {
        let bytes = Arc::new(Mutex::new(b"initial".to_vec()));
        let reads = Arc::new(AtomicUsize::new(0));
        let outputs = Arc::new(Mutex::new(Vec::new()));
        let signal = Arc::new(Mutex::new(None));
        let dropped = Arc::new(AtomicUsize::new(0));
        let (read_send, read_receive) = mpsc::channel();
        let (read_release, release_receive) = mpsc::sync_channel(0);

        let mut runtime = Runtime::start_with(
            Some(Path::new("/fixture/inbox")),
            {
                let bytes = Arc::clone(&bytes);
                let reads = Arc::clone(&reads);
                let signal = Arc::clone(&signal);
                let dropped = Arc::clone(&dropped);
                move |_, changed| {
                    assert_eq!(reads.load(Ordering::SeqCst), 0);
                    *bytes.lock().expect("fixture bytes lock") = b"published".to_vec();
                    changed
                        .try_send(Signal::Changed)
                        .expect("admission-gap notification queues");
                    *signal.lock().expect("fixture signal lock") = Some(changed.clone());
                    Some(FakeWatcher {
                        dropped,
                        _changed: changed,
                    })
                }
            },
            {
                let bytes = Arc::clone(&bytes);
                let reads = Arc::clone(&reads);
                move |_| {
                    let call = reads.fetch_add(1, Ordering::SeqCst) + 1;
                    read_send.send(call).expect("read observation sends");
                    if call > 1 {
                        release_receive.recv().expect("worker read releases");
                    }
                    Some(bytes.lock().expect("fixture bytes lock").clone())
                }
            },
            {
                let outputs = Arc::clone(&outputs);
                move |bytes| {
                    outputs
                        .lock()
                        .expect("fixture output lock")
                        .push(bytes.to_vec());
                }
            },
        );

        assert_eq!(read_receive.recv().expect("initial read completes"), 1);
        assert_eq!(read_receive.recv().expect("queued read starts"), 2);
        read_release.send(()).expect("queued read releases");
        signal
            .lock()
            .expect("fixture signal lock")
            .as_ref()
            .expect("watcher signal captured")
            .send(Signal::Changed)
            .expect("unchanged notification queues");
        assert_eq!(read_receive.recv().expect("unchanged read starts"), 3);
        assert_eq!(
            *outputs.lock().expect("fixture output lock"),
            [b"published".to_vec()]
        );
        read_release.send(()).expect("unchanged read releases");
        runtime.stop();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn explicit_stop_and_drop_each_wake_join_and_release_the_watcher() {
        let explicit = Arc::new(AtomicUsize::new(0));
        let mut runtime = idle_runtime(Arc::clone(&explicit));
        runtime.stop();
        runtime.stop();
        assert_eq!(explicit.load(Ordering::SeqCst), 1);

        let implicit = Arc::new(AtomicUsize::new(0));
        drop(idle_runtime(Arc::clone(&implicit)));
        assert_eq!(implicit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_inflight_change_finishes_once_before_a_queued_stop_joins() {
        let (signals, receiver) = mpsc::sync_channel(2);
        let (entered, observed) = mpsc::sync_channel(0);
        let (release, released) = mpsc::sync_channel(0);
        let outputs = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(AtomicUsize::new(0));
        signals.send(Signal::Changed).expect("change queues");

        let worker = {
            let outputs = Arc::clone(&outputs);
            let dropped = Arc::clone(&dropped);
            let callback = signals.clone();
            thread::spawn(move || {
                drive(
                    FakeWatcher {
                        dropped,
                        _changed: callback,
                    },
                    receiver,
                    PathBuf::from("/fixture/inbox"),
                    Some(b"initial".to_vec()),
                    move |_| {
                        entered.send(()).expect("inflight read announces");
                        released.recv().expect("inflight read releases");
                        Some(b"changed".to_vec())
                    },
                    move |bytes| {
                        outputs
                            .lock()
                            .expect("fixture output lock")
                            .push(bytes.to_vec());
                    },
                );
            })
        };
        observed.recv().expect("change read is in flight");
        signals
            .send(Signal::Stop)
            .expect("stop queues behind change");
        release.send(()).expect("inflight read release sends");
        worker.join().expect("worker joins after queued stop");

        assert_eq!(
            *outputs.lock().expect("fixture output lock"),
            [b"changed".to_vec()]
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    fn idle_runtime(dropped: Arc<AtomicUsize>) -> Runtime {
        Runtime::start_with(
            Some(Path::new("/fixture/inbox")),
            move |_, changed| {
                Some(FakeWatcher {
                    dropped,
                    _changed: changed,
                })
            },
            |_| Some(Vec::new()),
            |_| {},
        )
    }
}

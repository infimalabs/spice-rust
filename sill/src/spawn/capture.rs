use super::{CapturedOutput, IoFailure, ProcessState, WAKE_BYTE, fail_observation, platform};
use crate::observe::{
    Observation, ObservationFailure, ObservedRecord, OutputStream, RecordBoundary, RecordObserver,
};
use crate::spec::ProcessGroup;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct CaptureHandle {
    state: Arc<Mutex<CaptureState>>,
    failure_supervised: bool,
    interrupt: Option<UnixDatagram>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug)]
struct CaptureState {
    output: CapturedOutput,
    failure: Option<IoFailure>,
    observer_complete: bool,
}

impl CaptureHandle {
    pub(super) fn is_complete(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.output.complete && state.observer_complete)
    }

    pub(super) fn failure(&self) -> io::Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("capture state lock is poisoned"))?;
        if self.failure_supervised {
            return Ok(());
        }
        state
            .failure
            .as_ref()
            .map_or(Ok(()), |failure| Err(failure.error()))
    }

    pub(super) fn snapshot(&self) -> io::Result<CapturedOutput> {
        self.state
            .lock()
            .map(|state| state.output.clone())
            .map_err(|_| io::Error::other("capture state lock is poisoned"))
    }

    pub(super) fn finish(mut self) -> io::Result<()> {
        let complete = self
            .state
            .lock()
            .map(|state| state.output.complete)
            .map_err(|_| io::Error::other("capture state lock is poisoned"))?;
        if !complete && let Some(interrupt) = &self.interrupt {
            let _interrupt_result = interrupt.send(WAKE_BYTE);
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("capture worker panicked"))?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct FailureContext {
    pub(super) pid: u32,
    pub(super) group: ProcessGroup,
    pub(super) owner: String,
    pub(super) grace: Duration,
    pub(super) reap: Duration,
    pub(super) process: Arc<ProcessState>,
    pub(super) wake: std::os::unix::net::UnixDatagram,
    pub(super) failed: AtomicBool,
}

#[derive(Debug)]
struct Frame {
    bytes: Vec<u8>,
    boundary: RecordBoundary,
}

pub(super) struct CaptureWorker {
    pub(super) stream: OutputStream,
    pub(super) observation: Option<Observation>,
    pub(super) failure_observer: Option<Arc<dyn RecordObserver>>,
    pub(super) sink_deadline: Duration,
    pub(super) failure: Arc<FailureContext>,
    pub(super) wake: std::os::unix::net::UnixDatagram,
    pub(super) observer_wake: Option<std::os::unix::net::UnixDatagram>,
}

pub(super) fn start_capture<R>(mut reader: R, limit: usize, worker: CaptureWorker) -> CaptureHandle
where
    R: Read + AsRawFd + Send + 'static,
{
    let reader_descriptor = reader.as_raw_fd();
    let CaptureWorker {
        stream,
        observation,
        failure_observer,
        sink_deadline,
        failure,
        wake,
        observer_wake,
    } = worker;
    let selected = observation.is_some();
    let failure_supervised = failure_observer.is_some();
    let (interrupt, interrupt_receiver) = if selected {
        (None, None)
    } else {
        let (sender, receiver) = UnixDatagram::pair()
            .expect("capture interruption descriptors are created before the worker");
        (Some(sender), Some(receiver))
    };
    let state = Arc::new(Mutex::new(CaptureState {
        output: CapturedOutput {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            truncated: false,
            complete: false,
        },
        failure: None,
        observer_complete: !selected,
    }));
    let worker_state = Arc::clone(&state);
    let worker = std::thread::spawn(move || {
        let mut framer = observation.map(|observation| {
            SelectedFramer::new(
                observation,
                sink_deadline,
                Arc::clone(&failure),
                Arc::clone(&worker_state),
                observer_wake.expect("selected capture has a preallocated observer wake"),
                failure_observer
                    .as_ref()
                    .map(Arc::clone)
                    .expect("selected capture has a failure observer"),
            )
        });
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match wait_to_read(reader_descriptor, interrupt_receiver.as_ref()) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    record_reader_failure(
                        stream,
                        error,
                        &worker_state,
                        failure_observer.as_ref(),
                        &failure,
                    );
                    let _wake_result = wake.send(WAKE_BYTE);
                    return;
                }
            }
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Some(framer) = &mut framer {
                        framer.finish();
                    }
                    if let Ok(mut state) = worker_state.lock() {
                        state.output.complete = true;
                    }
                    let _wake_result = wake.send(WAKE_BYTE);
                    return;
                }
                Ok(read) => {
                    if let Ok(mut state) = worker_state.lock() {
                        let available = limit.saturating_sub(state.output.bytes.len());
                        let retained = available.min(read);
                        state.output.bytes.extend_from_slice(&buffer[..retained]);
                        state.output.truncated |= retained < read;
                    }
                    if let Some(framer) = &mut framer
                        && !framer.push(&buffer[..read])
                    {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    record_reader_failure(
                        stream,
                        error,
                        &worker_state,
                        failure_observer.as_ref(),
                        &failure,
                    );
                    let _wake_result = wake.send(WAKE_BYTE);
                    return;
                }
            }
        }
    });

    CaptureHandle {
        state,
        failure_supervised,
        interrupt,
        // Opaque capture is wholly owned by Sill and can therefore be joined
        // after process-group cleanup. A selected capture may still be inside
        // caller-owned observer code whose separate deadline and failure path
        // deliberately prevent it from bounding process cleanup.
        worker: (!selected).then_some(worker),
    }
}

fn wait_to_read(reader_descriptor: RawFd, interrupt: Option<&UnixDatagram>) -> io::Result<bool> {
    let Some(interrupt) = interrupt else {
        return Ok(true);
    };
    let mut descriptors = [
        platform::PollFd::read(reader_descriptor),
        platform::PollFd::read(interrupt.as_raw_fd()),
    ];
    loop {
        let result = unsafe {
            platform::poll(
                descriptors.as_mut_ptr(),
                platform::Nfds::try_from(descriptors.len()).expect("two descriptors fit"),
                -1,
            )
        };
        if result >= 0 {
            if descriptors[1].ready() {
                return Ok(false);
            }
            if descriptors[0].ready() {
                return Ok(true);
            }
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn record_reader_failure(
    stream: OutputStream,
    error: io::Error,
    state: &Mutex<CaptureState>,
    failure_observer: Option<&Arc<dyn RecordObserver>>,
    failure: &FailureContext,
) {
    let captured = IoFailure::capture(error);
    if let Ok(mut state) = state.lock() {
        state.failure = Some(captured.clone());
    }
    if let Some(observer) = failure_observer {
        fail_observation(
            failure,
            observer,
            ObservationFailure::Reader {
                stream,
                detail: captured.detail,
            },
        );
    }
}

struct SelectedFramer {
    stream: OutputStream,
    limit: usize,
    capacity: usize,
    sink_deadline: Duration,
    record: Vec<u8>,
    sender: mpsc::SyncSender<Frame>,
    observer: Arc<dyn RecordObserver>,
    failure: Arc<FailureContext>,
}

impl SelectedFramer {
    fn new(
        observation: Observation,
        sink_deadline: Duration,
        failure: Arc<FailureContext>,
        state: Arc<Mutex<CaptureState>>,
        wake: std::os::unix::net::UnixDatagram,
        observer: Arc<dyn RecordObserver>,
    ) -> Self {
        let stream = observation.stream();
        let limit = observation.frame_limit();
        let capacity = observation.queue_capacity();
        let (sender, receiver) = mpsc::sync_channel(capacity);
        start_observer_worker(
            receiver,
            ObserverWorker {
                stream,
                capacity,
                sink_deadline,
                observer: Arc::clone(&observer),
                failure: Arc::clone(&failure),
                state,
                wake,
            },
        );
        Self {
            stream,
            limit,
            capacity,
            sink_deadline,
            record: Vec::with_capacity(limit.min(8 * 1024)),
            sender,
            observer,
            failure,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> bool {
        let mut start = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            let end = index + 1;
            if !self.extend_or_preserve_overflow(&bytes[start..end], &bytes[end..]) {
                return false;
            }
            if !self.send(RecordBoundary::Complete) {
                return false;
            }
            start = end;
        }
        self.extend_or_preserve_overflow(&bytes[start..], &[])
    }

    fn extend_or_preserve_overflow(&mut self, bytes: &[u8], remainder: &[u8]) -> bool {
        let observed = self.record.len().saturating_add(bytes.len());
        if observed > self.limit {
            self.record.extend_from_slice(bytes);
            self.record.extend_from_slice(remainder);
            let frame = Frame {
                bytes: std::mem::take(&mut self.record),
                boundary: RecordBoundary::Terminal,
            };
            let _preserved = self.sender.try_send(frame);
            self.fail(ObservationFailure::FramingOverflow {
                stream: self.stream,
                limit: self.limit,
                observed: observed.saturating_add(remainder.len()),
            });
            return false;
        }
        self.record.extend_from_slice(bytes);
        true
    }

    fn finish(&mut self) {
        if !self.record.is_empty() {
            let _accepted = self.send(RecordBoundary::Terminal);
        }
    }

    fn send(&mut self, boundary: RecordBoundary) -> bool {
        let mut frame = Frame {
            bytes: std::mem::take(&mut self.record),
            boundary,
        };
        let started = Instant::now();
        loop {
            match self.sender.try_send(frame) {
                Ok(()) => return true,
                Err(mpsc::TrySendError::Full(returned)) => {
                    if started.elapsed() >= self.sink_deadline {
                        self.fail(ObservationFailure::SinkBackpressure {
                            stream: self.stream,
                            capacity: self.capacity,
                            deadline: self.sink_deadline,
                        });
                        return false;
                    }
                    frame = returned;
                    std::thread::park_timeout(
                        self.sink_deadline
                            .saturating_sub(started.elapsed())
                            .min(Duration::from_millis(1)),
                    );
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.fail(ObservationFailure::ObserverLost {
                        stream: self.stream,
                    });
                    return false;
                }
            }
        }
    }

    fn fail(&self, failure: ObservationFailure) {
        fail_observation(&self.failure, &self.observer, failure);
    }
}

struct ObserverWorker {
    stream: OutputStream,
    capacity: usize,
    sink_deadline: Duration,
    observer: Arc<dyn RecordObserver>,
    failure: Arc<FailureContext>,
    state: Arc<Mutex<CaptureState>>,
    wake: std::os::unix::net::UnixDatagram,
}

fn start_observer_worker(receiver: mpsc::Receiver<Frame>, worker: ObserverWorker) {
    let ObserverWorker {
        stream,
        capacity,
        sink_deadline,
        observer,
        failure,
        state,
        wake,
    } = worker;
    let (requests, callbacks) = mpsc::sync_channel::<Frame>(0);
    let (results, result_receiver) = mpsc::sync_channel(0);
    let callback_observer = Arc::clone(&observer);
    std::thread::spawn(move || {
        for frame in callbacks {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback_observer.observe(ObservedRecord::new(&frame.bytes, frame.boundary))
            }));
            let result = match result {
                Ok(Ok(())) => ObserverResult::Accepted,
                Ok(Err(error)) => ObserverResult::Refused(error),
                Err(_) => ObserverResult::Lost,
            };
            if results.send(result).is_err() {
                return;
            }
        }
    });
    std::thread::spawn(move || {
        for frame in receiver {
            if requests.send(frame).is_err() {
                fail_observation(
                    &failure,
                    &observer,
                    ObservationFailure::ObserverLost { stream },
                );
                break;
            }
            match result_receiver.recv_timeout(sink_deadline) {
                Ok(ObserverResult::Accepted) => {}
                Ok(ObserverResult::Refused(error)) => {
                    fail_observation(
                        &failure,
                        &observer,
                        ObservationFailure::SinkRefused {
                            stream,
                            detail: error.detail().to_owned(),
                        },
                    );
                    break;
                }
                Ok(ObserverResult::Lost) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    fail_observation(
                        &failure,
                        &observer,
                        ObservationFailure::ObserverLost { stream },
                    );
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    fail_observation(
                        &failure,
                        &observer,
                        ObservationFailure::SinkBackpressure {
                            stream,
                            capacity,
                            deadline: sink_deadline,
                        },
                    );
                    break;
                }
            }
        }
        if let Ok(mut state) = state.lock() {
            state.observer_complete = true;
        }
        let _wake_result = wake.send(WAKE_BYTE);
    });
}

enum ObserverResult {
    Accepted,
    Refused(crate::observe::ObserveError),
    Lost,
}

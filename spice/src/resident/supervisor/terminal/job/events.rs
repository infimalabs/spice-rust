//! Line events: a job started with events makes each line its command prints
//! on stdout an event the lane delivers while the command runs.
//!
//! The keeper reads the command's stdout itself, copying every byte to the
//! job's log so the log stays the whole record, and splits it into lines.
//! Lines arriving within one window are gathered into one event; a command
//! printing faster than the flood bound allows ends its events with one final
//! event saying so and keeps running. Each event is a numbered record in the
//! job's directory, published atomically and numbered from one without gaps,
//! so the directory's watch wakes whoever follows the job and a reader knows
//! where the records end. The lane records the last event it heard; every
//! later record is still owed, so a successor seat hears exactly what its
//! predecessor did not.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant as Monotonic};

use serde::{Deserialize, Serialize};

use super::super::super::{publish_json, read_json};
use super::DELIVERED;

/// How long one event gathers lines, from the first line it carries.
const WINDOW: Duration = Duration::from_millis(vendor::omp::SHELL_EVENT_WINDOW_MILLIS);

/// Most lines a job may print within [`FLOOD_WINDOW`] before its events stop.
const FLOOD_LINES: usize = vendor::omp::SHELL_EVENT_FLOOD_LINES;

/// The span over which [`FLOOD_LINES`] is counted.
const FLOOD_WINDOW: Duration = Duration::from_secs(vendor::omp::SHELL_EVENT_FLOOD_SECONDS);

/// Bytes of one line an event keeps; the rest stays in the log.
const LINE_BYTES: usize = 1024;

/// Bound on one event record: a full flood window of clipped lines, with
/// room for JSON escaping.
const EVENT_BYTES: u64 = 1024 * 1024;

/// Bytes one read of the command's stdout takes.
const CHUNK: usize = 8 * 1024;

/// The record of the last event the lane heard.
const HEARD: &str = "events-heard.json";

/// What one event says.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::resident) enum Event {
    /// Lines the command printed within one window, in order.
    Lines { lines: Vec<String> },
    /// The command printed faster than the flood bound; no event follows.
    Flooded,
}

#[derive(Deserialize, Serialize)]
struct Heard {
    through: u64,
}

pub(super) fn record(directory: &Path, sequence: u64) -> PathBuf {
    directory.join(format!("event-{sequence:08}.json"))
}

/// The number of the last event the lane heard; zero before any.
pub(super) fn heard(directory: &Path) -> io::Result<u64> {
    match read_json::<Heard>(&directory.join(HEARD), 1024) {
        Ok(heard) => Ok(heard.through),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// Record that the lane heard every event through `through`.
pub(super) fn hear(directory: &Path, through: u64) -> io::Result<()> {
    publish_json(&directory.join(HEARD), &Heard { through })
}

/// Whether the lane needs no wake for event `sequence`: it heard that event,
/// or it heard the job's ending, which every telling of the log covers.
pub(super) fn heard_through(directory: &Path, sequence: u64) -> bool {
    directory.join(DELIVERED).exists() || heard(directory).is_ok_and(|through| through >= sequence)
}

/// Every event published after `through`, oldest first.
pub(super) fn after(directory: &Path, through: u64) -> io::Result<Vec<(u64, Event)>> {
    let mut events = Vec::new();
    for sequence in through + 1.. {
        match read_json(&record(directory, sequence), EVENT_BYTES) {
            Ok(event) => events.push((sequence, event)),
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(events)
}

/// Splits stdout bytes into lines: a line ends at a newline, loses a
/// carriage return before it, and keeps at most [`LINE_BYTES`] bytes,
/// marked when clipped. An unterminated last line is still a line.
#[derive(Default)]
struct Lines {
    partial: Vec<u8>,
    clipped: bool,
}

impl Lines {
    fn push(&mut self, bytes: &[u8], mut line: impl FnMut(String)) {
        for piece in bytes.split_inclusive(|byte| *byte == b'\n') {
            let (body, ended) = match piece.strip_suffix(b"\n") {
                Some(body) => (body, true),
                None => (piece, false),
            };
            let room = LINE_BYTES.saturating_sub(self.partial.len());
            self.clipped |= body.len() > room;
            self.partial
                .extend_from_slice(&body[..body.len().min(room)]);
            if ended {
                line(self.take());
            }
        }
    }

    fn finish(mut self) -> Option<String> {
        (!self.partial.is_empty() || self.clipped).then(|| self.take())
    }

    fn take(&mut self) -> String {
        let mut bytes = std::mem::take(&mut self.partial);
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if std::mem::take(&mut self.clipped) {
            text.push_str(" …");
        }
        text
    }
}

/// Gathers lines into events by when they arrived.
#[derive(Default)]
struct Batcher {
    pending: Vec<String>,
    /// When the first pending line arrived.
    opened: Option<Monotonic>,
    /// When each line still inside the flood window arrived.
    recent: VecDeque<Monotonic>,
    flooded: bool,
}

impl Batcher {
    /// When the event being gathered closes, if one is.
    fn due(&self) -> Option<Monotonic> {
        self.opened.map(|opened| opened + WINDOW)
    }

    /// Take one line arriving at `at`: the events it closes, in order. A
    /// line past the window closes the event before it; the line that
    /// breaks the flood bound closes the pending event and ends the events.
    fn line(&mut self, at: Monotonic, text: String) -> Vec<Event> {
        if self.flooded {
            return Vec::new();
        }
        let mut closed: Vec<Event> = self.tick(at).into_iter().collect();
        while self
            .recent
            .front()
            .is_some_and(|first| at.saturating_duration_since(*first) >= FLOOD_WINDOW)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= FLOOD_LINES {
            self.flooded = true;
            closed.extend(self.close());
            closed.push(Event::Flooded);
            return closed;
        }
        self.recent.push_back(at);
        self.opened.get_or_insert(at);
        self.pending.push(text);
        closed
    }

    /// The pending event, once its window has passed by `at`.
    fn tick(&mut self, at: Monotonic) -> Option<Event> {
        if self.due().is_some_and(|due| at >= due) {
            self.close()
        } else {
            None
        }
    }

    /// The pending event, whatever its window: stdout has ended.
    fn close(&mut self) -> Option<Event> {
        self.opened = None;
        (!self.pending.is_empty()).then(|| Event::Lines {
            lines: std::mem::take(&mut self.pending),
        })
    }
}

/// The keeper's end of a command's stdout, publishing its events.
pub(super) struct Pump {
    done: mpsc::Receiver<()>,
    /// Held while an event is published; once set, nothing more is.
    closed: Arc<Mutex<bool>>,
}

impl Pump {
    /// Wait up to `bound` for stdout to end and its last event to be
    /// published. A descendant can hold stdout open past the command, so a
    /// pump still running then publishes nothing more: no event follows the
    /// ending.
    pub(super) fn settle(self, bound: Duration) {
        if self.done.recv_timeout(bound).is_err() {
            *self.closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        }
    }
}

/// Read the command's stdout into the log and its events into the job's
/// directory, telling `published` the number of each event published.
///
/// # Errors
///
/// Returns the failure of starting the pump's threads.
pub(super) fn pump(
    stdout: impl Read + Send + 'static,
    mut log: File,
    directory: PathBuf,
    published: Box<dyn Fn(u64) + Send>,
) -> io::Result<Pump> {
    let (lines_sender, lines_receiver) = mpsc::channel::<(Monotonic, String)>();
    thread::Builder::new()
        .name("spice-job-stdout".to_owned())
        .spawn(move || {
            let mut stdout = stdout;
            let mut lines = Lines::default();
            let mut chunk = [0u8; CHUNK];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(length) => {
                        let _ = log.write_all(&chunk[..length]);
                        lines.push(&chunk[..length], |line| {
                            let _ = lines_sender.send((Monotonic::now(), line));
                        });
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            if let Some(line) = lines.finish() {
                let _ = lines_sender.send((Monotonic::now(), line));
            }
        })?;
    let (done_sender, done) = mpsc::channel();
    let closed = Arc::new(Mutex::new(false));
    let publishing = Arc::clone(&closed);
    thread::Builder::new()
        .name("spice-job-events".to_owned())
        .spawn(move || {
            let mut batcher = Batcher::default();
            let mut sequence = 0;
            let mut publish = |event: Event| {
                let closed = publishing.lock().unwrap_or_else(PoisonError::into_inner);
                if *closed {
                    return;
                }
                if publish_json(&record(&directory, sequence + 1), &event).is_ok() {
                    sequence += 1;
                    published(sequence);
                }
            };
            loop {
                let received = match batcher.due() {
                    Some(due) => {
                        lines_receiver.recv_timeout(due.saturating_duration_since(Monotonic::now()))
                    }
                    None => lines_receiver
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                match received {
                    Ok((at, line)) => batcher.line(at, line).into_iter().for_each(&mut publish),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        batcher
                            .tick(Monotonic::now())
                            .into_iter()
                            .for_each(&mut publish);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        batcher.close().into_iter().for_each(&mut publish);
                        break;
                    }
                }
            }
            let _ = done_sender.send(());
        })?;
    Ok(Pump { done, closed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Event {
        Event::Lines {
            lines: texts.iter().map(|text| (*text).to_owned()).collect(),
        }
    }

    #[test]
    fn lines_within_one_window_are_one_event_in_arrival_order() {
        let start = Monotonic::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut batcher = Batcher::default();
        for (millis, text) in [(0, "a"), (50, "b"), (199, "c")] {
            assert!(batcher.line(at(millis), text.to_owned()).is_empty());
        }
        assert_eq!(batcher.tick(at(199)), None, "the window is still open");
        assert_eq!(batcher.tick(at(200)), Some(lines(&["a", "b", "c"])));
        assert_eq!(batcher.tick(at(1_000)), None, "a closed event is told once");

        assert!(batcher.line(at(300), "d".to_owned()).is_empty());
        assert_eq!(
            batcher.line(at(600), "e".to_owned()),
            vec![lines(&["d"])],
            "a line past the window closes the event before it",
        );
        assert_eq!(batcher.close(), Some(lines(&["e"])));
        assert_eq!(batcher.close(), None);
    }

    #[test]
    fn a_flood_closes_the_pending_lines_then_ends_the_events() {
        let start = Monotonic::now();
        let mut batcher = Batcher::default();
        // A full bound of lines, then the same again once the span has passed.
        for round in 0..2 {
            let at = start + FLOOD_WINDOW * round;
            for line in 0..FLOOD_LINES {
                let closed = batcher.line(at, line.to_string());
                assert!(closed.iter().all(|event| *event != Event::Flooded));
            }
        }
        let at = start + FLOOD_WINDOW;
        let closed = batcher.line(at, "over".to_owned());
        assert_eq!(closed.len(), 2, "the pending lines, then the flood");
        let Event::Lines { lines } = &closed[0] else {
            panic!("the pending lines close first: {closed:?}");
        };
        assert_eq!(lines.len(), FLOOD_LINES);
        assert_eq!(closed[1], Event::Flooded);
        assert!(
            batcher
                .line(at + FLOOD_WINDOW * 2, "late".to_owned())
                .is_empty()
        );
        assert_eq!(batcher.close(), None, "nothing follows a flood");
    }

    #[test]
    fn stdout_lines_survive_any_read_boundary_and_stay_bounded() {
        let mut lines = Lines::default();
        let mut seen = Vec::new();
        lines.push(b"one\r\ntw", |line| seen.push(line));
        lines.push(b"o\n", |line| seen.push(line));
        lines.push(&[b'x'; LINE_BYTES + 10], |line| seen.push(line));
        lines.push(b"\nlast", |line| seen.push(line));
        seen.extend(lines.finish());
        assert_eq!(seen[..2], ["one", "two"]);
        assert_eq!(seen[2], format!("{} …", "x".repeat(LINE_BYTES)));
        assert_eq!(seen[3], "last", "an unterminated last line is a line");
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn an_event_is_owed_until_the_lane_hears_it_or_its_ending() {
        let scratch = std::env::temp_dir().join(format!(
            "spice-job-events-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        std::fs::create_dir_all(&scratch).expect("job directory");
        let directory = scratch.as_path();
        for (sequence, event) in [(1, lines(&["a"])), (2, lines(&["b"])), (3, Event::Flooded)] {
            publish_json(&record(directory, sequence), &event).expect("event publishes");
        }
        assert_eq!(heard(directory).expect("cursor reads"), 0);
        assert_eq!(
            after(directory, 0).expect("events read"),
            vec![(1, lines(&["a"])), (2, lines(&["b"])), (3, Event::Flooded)]
        );
        hear(directory, 2).expect("cursor publishes");
        assert_eq!(
            after(directory, heard(directory).expect("cursor reads")).expect("events read"),
            vec![(3, Event::Flooded)]
        );
        assert!(heard_through(directory, 2));
        assert!(
            !heard_through(directory, 3),
            "an unheard event wakes the lane"
        );
        File::create(directory.join(DELIVERED)).expect("ending heard");
        assert!(heard_through(directory, 3), "a heard ending owes no wake");
        std::fs::remove_dir_all(&scratch).expect("scratch removes");
    }
}

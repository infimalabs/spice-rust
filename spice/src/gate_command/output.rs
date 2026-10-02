//! Decision-first terminal output shared by the gate commands.
//!
//! A gate command tells a person what it is doing and what it decided, not
//! everything its subprocesses said. [`Progress`] writes bounded live lines to
//! the diagnostic stream: a step's identity when it starts, an elapsed
//! heartbeat while it runs, and its disposition when it ends. Progress is best
//! effort; a lost write never changes a gate decision.
//!
//! Subprocess output never streams. [`retain`] keeps one complete capture as a
//! content-addressed file below the Git common directory, and a failure shows
//! only its bounded [`DiagnosticTail`] beside that file's path and digest.
//! [`Summary`] is the final reply: one headline, ordered decision rows, and the
//! exact next commands.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use wire::output::Export;
use wire::{Digest, Domain};

/// Interval between heartbeats while one step runs.
pub(super) const HEARTBEAT: Duration = Duration::from_secs(30);

/// Diagnostic lines a failure shows from the end of its captured output.
pub(super) const TAIL_LINES: usize = 40;

/// Bytes a failure shows from the end of its captured output.
pub(super) const TAIL_BYTES: usize = 4_096;

/// Retained capture files kept per repository; older ones are pruned.
const RETAINED_CAPTURES: usize = 64;

const CAPTURE_DIRECTORY: [&str; 2] = [".spice-rs", "gate-output"];

/// Cargo's machine-readable artifact records, which carry no diagnostic a
/// person needs once `json-render-diagnostics` has rendered the compiler's.
const CARGO_MESSAGE_PREFIX: &str = "{\"reason\":";

/// Best-effort live progress for one gate command.
pub(super) struct Progress {
    command: &'static str,
    sink: Arc<Mutex<dyn Write + Send>>,
    interval: Duration,
}

impl Progress {
    /// Progress on standard error with the production heartbeat.
    pub(super) fn stderr(command: &'static str) -> Self {
        Self::new(command, Arc::new(Mutex::new(io::stderr())), HEARTBEAT)
    }

    pub(super) fn new(
        command: &'static str,
        sink: Arc<Mutex<dyn Write + Send>>,
        interval: Duration,
    ) -> Self {
        Self {
            command,
            sink,
            interval,
        }
    }

    /// Write one progress line; a failed write or flush is ignored.
    pub(super) fn line(&self, text: fmt::Arguments<'_>) {
        let Ok(mut sink) = self.sink.lock() else {
            return;
        };
        let _ = writeln!(sink, "[{}] {text}", self.command).and_then(|()| sink.flush());
    }

    /// Run `work` while a heartbeat names `step` once per interval.
    ///
    /// The heartbeat thread ends before this returns, so no heartbeat follows
    /// the caller's completion line.
    pub(super) fn during<T>(&self, step: &str, work: impl FnOnce() -> T) -> (T, Duration) {
        let started = Instant::now();
        let (stop, stopped) = mpsc::channel::<()>();
        let value = std::thread::scope(|scope| {
            scope.spawn(move || {
                while matches!(
                    stopped.recv_timeout(self.interval),
                    Err(RecvTimeoutError::Timeout)
                ) {
                    self.line(format_args!(
                        "{step} running {}",
                        Elapsed(started.elapsed())
                    ));
                }
            });
            let value = work();
            drop(stop);
            value
        });
        (value, started.elapsed())
    }
}

/// A duration as a person reads it: `8.4s`, `2m05s`, `1h03m`.
#[derive(Clone, Copy)]
pub(super) struct Elapsed(pub(super) Duration);

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;

impl fmt::Display for Elapsed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0.as_secs();
        let (minutes, hours) = (seconds / SECONDS_PER_MINUTE, seconds / SECONDS_PER_HOUR);
        if minutes == 0 {
            write!(formatter, "{:.1}s", self.0.as_secs_f64())
        } else if hours == 0 {
            write!(formatter, "{minutes}m{:02}s", seconds % SECONDS_PER_MINUTE)
        } else {
            write!(formatter, "{hours}h{:02}m", minutes % SECONDS_PER_MINUTE)
        }
    }
}

/// The directory complete captures are retained in.
pub(super) fn capture_directory(common_git: &Path) -> PathBuf {
    CAPTURE_DIRECTORY
        .iter()
        .fold(common_git.to_path_buf(), |path, part| path.join(part))
}

/// Retain one complete capture as `DIGEST.log` and prune the oldest beyond
/// the retention bound.
pub(super) fn retain(directory: &Path, bytes: &[u8]) -> Result<Export, String> {
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join(format!("{}.log", Digest::compute(Domain::CliExport, bytes)));
    let export = spice::export::export(&path, bytes).map_err(|refusal| {
        let detail = refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.detail());
        format!("{}: {}", refusal.reason(), detail.unwrap_or_default())
    })?;
    prune(directory, &path);
    Ok(export)
}

fn prune(directory: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut captures = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "log")
        })
        .filter(|entry| entry.path() != keep)
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect::<Vec<(SystemTime, PathBuf)>>();
    if captures.len() < RETAINED_CAPTURES {
        return;
    }
    captures.sort();
    let excess = captures.len() + 1 - RETAINED_CAPTURES;
    for (_, path) in captures.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// The end of a captured output a person reads after a failure.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DiagnosticTail {
    /// Kept diagnostic text, ending at the capture's end.
    pub(super) text: String,
    /// Diagnostic lines kept.
    pub(super) shown_lines: usize,
    /// Diagnostic lines in the whole capture.
    pub(super) lines: usize,
    /// Diagnostic bytes before the kept text.
    pub(super) withheld_bytes: usize,
}

impl DiagnosticTail {
    /// Keep at most `max_lines` final diagnostic lines in at most `max_bytes`.
    ///
    /// Cargo's machine records are not diagnostics and are dropped first; the
    /// cut lands on a UTF-8 boundary.
    pub(super) fn of(capture: &[u8], max_lines: usize, max_bytes: usize) -> Self {
        let capture = String::from_utf8_lossy(capture);
        let diagnostic = capture
            .lines()
            .filter(|line| !line.starts_with(CARGO_MESSAGE_PREFIX))
            .collect::<Vec<_>>();
        let total = diagnostic.iter().map(|line| line.len() + 1).sum::<usize>();
        let first = diagnostic.len().saturating_sub(max_lines);
        let mut text = diagnostic[first..].join("\n");
        if text.len() > max_bytes {
            let start = text.ceil_char_boundary(text.len() - max_bytes);
            text.drain(..start);
        }
        let shown_lines = if text.is_empty() {
            0
        } else {
            text.lines().count()
        };
        let shown_bytes = if text.is_empty() { 0 } else { text.len() + 1 };
        Self {
            shown_lines,
            lines: diagnostic.len(),
            withheld_bytes: total.saturating_sub(shown_bytes),
            text,
        }
    }
}

impl fmt::Display for DiagnosticTail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.lines == 0 {
            return formatter.write_str("(no diagnostic output)");
        }
        writeln!(
            formatter,
            "last {} of {} diagnostic lines ({} earlier bytes withheld):",
            self.shown_lines, self.lines, self.withheld_bytes,
        )?;
        formatter.write_str(&self.text)
    }
}

/// A failed step's diagnostic: its bounded tail and where the complete
/// capture lives.
pub(super) fn failure_evidence(directory: &Path, capture: &[u8]) -> String {
    let tail = DiagnosticTail::of(capture, TAIL_LINES, TAIL_BYTES);
    let retained = match retain(directory, capture) {
        Ok(export) => format!("complete output: {} ({} bytes)", export.path, export.bytes),
        Err(error) => format!("complete output was not retained: {error}"),
    };
    format!("{tail}\n{retained}")
}

/// The final decision-first reply of one gate command.
pub(super) struct Summary {
    text: String,
}

impl Summary {
    pub(super) fn new(headline: fmt::Arguments<'_>) -> Self {
        Self {
            text: headline.to_string(),
        }
    }

    /// One decision: its verdict, subject, and elapsed time when it ran.
    pub(super) fn row(&mut self, verdict: &str, subject: &str, elapsed: Option<Duration>) {
        self.text.push_str(&format!("\n  {verdict:<7} {subject}"));
        if let Some(elapsed) = elapsed {
            self.text.push_str(&format!("  {}", Elapsed(elapsed)));
        }
    }

    /// Bounded detail under the previous row.
    pub(super) fn detail(&mut self, detail: fmt::Arguments<'_>) {
        self.text.push_str(&format!("\n          {detail}"));
    }

    /// One closing line such as `next: COMMAND`.
    pub(super) fn closing(&mut self, label: &str, text: &str) {
        self.text.push_str(&format!("\n{label}: {text}"));
    }

    pub(super) fn finish(self) -> String {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Shared {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("sink lock").clone()).expect("UTF-8 progress")
        }
    }

    impl Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("sink lock").extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_running_step_beats_until_it_ends_and_never_after() {
        let shared = Shared::default();
        let progress = Progress::new(
            "gate test",
            Arc::new(Mutex::new(shared.clone())),
            Duration::from_millis(1),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let (value, _) = progress.during("1/1 shard", || {
            while shared.text().matches("running").count() < 3 {
                assert!(Instant::now() < deadline, "heartbeats never arrived");
                std::thread::sleep(Duration::from_millis(1));
            }
            7
        });
        assert_eq!(value, 7);
        let ended = shared.text();
        assert!(
            ended
                .lines()
                .all(|line| line.starts_with("[gate test] 1/1 shard running ")),
            "{ended}"
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(shared.text(), ended, "no heartbeat follows completion");
    }

    #[test]
    fn a_tail_drops_cargo_records_keeps_the_end_and_counts_what_it_withheld() {
        let mut capture = String::new();
        for index in 0..100 {
            capture.push_str(&format!(
                "{{\"reason\":\"compiler-artifact\",\"n\":{index}}}\n"
            ));
            capture.push_str(&format!("line {index}\n"));
        }
        capture.push_str("error: naïve 🦀 failure\n");

        let tail = DiagnosticTail::of(capture.as_bytes(), 3, 4_096);
        assert_eq!(tail.text, "line 98\nline 99\nerror: naïve 🦀 failure");
        assert_eq!((tail.shown_lines, tail.lines), (3, 101));
        let diagnostic_bytes = capture
            .lines()
            .filter(|line| !line.starts_with('{'))
            .map(|line| line.len() + 1)
            .sum::<usize>();
        assert_eq!(tail.withheld_bytes, diagnostic_bytes - tail.text.len() - 1);

        let cut = DiagnosticTail::of("aaaa\n🦀🦀\n".as_bytes(), 10, 6);
        assert_eq!(cut.text, "🦀", "the byte cut never splits a character");
        assert_eq!(cut.withheld_bytes, 14 - 5);
        assert_eq!(
            DiagnosticTail::of(b"", 10, 10).to_string(),
            "(no diagnostic output)"
        );
    }

    #[test]
    fn a_failure_shows_its_tail_and_names_a_file_holding_every_byte() {
        let directory = fixture_directory();
        let mut capture = Vec::new();
        for index in 0..5_000 {
            capture.extend_from_slice(
                format!("{{\"reason\":\"compiler-artifact\",\"n\":{index}}}\nwarning {index}\n")
                    .as_bytes(),
            );
        }
        capture.extend_from_slice(b"error[E0308]: mismatched types\n");

        let evidence = failure_evidence(&directory, &capture);
        assert!(evidence.len() < TAIL_BYTES + 512, "{evidence}");
        assert!(!evidence.contains("compiler-artifact"), "{evidence}");
        assert!(evidence.contains("of 5001 diagnostic lines"), "{evidence}");
        let path = evidence
            .lines()
            .last()
            .and_then(|line| line.strip_prefix("complete output: "))
            .and_then(|line| line.split(" (").next())
            .expect("evidence names the complete capture");
        assert_eq!(fs::read(path).expect("capture reads"), capture);
        assert!(evidence.contains("error[E0308]: mismatched types\ncomplete output: "));
        fs::remove_dir_all(&directory).expect("fixture directory removes");
    }

    fn fixture_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "spice-gate-output-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn retention_names_the_complete_capture_by_digest_and_prunes_the_oldest() {
        let directory = fixture_directory();
        let first = retain(&directory, b"first capture\n").expect("first capture retains");
        assert_eq!(
            fs::read(&first.path).expect("capture reads"),
            b"first capture\n"
        );
        assert_eq!(
            first.digest,
            Digest::compute(Domain::CliExport, b"first capture\n")
        );
        for index in 0..RETAINED_CAPTURES + 5 {
            std::thread::sleep(Duration::from_millis(2));
            retain(&directory, format!("capture {index}\n").as_bytes()).expect("capture retains");
        }
        let kept = fs::read_dir(&directory).expect("directory lists").count();
        assert_eq!(kept, RETAINED_CAPTURES);
        assert!(
            !Path::new(&first.path).exists(),
            "the oldest capture is pruned"
        );
        fs::remove_dir_all(&directory).expect("fixture directory removes");
    }
}

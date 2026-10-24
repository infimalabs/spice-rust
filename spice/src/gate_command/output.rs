//! Decision-first terminal output shared by the gate commands and the
//! developer Cargo surfaces.
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
pub(crate) const HEARTBEAT: Duration = Duration::from_secs(30);

/// Diagnostic lines a failure shows from the end of its captured output.
pub(crate) const TAIL_LINES: usize = 40;

/// Bytes a failure shows from the end of its captured output.
pub(crate) const TAIL_BYTES: usize = 4_096;

/// Retained capture files kept per repository; older ones are pruned.
const RETAINED_CAPTURES: usize = 64;

const CAPTURE_DIRECTORY: &str = "gate-output";

/// Cargo's machine-readable artifact records, which carry no diagnostic a
/// person needs once `json-render-diagnostics` has rendered the compiler's.
const CARGO_MESSAGE_PREFIX: &str = "{\"reason\":";

/// Best-effort live progress for one gate command.
pub(crate) struct Progress {
    command: &'static str,
    sink: Arc<Mutex<dyn Write + Send>>,
    interval: Duration,
}

impl Progress {
    /// Progress on standard error with the production heartbeat, or nowhere
    /// when the invocation is not `live`: under `--json` standard error
    /// carries exactly one document, so progress would corrupt it.
    pub(super) fn stderr(command: &'static str, live: bool) -> Self {
        let sink: Arc<Mutex<dyn Write + Send>> = if live {
            Arc::new(Mutex::new(io::stderr()))
        } else {
            Arc::new(Mutex::new(io::sink()))
        };
        Self::new(command, sink, HEARTBEAT)
    }

    pub(crate) fn new(
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
    pub(crate) fn line(&self, text: fmt::Arguments<'_>) {
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
        self.watching(|| step.to_owned(), work)
    }

    /// Run `work` while a heartbeat names whatever `phase` reads at each beat.
    pub(crate) fn watching<T>(
        &self,
        phase: impl Fn() -> String + Sync,
        work: impl FnOnce() -> T,
    ) -> (T, Duration) {
        let started = Instant::now();
        let (stop, stopped) = mpsc::channel::<()>();
        let phase = &phase;
        let value = std::thread::scope(|scope| {
            scope.spawn(move || {
                while matches!(
                    stopped.recv_timeout(self.interval),
                    Err(RecvTimeoutError::Timeout)
                ) {
                    self.line(format_args!(
                        "{} running {}",
                        phase(),
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

    /// Whether a run of `elapsed` outlasted one heartbeat interval.
    pub(super) fn beat_during(&self, elapsed: Duration) -> bool {
        elapsed >= self.interval
    }
}

/// The phase a heartbeat names; the work updates it as it moves.
#[derive(Clone)]
pub(super) struct Phase(Arc<Mutex<String>>);

impl Phase {
    pub(super) fn new(initial: &str) -> Self {
        Self(Arc::new(Mutex::new(initial.to_owned())))
    }

    pub(super) fn set(&self, phase: String) {
        if let Ok(mut current) = self.0.lock() {
            *current = phase;
        }
    }

    pub(super) fn get(&self) -> String {
        self.0
            .lock()
            .map_or_else(|_| "unavailable".to_owned(), |current| current.clone())
    }
}

/// What one recognized protocol line means to a person.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Event {
    /// The phase a heartbeat names from here on.
    pub(super) phase: Option<String>,
    /// A progress line worth printing now.
    pub(super) line: Option<String>,
}

/// Who reads the recognized lines of a [`PhaseLog`].
pub(super) enum Audience<'a> {
    /// A person: recognized lines become bounded progress lines.
    Person(&'a Progress),
    /// A supervising gate that parses the protocol: recognized lines pass
    /// through verbatim, best effort.
    Supervisor(Box<dyn Write + Send + 'a>),
}

/// A progress writer for gate work that speaks a line protocol.
///
/// Every byte lands in the complete capture. Only lines `translate`
/// recognizes reach the audience, and each moves the heartbeat's phase, so
/// subprocess chatter is retained without ever streaming.
pub(super) struct PhaseLog<'a> {
    audience: Audience<'a>,
    phase: Phase,
    translate: fn(&str) -> Option<Event>,
    capture: Vec<u8>,
    line_start: usize,
}

impl<'a> PhaseLog<'a> {
    pub(super) fn new(
        audience: Audience<'a>,
        phase: Phase,
        translate: fn(&str) -> Option<Event>,
    ) -> Self {
        Self {
            audience,
            phase,
            translate,
            capture: Vec::new(),
            line_start: 0,
        }
    }

    /// Every byte the work wrote, without copying it.
    pub(super) fn into_capture(self) -> Vec<u8> {
        self.capture
    }

    fn recognize(&mut self, end: usize) {
        let line = String::from_utf8_lossy(&self.capture[self.line_start..end]);
        let line = line.trim_end_matches(['\n', '\r']);
        let Some(event) = (self.translate)(line) else {
            return;
        };
        match &mut self.audience {
            Audience::Person(progress) => {
                if let Some(text) = &event.line {
                    progress.line(format_args!("{text}"));
                }
            }
            Audience::Supervisor(sink) => {
                let _ = writeln!(sink, "{line}").and_then(|()| sink.flush());
            }
        }
        if let Some(phase) = event.phase {
            self.phase.set(phase);
        }
    }
}

impl Write for PhaseLog<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let offset = self.capture.len();
        self.capture.extend_from_slice(bytes);
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                let end = offset + index + 1;
                self.recognize(end);
                self.line_start = end;
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Longest phase label a heartbeat carries.
const PHASE_CHARS: usize = 96;

/// The change gate's progress protocol, read for a person.
///
/// Phase and shard boundaries print; arm and test-shard starts only move the
/// heartbeat's phase; budgets, schedules, and subprocess output stay in the
/// capture.
pub(super) fn change_gate_event(line: &str) -> Option<Event> {
    if let Some(rest) = line.strip_prefix("[change-gate-phase] stage=") {
        let stage = rest.split_whitespace().next()?;
        if let Some(stage) = stage.strip_suffix("-terminal") {
            return Some(Event {
                phase: None,
                line: Some(finished(stage, rest)),
            });
        }
        let changes =
            field(rest, "changes").map_or_else(String::new, |n| format!(" ({n} changes)"));
        return Some(Event {
            phase: Some(stage.to_owned()),
            line: Some(format!("{stage} started{changes}")),
        });
    }
    if let Some(rest) = line.strip_prefix("[change-gate-policy] gate=") {
        let gate = rest.split_whitespace().next()?;
        return Some(if field(rest, "stage") == Some("terminal") {
            Event {
                phase: None,
                line: Some(finished(&format!("arm {gate}"), rest)),
            }
        } else {
            Event {
                phase: Some(format!("arm {gate}")),
                line: None,
            }
        });
    }
    if let Some(rest) = line.strip_prefix("[change-gate-budget-warning] ") {
        let reason = field(rest, "reason").unwrap_or("unspecified");
        return Some(Event {
            phase: None,
            line: Some(format!("budget warning: {reason}")),
        });
    }
    let rest = line.strip_prefix("[rust-test] ")?;
    if let Some(rest) = rest.strip_prefix("coordinator ") {
        return Some(Event {
            phase: None,
            line: Some(finished("tests", rest)),
        });
    }
    let stage = field(rest, "stage")?;
    let subject = field(rest, "shard").map_or_else(
        || format!("tests {stage}"),
        |shard| {
            let package = field(rest, "package").unwrap_or_default().trim_matches('"');
            format!("tests shard {shard} {package}")
        },
    );
    let subject = clip(&subject, PHASE_CHARS);
    Some(match stage {
        "start" => Event {
            line: Some(format!("{subject} started")),
            phase: Some(subject),
        },
        "terminal" => Event {
            phase: None,
            line: Some(finished(&subject, rest)),
        },
        _ if stage.ends_with("-terminal") => {
            let disposition = field(rest, "disposition").unwrap_or_default();
            Event {
                phase: None,
                line: (!matches!(disposition, "success" | "reused"))
                    .then(|| finished(&subject, rest)),
            }
        }
        _ => Event {
            phase: Some(subject),
            line: None,
        },
    })
}

/// `SUBJECT DISPOSITION in ELAPSED` from a terminal protocol line.
fn finished(subject: &str, rest: &str) -> String {
    let disposition = field(rest, "disposition").unwrap_or("finished");
    match field(rest, "elapsed_ms").and_then(|ms| ms.parse::<u64>().ok()) {
        Some(ms) => format!(
            "{subject} {disposition} in {}",
            Elapsed(Duration::from_millis(ms))
        ),
        None => format!("{subject} {disposition}"),
    }
}

/// The value of one `name=value` word.
fn field<'a>(rest: &'a str, name: &str) -> Option<&'a str> {
    rest.split_whitespace()
        .find_map(|word| word.strip_prefix(name)?.strip_prefix('='))
}

/// `text` cut to at most `chars` characters, marked when cut.
pub(crate) fn clip(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// One diagnostic as a person reads it: whole when it is small, otherwise
/// its bounded tail with the complete text retained by digest.
pub(super) fn bounded(directory: &Path, text: &str) -> String {
    let tail = DiagnosticTail::of(text.as_bytes(), TAIL_LINES, TAIL_BYTES);
    if tail.withheld_bytes == 0 && tail.shown_lines == tail.lines {
        return text.to_owned();
    }
    failure_evidence(directory, text.as_bytes())
}

/// The one-word decision a completion line names.
pub(super) const fn verdict<B>(outcome: &wire::Outcome<B>) -> &'static str {
    match outcome {
        wire::Outcome::Reply(_) => "admitted",
        wire::Outcome::Refusal(_) => "refused",
        wire::Outcome::Fault(_) => "faulted",
    }
}

/// Where the current repository retains complete gate output; a temporary
/// directory when no repository is discoverable here.
pub(super) fn current_captures() -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|current| spice::repository::Repository::discover(&current).ok())
        .map_or_else(
            || std::env::temp_dir().join("spice-gate-output"),
            |repository| capture_directory(repository.common_dir()),
        )
}

/// A battery family as progress and summaries name it.
pub(super) const fn family_name(family: gate::ArmFamily) -> &'static str {
    match family {
        gate::ArmFamily::Core => "core",
        gate::ArmFamily::Toolchain => "toolchain",
        gate::ArmFamily::Metrics => "metrics",
        gate::ArmFamily::Hygiene => "hygiene",
    }
}

/// A duration as a person reads it: `8.4s`, `2m05s`, `1h03m`.
#[derive(Clone, Copy)]
pub(crate) struct Elapsed(pub(crate) Duration);

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
    wire::state_root(common_git).join(CAPTURE_DIRECTORY)
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

pub(crate) fn prune(directory: &Path, keep: &Path) {
    prune_to(directory, keep, RETAINED_CAPTURES);
}

fn prune_to(directory: &Path, keep: &Path, capacity: usize) {
    debug_assert!(capacity > 0);
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
    if captures.len() < capacity {
        return;
    }
    captures.sort();
    let excess = captures.len() + 1 - capacity;
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

    /// A sink that also announces each write, so a test blocks on the
    /// heartbeat it expects instead of re-reading the sink.
    struct Announcing {
        sink: Shared,
        written: std::sync::mpsc::Sender<()>,
    }

    impl Write for Announcing {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let written = self.sink.write(bytes)?;
            let _ = self.written.send(());
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The condition `until` awaits over the sink's text.
    type Reached<'a> = &'a dyn Fn(&str) -> bool;

    /// A progress writer whose sink `shared` reads and whose writes `until`
    /// awaits.
    fn announcing(
        label: &'static str,
        interval: Duration,
    ) -> (Progress, Shared, impl Fn(Reached<'_>)) {
        let shared = Shared::default();
        let (written, writes) = std::sync::mpsc::channel();
        let progress = Progress::new(
            label,
            Arc::new(Mutex::new(Announcing {
                sink: shared.clone(),
                written,
            })),
            interval,
        );
        let observed = shared.clone();
        let until = move |reached: &dyn Fn(&str) -> bool| {
            while !reached(&observed.text()) {
                writes
                    .recv_timeout(Duration::from_secs(10))
                    .expect("the heartbeat writes again");
            }
        };
        (progress, shared, until)
    }

    #[test]
    fn a_running_step_beats_until_it_ends() {
        let (progress, shared, until) = announcing("gate test", Duration::from_millis(1));
        let (value, _) = progress.during("1/1 shard", || {
            until(&|text| text.matches("running").count() >= 3);
            7
        });
        assert_eq!(value, 7);
        // The heartbeat runs in a scope that joins before `during` returns.
        let ended = shared.text();
        assert!(
            ended
                .lines()
                .all(|line| line.starts_with("[gate test] 1/1 shard running ")),
            "{ended}"
        );
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
    fn retention_names_the_complete_capture_by_digest_and_prunes_to_its_capacity() {
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
        fs::remove_file(first.path).expect("digest fixture removes");

        for name in ["a.log", "b.log", "c.log", "keep.log"] {
            fs::write(directory.join(name), name).expect("capture fixture writes");
        }
        let keep = directory.join("keep.log");
        prune_to(&directory, &keep, 3);

        assert!(!directory.join("a.log").exists(), "oldest capture remains");
        assert!(directory.join("b.log").exists());
        assert!(directory.join("c.log").exists());
        assert!(keep.exists());
        assert_eq!(
            fs::read_dir(&directory).expect("directory lists").count(),
            3
        );
        fs::remove_dir_all(&directory).expect("fixture directory removes");
    }

    /// A noisy successful run as the change gate writes it: protocol lines
    /// interleaved with compiler chatter, split across arbitrary writes.
    const NOISY_RUN: &str = concat!(
        "[change-gate-phase] stage=selection changes=2\n",
        "   Compiling spice v0.1.0\n",
        "{\"reason\":\"compiler-artifact\",\"n\":1}\n",
        "[change-gate-budget] cpus=12 ordinary_lanes=4\n",
        "[change-gate-budget-warning] reason=advisory-lane-share-exceeded host_capacity=12\n",
        "[change-gate-policy] gate=repo-docs disposition=executed\n",
        "warning: unused variable `x`\n",
        "[change-gate-policy] gate=repo-docs stage=terminal elapsed_ms=1200 disposition=success\n",
        "[rust-test] stage=metadata-terminal elapsed_ms=40 disposition=success\n",
        "[rust-test] stage=start shard=1/2 package=\"spice\" \n",
        "test a ... ok\n",
        "[rust-test] stage=progress shard=1/2 package=\"spice\" targets=[\"lib\"] \n",
        "[rust-test] stage=terminal shard=1/2 package=\"spice\" targets=[\"lib\"] elapsed_ms=2500 disposition=success\n",
        "[rust-test] coordinator disposition=success elapsed_ms=3000 peak_concurrency=2 \n",
    );

    fn write_in_pieces(log: &mut PhaseLog<'_>, text: &str) {
        for piece in text.as_bytes().chunks(7) {
            log.write_all(piece).expect("phase log accepts every byte");
        }
    }

    #[test]
    fn a_person_sees_only_phase_boundaries_while_the_capture_keeps_every_byte() {
        let shared = Shared::default();
        let progress = Progress::new(
            "gate selection",
            Arc::new(Mutex::new(shared.clone())),
            HEARTBEAT,
        );
        let phase = Phase::new("capture");
        let mut log = PhaseLog::new(
            Audience::Person(&progress),
            phase.clone(),
            change_gate_event,
        );
        write_in_pieces(&mut log, NOISY_RUN);

        assert_eq!(log.into_capture(), NOISY_RUN.as_bytes());
        assert_eq!(
            shared.text(),
            concat!(
                "[gate selection] selection started (2 changes)\n",
                "[gate selection] budget warning: advisory-lane-share-exceeded\n",
                "[gate selection] arm repo-docs success in 1.2s\n",
                "[gate selection] tests shard 1/2 spice started\n",
                "[gate selection] tests shard 1/2 spice success in 2.5s\n",
                "[gate selection] tests success in 3.0s\n",
            ),
            "chatter, budgets, and successful sub-stages stay in the capture"
        );
        assert_eq!(phase.get(), "tests shard 1/2 spice");
    }

    #[test]
    fn a_supervisor_receives_every_protocol_line_verbatim_and_no_chatter() {
        let shared = Shared::default();
        let mut log = PhaseLog::new(
            Audience::Supervisor(Box::new(shared.clone())),
            Phase::new("capture"),
            change_gate_event,
        );
        write_in_pieces(&mut log, NOISY_RUN);

        let protocol = NOISY_RUN
            .lines()
            .filter(|line| {
                [
                    "[change-gate-phase] ",
                    "[change-gate-policy] ",
                    "[change-gate-budget-warning] ",
                    "[rust-test] ",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix))
            })
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        assert_eq!(shared.text(), protocol);
        assert_eq!(log.into_capture(), NOISY_RUN.as_bytes());
    }

    #[test]
    fn a_heartbeat_names_the_phase_the_work_has_reached() {
        let (progress, shared, until) = announcing("gate selection", Duration::from_millis(1));
        let phase = Phase::new("capture");
        let ((), elapsed) = progress.watching(
            || phase.get(),
            || {
                until(&|text| text.contains("capture running "));
                phase.set("arm rust-typecheck".to_owned());
                until(&|text| text.contains("arm rust-typecheck running "));
            },
        );
        assert!(progress.beat_during(elapsed));
        let ended = shared.text();
        assert!(
            ended.lines().all(|line| {
                line.starts_with("[gate selection] capture running ")
                    || line.starts_with("[gate selection] arm rust-typecheck running ")
            }),
            "{ended}"
        );
    }

    #[test]
    fn an_oversized_detail_shows_a_bounded_tail_and_retains_every_byte() {
        let directory = fixture_directory();
        let short = "selected gate regressed\nrepo-docs: refused";
        assert_eq!(bounded(&directory, short), short);
        assert!(
            !directory.exists(),
            "a detail within bounds retains nothing"
        );

        let oversized = (0..20_000)
            .map(|index| format!("stdout line {index}\n"))
            .collect::<String>();
        let shown = bounded(&directory, &oversized);
        assert!(shown.len() < TAIL_BYTES + 512, "{shown}");
        assert!(shown.contains("stdout line 19999\n"), "{shown}");
        assert!(!shown.contains("stdout line 0\n"), "{shown}");
        assert!(shown.contains("of 20000 diagnostic lines"), "{shown}");
        let path = shown
            .lines()
            .last()
            .and_then(|line| line.strip_prefix("complete output: "))
            .and_then(|line| line.split(" (").next())
            .expect("a bounded detail names the complete capture");
        assert_eq!(fs::read_to_string(path).expect("capture reads"), oversized);
        fs::remove_dir_all(&directory).expect("fixture directory removes");
    }
}

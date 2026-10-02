use std::os::unix::process::ExitStatusExt as _;

use super::*;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("sink lock").clone()).expect("UTF-8 report")
    }
}

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("sink lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One fixture child at a time: where pipe creation and close-on-exec are two
/// steps (Apple targets), a concurrent test's child can inherit this child's
/// output pipe and hold it open, which the drain bound would then report.
static SPAWN: Mutex<()> = Mutex::new(());

struct Fixture {
    directory: PathBuf,
    sink: Buffer,
}

impl Fixture {
    fn new() -> Self {
        Self {
            // A legal Git common directory a bare shell word would split,
            // glob, and substitute.
            directory: std::env::temp_dir().join(format!(
                "spice-cargo-output-{}-{} it's $(exit 7) *",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            )),
            sink: Buffer::default(),
        }
    }

    fn run(&self, command: &mut Command, verbose: bool) -> (ExitStatus, String) {
        let settings = Settings {
            surface: "dev cargo",
            identity: "cargo fixture".to_owned(),
            sink: Arc::new(Mutex::new(self.sink.clone())),
            heartbeat: Duration::from_secs(3_600),
            verbose,
            directory: self.directory.clone(),
        };
        let _serial = SPAWN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let status =
            run(command, Stdout::Capture, &settings, |_| Ok(())).expect("captured child runs");
        (status, self.sink.text())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn sh(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    command
}

/// The retained capture a report names.
fn retained(report: &str) -> Vec<u8> {
    let path = report
        .lines()
        .find_map(|line| {
            line.strip_prefix("complete output: ")
                .and_then(|rest| rest.split(" (").next())
                .or_else(|| line.split("; log ").nth(1))
        })
        .expect("report names the retained capture");
    fs::read(path).expect("retained capture reads")
}

#[test]
fn noisy_success_prints_one_counted_line_and_retains_every_byte() {
    let fixture = Fixture::new();
    let (status, report) = fixture.run(
        &mut sh(concat!(
            "seq 1 3000 | sed 's/^/test noise::/' ; ",
            "seq 1 3 | sed 's/^/   Compiling crate-/' >&2 ; ",
            "echo 'warning: `fixture` (lib) generated 2 warnings' >&2 ; ",
            "echo 'test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out' ; ",
            "echo 'test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out'",
        )),
        false,
    );

    assert!(status.success());
    assert_eq!(report.lines().count(), 1, "{report}");
    assert!(
        report.starts_with("[dev cargo] cargo fixture ok in "),
        "{report}"
    );
    assert!(
        report.contains(
            "tests 12 passed, 0 failed, 1 ignored in 2 suites; 3 crates compiled; 2 warnings"
        ),
        "{report}"
    );
    assert!(!report.contains("noise"), "{report}");
    let capture = retained(&report);
    let text = String::from_utf8(capture).expect("fixture output is UTF-8");
    assert_eq!(text.matches("test noise::").count(), 3_000);
    assert!(text.contains("   Compiling crate-3\n"));
}

#[test]
fn noisy_failure_shows_a_bounded_tail_with_truncation_facts_and_an_inspection_command() {
    let fixture = Fixture::new();
    let (status, report) = fixture.run(
        &mut sh("seq 1 5000 | sed 's/^/chatter /' >&2 ; echo 'error: boom' >&2 ; exit 101"),
        false,
    );

    assert_eq!(status.code(), Some(101));
    assert!(
        report.contains("cargo fixture failed (exit 101) in "),
        "{report}"
    );
    assert!(report.len() < TAIL_BYTES + 1_024, "{report}");
    let capture = retained(&report);
    let shown = format!(
        "{}\n",
        (4_962..=5_000)
            .map(|line| format!("chatter {line}"))
            .chain(["error: boom".to_owned()])
            .collect::<Vec<_>>()
            .join("\n")
    );
    let withheld = capture.len() - shown.len();
    assert!(
        report.contains(&format!(
            "last {TAIL_LINES} of 5001 lines ({withheld} earlier bytes withheld):\n{}",
            shown.trim_end()
        )),
        "{report}"
    );
    let inspect = report
        .lines()
        .find_map(|line| line.strip_prefix("inspect: "))
        .expect("failure names an inspection command");
    let inspected = Command::new("/bin/sh")
        .arg("-c")
        .arg(inspect)
        .output()
        .expect("inspection command runs");
    assert!(inspected.status.success(), "{inspect}");
    assert_eq!(inspected.stdout, b"5001:error: boom\n", "{inspect}");
}

#[test]
fn non_utf8_output_and_escape_sequences_never_reach_the_terminal_raw() {
    let fixture = Fixture::new();
    let (status, report) = fixture.run(
        &mut sh(r"printf '\377\376bad \033[31mred\n' >&2 ; exit 1"),
        false,
    );

    assert_eq!(status.code(), Some(1));
    assert!(!report.contains('\u{1b}'), "{report:?}");
    assert!(
        report.contains("\u{fffd}\u{fffd}bad \u{fffd}[31mred"),
        "{report:?}"
    );
    assert_eq!(retained(&report), b"\xff\xfebad \x1b[31mred\n");
}

#[test]
fn a_child_ended_by_a_deadline_signal_reports_it_and_keeps_its_capture() {
    let fixture = Fixture::new();
    let (status, report) = fixture.run(&mut sh("echo started ; kill -TERM $$"), false);

    assert_eq!(status.signal(), Some(signal_hook::consts::SIGTERM));
    assert!(
        report.contains("cargo fixture terminated by SIGTERM in "),
        "{report}"
    );
    assert!(
        report.contains("):\nstarted\ncomplete output: "),
        "{report}"
    );
    assert_eq!(retained(&report), b"started\n");
}

#[test]
fn verbose_streams_live_lines_only_up_to_its_bound() {
    let fixture = Fixture::new();
    let (status, report) = fixture.run(&mut sh("seq 1 500 >&2"), true);

    assert!(status.success());
    let live = report
        .lines()
        .filter(|line| line.starts_with("[dev cargo] ") && !line.contains(" ok in "))
        .collect::<Vec<_>>();
    assert_eq!(live.len(), LIVE_LINES + 1, "{report}");
    assert_eq!(live[0], "[dev cargo] 1");
    assert!(
        live[LIVE_LINES].contains("live output stops after"),
        "{report}"
    );
    assert_eq!(
        retained(&report).len(),
        (1..=500)
            .map(|line: u32| line.to_string().len() + 1)
            .sum::<usize>()
    );
}

#[test]
fn output_held_open_by_a_descendant_does_not_hold_the_run() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let (status, report) = fixture.run(&mut sh("sleep 10 & echo parent done"), false);

    assert!(status.success());
    assert!(started.elapsed() < Duration::from_secs(8), "{report}");
    assert!(
        report.contains("a descendant still held the output open"),
        "{report}"
    );
    assert_eq!(retained(&report), b"parent done\n");
}

#[test]
fn product_output_is_delivered_and_diagnostic_output_is_captured() {
    let cases: [(&[&str], Stdout); 14] = [
        (&["test", "--locked", "-p", "spice"], Stdout::Capture),
        (&["check", "--locked"], Stdout::Capture),
        (&["+nightly", "clippy"], Stdout::Capture),
        (&["canvas-wasm-typecheck"], Stdout::Capture),
        (&["test", "--", "--nocapture"], Stdout::Capture),
        (
            &["run", "--quiet", "-p", "spice", "--", "board"],
            Stdout::Deliver,
        ),
        (
            &["--locked", "metadata", "--format-version", "1"],
            Stdout::Deliver,
        ),
        (&["--config", "build.jobs=1", "tree"], Stdout::Deliver),
        (&["test", "-p", "spice", "--", "--list"], Stdout::Deliver),
        (
            &["build", "--message-format=json-render-diagnostics"],
            Stdout::Deliver,
        ),
        (&["check", "--message-format", "json"], Stdout::Deliver),
        (&["check", "--help"], Stdout::Deliver),
        (&["--version"], Stdout::Deliver),
        (&[], Stdout::Deliver),
    ];
    for (arguments, expected) in cases {
        let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(cargo_stdout(&arguments), expected, "{arguments:?}");
    }
}

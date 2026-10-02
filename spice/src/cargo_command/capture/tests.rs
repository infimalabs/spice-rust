use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::ExitStatusExt as _;

use super::*;

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("sink lock").clone()).expect("UTF-8 report")
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("sink lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Fixture {
    directory: PathBuf,
    sink: Sink,
}

impl Fixture {
    fn new() -> Self {
        Self {
            directory: std::env::temp_dir().join(format!(
                "spice-cargo-output-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            )),
            sink: Sink::default(),
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
    let path = report
        .lines()
        .find_map(|line| line.strip_prefix("complete output: "))
        .and_then(|rest| rest.split(" (").next())
        .expect("failure names the capture");
    assert!(
        report.ends_with(&format!(
            "inspect: grep -n -E 'error|warning|FAILED|panicked' {path}\n"
        )),
        "{report}"
    );
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
fn arguments_and_environment_reach_the_child_exactly() {
    let fixture = Fixture::new();
    let mut command = sh(r#"printf '[%s]\n' "$@" ; printf 'env=[%s]\n' "$TMPDIR""#);
    command
        .arg("sh")
        .args(["a b", "", "ünï"])
        .arg(std::ffi::OsStr::from_bytes(b"\xff"))
        .env("TMPDIR", "value with space"); // env-policy: allow
    let (status, report) = fixture.run(&mut command, false);

    assert!(status.success(), "{report}");
    let mut expected = "[a b]\n[]\n[ünï]\n[".as_bytes().to_vec();
    expected.extend_from_slice(b"\xff]\nenv=[value with space]\n");
    assert_eq!(retained(&report), expected);
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
    let (status, report) = fixture.run(&mut sh("sleep 30 & echo parent done"), false);

    assert!(status.success());
    assert!(started.elapsed() < Duration::from_secs(20), "{report}");
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

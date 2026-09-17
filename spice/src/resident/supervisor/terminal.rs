//! The lane's own terminal: every command the occupant's model composes runs
//! here, in the occupant's environment, under a deadline the lane enforces.
//!
//! omp's `bash` is left off the roster and the lane registers a host tool in
//! its place, so a command never runs inside the agent process. The lane
//! runs it through `sill` exactly as the agent's shell tool would have: the
//! occupant's `SHELL`, in its working directory, under its environment, so
//! the installed shell hooks intercept it the same way. What the command
//! says comes back as the tool's result, with its exit code where omp's own
//! `bash` keeps it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Output,
    ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome,
    WaitPolicy,
};
use vendor::omp::ShellCall;

/// Bytes of each stream a result keeps: the end, where a failure explains
/// itself.
const OUTPUT_LIMIT: usize = 64 * 1024;

/// How long the lane keeps draining a finished command's output.
const OUTPUT_DRAIN: Duration = Duration::from_millis(250);

/// How long the lane waits for a terminated command's group to be reaped.
const REAP_DEADLINE: Duration = Duration::from_millis(500);

/// How long a command has to leave on its own once the lane ends it.
const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// Where and as whom the occupant's commands run.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Terminal {
    shell: OsString,
    cwd: PathBuf,
    environment: Environment,
}

/// What one command said and how it ended, as the tool result carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct Report {
    pub(in crate::resident) output: String,
    pub(in crate::resident) exit_code: Option<i32>,
    pub(in crate::resident) failed: bool,
}

impl Terminal {
    /// The terminal that runs what the occupant's own shell tool would have.
    pub(in crate::resident) fn new(
        shell: impl Into<OsString>,
        cwd: impl Into<PathBuf>,
        environment: Environment,
    ) -> Self {
        Self {
            shell: shell.into(),
            cwd: cwd.into(),
            environment,
        }
    }

    /// The occupant's own terminal, read from its launch: its `SHELL`, in its
    /// working directory, under its environment. An occupant launched with
    /// no shell has no terminal.
    pub(in crate::resident) fn from_spec(spec: &Spec) -> Option<Self> {
        let shell = spec
            .environment
            .entries()
            .iter()
            .find(|(name, _)| name == "SHELL")
            .map(|(_, shell)| shell.clone())?;
        Some(Self::new(shell, spec.cwd(), spec.environment.clone()))
    }

    /// Run one command to its end, its deadline, or its cancellation.
    pub(in crate::resident) fn run(
        &self,
        call: &ShellCall,
        cancellation: CancellationToken,
    ) -> Report {
        let spec = match self.spec(call, cancellation) {
            Ok(spec) => spec,
            Err(reason) => return Report::refused(&reason),
        };
        let running = match sill::spawn(spec) {
            Ok(running) => running,
            Err(error) => return Report::refused(&format!("shell launch failed: {error}")),
        };
        match running.wait() {
            Ok(outcome) => report(&outcome, call.timeout),
            Err(error) => Report::refused(&format!("shell process boundary failed: {error}")),
        }
    }

    fn spec(&self, call: &ShellCall, cancellation: CancellationToken) -> Result<Spec, String> {
        let capture = StreamPolicy::CaptureTail {
            limit: OUTPUT_LIMIT,
        };
        Ok(Spec {
            argv: Argv::new(
                self.shell.clone(),
                ["-c".into(), OsString::from(&call.command)],
            )
            .map_err(|error| format!("invalid shell argv: {error}"))?,
            cwd: self.cwd.clone(),
            environment: self.environment.clone(),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(call.timeout, OUTPUT_DRAIN, REAP_DEADLINE)
                .map_err(|error| format!("invalid shell deadlines: {error}"))?,
            wait: WaitPolicy::new(
                "lane terminal",
                Cancellation::Token(cancellation),
                RestartDisposition::Never,
            )
            .map_err(|error| format!("invalid shell wait policy: {error}"))?,
            output: Output::new(StdinPolicy::Null, capture, capture),
            redaction: Redaction::None,
            grace: TERMINATE_GRACE,
        })
    }
}

impl Report {
    fn refused(reason: &str) -> Self {
        Self {
            output: reason.to_owned(),
            exit_code: None,
            failed: true,
        }
    }
}

/// Read a finished command as its result: both streams, then how it ended.
///
/// A nonzero exit is stated in omp's own words, so the model reads the same
/// notice it would under `bash`; a signal, deadline, or cancellation is
/// stated as such.
fn report(outcome: &WaitOutcome, timeout: Duration) -> Report {
    let mut output = String::new();
    let truncated = [outcome.stdout(), outcome.stderr()]
        .into_iter()
        .flatten()
        .any(sill::CapturedOutput::truncated);
    if truncated {
        output.push_str("[earlier output dropped; the last 64 KiB of each stream follow]\n");
    }
    for captured in [outcome.stdout(), outcome.stderr()].into_iter().flatten() {
        let text = String::from_utf8_lossy(captured.bytes());
        if text.is_empty() {
            continue;
        }
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&text);
    }
    let status = outcome.completion().status();
    let exit_code = status.code();
    let notice = match outcome.completion() {
        Completion::Exited(_) => match exit_code {
            Some(0) => None,
            Some(code) => Some(format!("Command exited with code {code}")),
            None => Some(format!("Command ended by signal {}", signal(status))),
        },
        Completion::TimedOut(_) => Some(format!("Command timed out after {}s", timeout.as_secs())),
        Completion::Cancelled(_) => Some("Command cancelled".to_owned()),
    };
    let failed = notice.is_some();
    if let Some(notice) = notice {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&notice);
    }
    Report {
        output,
        exit_code,
        failed,
    }
}

#[cfg(unix)]
fn signal(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().unwrap_or_default()
}

#[cfg(not(unix))]
fn signal(_status: std::process::ExitStatus) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(command: &str) -> ShellCall {
        ShellCall {
            command: command.to_owned(),
            timeout: Duration::from_secs(vendor::omp::SHELL_TIMEOUT_DEFAULT),
        }
    }

    #[test]
    fn a_command_reports_both_streams_and_a_failure_in_omps_own_words() {
        let terminal = Terminal::new("/bin/sh", std::env::temp_dir(), Environment::empty());
        let report = terminal.run(
            &call("printf out; printf err >&2; exit 3"),
            CancellationToken::new(),
        );
        assert_eq!(
            report,
            Report {
                output: "out\nerr\n\nCommand exited with code 3".to_owned(),
                exit_code: Some(3),
                failed: true,
            }
        );

        let report = terminal.run(&call("printf 'fine\\n'"), CancellationToken::new());
        assert_eq!(
            report,
            Report {
                output: "fine\n".to_owned(),
                exit_code: Some(0),
                failed: false,
            }
        );
    }

    #[test]
    fn a_cancelled_command_is_ended_and_reported_as_such() {
        let root = std::env::temp_dir().join(format!(
            "spice-terminal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        std::fs::create_dir_all(&root).expect("scratch directory");
        // A fifo nobody writes holds its reader open, so the command ends
        // only because the lane ends it.
        let fifo = root.join("hold");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        let terminal = Terminal::new("/bin/sh", &root, Environment::empty());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let report = terminal.run(&call("cat hold"), cancellation);
        let _ = std::fs::remove_dir_all(&root);
        assert!(report.failed);
        assert!(
            report.output.ends_with("Command cancelled"),
            "{}",
            report.output
        );
    }

    #[test]
    fn the_terminal_is_read_from_the_occupants_own_launch() {
        let environment =
            Environment::new([("SHELL", "/bin/sh"), ("HOME", "/nowhere")]).expect("environment");
        let spec = Spec {
            argv: Argv::new("omp", Vec::<OsString>::new()).expect("argv"),
            cwd: PathBuf::from("/work"),
            environment: environment.clone(),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(Duration::from_secs(1), OUTPUT_DRAIN, REAP_DEADLINE)
                .expect("deadlines"),
            wait: WaitPolicy::new("test", Cancellation::Never, RestartDisposition::Never)
                .expect("wait"),
            output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
            redaction: Redaction::None,
            grace: TERMINATE_GRACE,
        };
        let terminal = Terminal::from_spec(&spec).expect("a launch with a SHELL has a terminal");
        assert_eq!(terminal.shell, OsString::from("/bin/sh"));
        assert_eq!(terminal.cwd, PathBuf::from("/work"));
        assert_eq!(terminal.environment.entries(), environment.entries());

        let bare = Spec {
            environment: Environment::empty(),
            ..spec
        };
        assert!(Terminal::from_spec(&bare).is_none());
    }
}

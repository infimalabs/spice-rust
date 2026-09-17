//! The lane's own terminal: every command the occupant's model composes runs
//! here, in the occupant's environment, kept beyond the call that started it.
//!
//! omp's `bash` is left off the roster and the lane registers a host tool in
//! its place, so a command never runs inside the agent process. The lane
//! runs it in the occupant's native shell, in its working directory, under
//! its environment with the shell interception lifted: the lane is the
//! wrapper here, so it applies RTK itself and steers the occupant over its
//! own protocol rather than through a hook. Every command is kept as a
//! [`Job`] by a detached keeper, so it survives the turn and the lane can
//! answer a call without ending the command: a command that outruns its
//! foreground window, or that is running when a message arrives for the
//! occupant, is answered with what it has said so far and its ending is
//! delivered when it comes, in omp's own words for both. The model follows
//! kept commands through the `job` tool.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

use sill::{Environment, Spec};
use vendor::omp::{JobCall, JobOp, ShellCall};

pub(super) mod job;

pub(in crate::resident) use job::{Control, Handle, Job, Jobs};
use job::{Ending, Observed, Tail};

/// How long after a stop is asked the lane waits for the keeper's ending
/// before answering with the command still ending: the keeper's grace and
/// the reap that follows it.
const STOP_BOUND: Duration = Duration::from_secs(3);

/// omp's own notice when a message backgrounds the running command.
const STEERED: &str =
    "Backgrounded early to handle an incoming message; the command keeps running.";

/// The shell interception the occupant's launch carries and the terminal
/// lifts: the terminal runs the native shell itself, so no hook may re-enter
/// the wrapper from inside a command.
const INTERCEPTION: [&str; 5] = [
    "SHELL",
    steer::hook::BASH_ENV,
    steer::hook::ZDOTDIR,
    steer::hook::HOOKS,
    steer::hook::NATIVE_SHELL,
];

/// Where and as whom the occupant's commands run, and who keeps them.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Terminal {
    shell: OsString,
    cwd: PathBuf,
    environment: Environment,
    jobs: Jobs,
    /// How long a command runs in the foreground before it is kept.
    foreground: Duration,
}

/// What one call said and how it ended, as the tool result carries it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct Report {
    pub(in crate::resident) output: String,
    pub(in crate::resident) exit_code: Option<i32>,
    pub(in crate::resident) failed: bool,
}

/// How one command's call was answered.
#[derive(Debug)]
pub(in crate::resident) enum Run {
    /// The command ended within the call.
    Ended(Report),
    /// The call was answered with what the command said so far; the job
    /// keeps running and its ending is still to be delivered.
    Backgrounded(Report, Job),
}

impl Terminal {
    /// The terminal that runs what the occupant's own shell tool would have.
    pub(in crate::resident) fn new(
        shell: impl Into<OsString>,
        cwd: impl Into<PathBuf>,
        environment: Environment,
        jobs: Jobs,
    ) -> Self {
        Self {
            shell: shell.into(),
            cwd: cwd.into(),
            environment,
            jobs,
            foreground: Duration::from_secs(vendor::omp::SHELL_FOREGROUND),
        }
    }

    #[cfg(test)]
    fn foreground(self, foreground: Duration) -> Self {
        Self { foreground, ..self }
    }

    /// The occupant's own terminal, read from its launch: its native shell,
    /// in its working directory, under its environment with the interception
    /// lifted and `SHELL` naming the native shell. An occupant launched with
    /// no shell has no terminal.
    pub(in crate::resident) fn from_launch(spec: &Spec, jobs: Jobs) -> Option<Self> {
        let entries = spec.environment.entries();
        let value = |name: &str| {
            entries
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let shell = value(steer::hook::NATIVE_SHELL).or_else(|| value("SHELL"))?;
        let environment = Environment::new(
            entries
                .iter()
                .filter(|(name, _)| !INTERCEPTION.iter().any(|lifted| name == lifted))
                .cloned()
                .chain([(OsString::from("SHELL"), shell.clone())]),
        )
        .expect("lifting entries from a valid environment preserves validity");
        Some(Self::new(shell, spec.cwd(), environment, jobs))
    }

    /// Run one command: to its end within the foreground window, or until a
    /// message arrives for the occupant, after which it is kept.
    pub(in crate::resident) fn run(&self, call: &ShellCall, control: &Control) -> Run {
        let (argv, diagnostic) = self.argv(call);
        let started = self.jobs.start(
            &call.command,
            argv,
            &self.cwd,
            &self.environment,
            call.timeout,
        );
        let job = match started {
            Ok(job) => job,
            Err(error) => {
                return Run::Ended(Report::refused(&format!("shell launch failed: {error}")));
            }
        };
        let ending = match job.observe(control, self.foreground) {
            Ok(Observed::Ended(ending)) => ending,
            Ok(Observed::Cancelled) => stop(&job, control).unwrap_or(Ending::Stopped),
            Ok(Observed::Interrupted) => {
                return Run::Backgrounded(prefixed(kept(&job, &[STEERED]), diagnostic), job);
            }
            Ok(Observed::Elapsed) => {
                return Run::Backgrounded(prefixed(kept(&job, &[]), diagnostic), job);
            }
            Err(error) => Ending::Failed {
                reason: format!("job observation failed: {error}"),
            },
        };
        Run::Ended(prefixed(finished(&job, &ending), diagnostic))
    }

    /// Follow a kept command to its ending: what the delivery says.
    pub(in crate::resident) fn follow(&self, job: &Job, control: &Control) -> Report {
        loop {
            match job.observe(control, job.timeout() + STOP_BOUND) {
                Ok(Observed::Ended(ending)) => return finished(job, &ending),
                Ok(Observed::Elapsed) => return finished(job, &Ending::Lost),
                Ok(Observed::Cancelled | Observed::Interrupted) => {}
                Err(error) => {
                    return Report::refused(&format!(
                        "job {} could not be followed: {error}",
                        job.id()
                    ));
                }
            }
        }
    }

    /// Answer one request on the lane's jobs.
    pub(in crate::resident) fn job(&self, call: &JobCall, control: &Control) -> Report {
        let job = match call.id.as_deref().map(|id| self.jobs.open(id)) {
            Some(Ok(job)) => Some(job),
            Some(Err(error)) => return Report::refused(&error.to_string()),
            None => None,
        };
        match (call.op, job) {
            (JobOp::List, _) => self.list(),
            (JobOp::Logs, Some(job)) => snapshot(&job, job.ending().as_ref()),
            (JobOp::Wait, Some(job)) => match job.observe(control, call.timeout) {
                Ok(Observed::Ended(ending)) => snapshot(&job, Some(&ending)),
                Ok(Observed::Elapsed) => snapshot(&job, None),
                Ok(Observed::Interrupted) => still(&job, "wait interrupted by an incoming message"),
                Ok(Observed::Cancelled) => still(&job, "wait cancelled"),
                Err(error) => {
                    Report::refused(&format!("job {} could not be observed: {error}", job.id()))
                }
            },
            (JobOp::Stop, Some(job)) => match stop(&job, control) {
                Some(ending) => snapshot(&job, Some(&ending)),
                None => still(&job, "stop asked"),
            },
            (op, None) => Report::refused(&format!("job {} needs an id", op.name())),
        }
    }

    fn list(&self) -> Report {
        let jobs = match self.jobs.list() {
            Ok(jobs) => jobs,
            Err(error) => return Report::refused(&format!("jobs could not be listed: {error}")),
        };
        let mut output = String::new();
        for job in &jobs {
            let status = status(job.ending().as_ref());
            output.push_str(&format!("{}  {status}  {}\n", job.id(), job.command()));
        }
        if output.is_empty() {
            output.push_str("no jobs");
        }
        Report {
            output,
            exit_code: None,
            failed: false,
        }
    }

    /// The command as the shell runs it, rewritten by RTK when the launch
    /// configured one; a degraded rewrite runs the native command and says
    /// why, as the wrapper does.
    fn argv(&self, call: &ShellCall) -> (Vec<OsString>, Option<String>) {
        let native = vec![
            self.shell.clone(),
            OsString::from("-c"),
            OsString::from(&call.command),
        ];
        let Some(rtk) = self.variable(spice::rtk::EXECUTABLE_VARIABLE) else {
            return (native, None);
        };
        let decision = spice::rtk::rewrite_in(&native, rtk, &self.cwd, &self.environment);
        (
            decision.command().to_vec(),
            decision.diagnostic().map(str::to_owned),
        )
    }

    fn variable(&self, name: &str) -> Option<&OsStr> {
        self.environment
            .entries()
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_os_str())
    }
}

impl Job {
    /// How the job ended, if it has.
    fn ending(&self) -> Option<Ending> {
        self.state().ok().flatten().and_then(|state| state.ending)
    }
}

/// One job's standing in a listing.
fn status(ending: Option<&Ending>) -> String {
    match ending {
        None => "running".to_owned(),
        Some(Ending::Exited { code }) => format!("exited {code}"),
        Some(Ending::Signalled { signal }) => format!("signal {signal}"),
        Some(Ending::TimedOut) => "timed out".to_owned(),
        Some(Ending::Stopped) => "stopped".to_owned(),
        Some(Ending::Failed { .. }) => "failed".to_owned(),
        Some(Ending::Lost) => "lost".to_owned(),
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

/// Ask the keeper to end the command and wait briefly for its ending.
fn stop(job: &Job, control: &Control) -> Option<Ending> {
    job.stop().ok()?;
    match job.observe(control, STOP_BOUND) {
        Ok(Observed::Ended(ending)) => Some(ending),
        Ok(_) | Err(_) => None,
    }
}

/// The RTK diagnostic ahead of what the command said, as the wrapper puts it.
fn prefixed(mut report: Report, diagnostic: Option<String>) -> Report {
    if let Some(diagnostic) = diagnostic {
        report.output.insert(0, '\n');
        report.output.insert_str(0, &diagnostic);
    }
    report
}

/// A finished command as its result: what it said, then how it ended.
///
/// A nonzero exit is stated in omp's own words, so the model reads the same
/// notice it would under `bash`; a signal, deadline, or cancellation is
/// stated as such.
fn finished(job: &Job, ending: &Ending) -> Report {
    let mut output = transcript(job);
    let (exit_code, notice) = notice(ending, job.timeout());
    let failed = notice.is_some();
    if let Some(notice) = notice {
        append(&mut output, &notice);
    }
    Report {
        output,
        exit_code,
        failed,
    }
}

/// A command answered while it keeps running, as omp answers its own
/// backgrounded `bash`: the preview, the notices, and the job it became.
fn kept(job: &Job, notices: &[&str]) -> Report {
    let mut lines = Vec::new();
    let preview = transcript(job);
    let preview = preview.trim_end();
    if !preview.is_empty() {
        lines.push(preview.to_owned());
        lines.push(String::new());
    }
    if !notices.is_empty() {
        lines.extend(notices.iter().map(|notice| (*notice).to_owned()));
        lines.push(String::new());
    }
    lines.push(format!(
        "Backgrounded as job {}; result will be delivered automatically.",
        job.id()
    ));
    Report {
        output: lines.join("\n"),
        exit_code: None,
        failed: false,
    }
}

/// One job as the `job` tool reads it: named, what it said, how it stands.
fn snapshot(job: &Job, ending: Option<&Ending>) -> Report {
    let mut output = format!("[job {}] {}\n", job.id(), job.command());
    output.push_str(&transcript(job));
    let (exit_code, notice) = match ending {
        Some(ending) => notice(ending, job.timeout()),
        None => (None, Some("Command still running".to_owned())),
    };
    if let Some(notice) = notice {
        append(&mut output, &notice);
    }
    Report {
        output,
        exit_code,
        failed: false,
    }
}

/// A job op that returned without the job ending.
fn still(job: &Job, why: &str) -> Report {
    let mut report = snapshot(job, None);
    append(
        &mut report.output,
        &format!("{why}; job {} keeps running", job.id()),
    );
    report
}

/// The last of the log, with earlier output noted as dropped.
fn transcript(job: &Job) -> String {
    let tail = job.tail().unwrap_or_else(|error| Tail {
        text: format!("log unreadable: {error}"),
        truncated: false,
    });
    let mut output = String::new();
    if tail.truncated {
        output.push_str("[earlier output dropped; the last 64 KiB follow]\n");
    }
    output.push_str(&tail.text);
    output
}

/// The exit code a result carries and the notice that states an ending
/// other than success.
fn notice(ending: &Ending, timeout: Duration) -> (Option<i32>, Option<String>) {
    match ending {
        Ending::Exited { code: 0 } => (Some(0), None),
        Ending::Exited { code } => (
            Some(*code),
            Some(format!("Command exited with code {code}")),
        ),
        Ending::Signalled { signal } => (None, Some(format!("Command ended by signal {signal}"))),
        Ending::TimedOut => (
            None,
            Some(format!("Command timed out after {}s", timeout.as_secs())),
        ),
        Ending::Stopped => (None, Some("Command cancelled".to_owned())),
        Ending::Failed { reason } => (None, Some(format!("Command failed to run: {reason}"))),
        Ending::Lost => (
            None,
            Some("Command lost: its keeper ended without reporting how it ended".to_owned()),
        ),
    }
}

/// A notice after the output, set off by a blank line.
fn append(output: &mut String, notice: &str) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(notice);
}

#[cfg(test)]
mod tests {
    use sill::{
        Argv, Cancellation, Deadlines, Output, ProcessGroup, Redaction, RestartDisposition,
        StdinPolicy, StreamPolicy, WaitPolicy,
    };

    use super::*;

    fn call(command: &str) -> ShellCall {
        ShellCall {
            command: command.to_owned(),
            timeout: Duration::from_secs(vendor::omp::SHELL_TIMEOUT_DEFAULT),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-terminal-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        std::fs::create_dir_all(&root).expect("scratch directory");
        root
    }

    fn terminal(root: &std::path::Path, environment: Environment) -> Terminal {
        Terminal::new("/bin/sh", root, environment, Jobs::new(root.join("jobs")))
    }

    /// A fifo nobody writes holds its reader open, so `cat hold` ends only
    /// because the lane ends it.
    fn hold(root: &std::path::Path) {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("hold"))
                .status()
                .expect("mkfifo")
                .success()
        );
    }

    fn ended(run: Run) -> Report {
        match run {
            Run::Ended(report) => report,
            Run::Backgrounded(report, job) => panic!("kept as job {}: {report:?}", job.id()),
        }
    }

    fn kept(run: Run) -> (Report, Job) {
        match run {
            Run::Backgrounded(report, job) => (report, job),
            Run::Ended(report) => panic!("ended: {report:?}"),
        }
    }

    /// Wait for a file the command touches once it has said what it says.
    fn touched(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "{} never appears",
                path.display()
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_command_reports_both_streams_and_a_failure_in_omps_own_words() {
        let root = scratch("streams");
        let terminal = terminal(&root, Environment::empty());
        let report = ended(terminal.run(
            &call("printf 'out\\n'; printf 'err\\n' >&2; exit 3"),
            &Control::new(),
        ));
        assert_eq!(
            report,
            Report {
                output: "out\nerr\n\nCommand exited with code 3".to_owned(),
                exit_code: Some(3),
                failed: true,
            }
        );

        let report = ended(terminal.run(&call("printf 'fine\\n'"), &Control::new()));
        assert_eq!(
            report,
            Report {
                output: "fine\n".to_owned(),
                exit_code: Some(0),
                failed: false,
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_command_is_ended_and_reported_as_such() {
        let root = scratch("cancelled");
        hold(&root);
        let terminal = terminal(&root, Environment::empty());
        let control = Control::new();
        control.handle().cancel();
        let report = ended(terminal.run(&call("cat hold"), &control));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            report,
            Report {
                output: "Command cancelled".to_owned(),
                exit_code: None,
                failed: true,
            }
        );
    }

    #[test]
    fn a_command_that_outruns_the_foreground_is_kept_and_its_ending_follows() {
        let root = scratch("kept");
        hold(&root);
        let terminal = terminal(&root, Environment::empty()).foreground(Duration::from_millis(50));
        let control = Control::new();
        let (report, job) = kept(terminal.run(&call("cat hold"), &control));
        assert_eq!(
            report,
            Report {
                output: format!(
                    "Backgrounded as job {}; result will be delivered automatically.",
                    job.id()
                ),
                exit_code: None,
                failed: false,
            }
        );

        let stop = JobCall {
            op: JobOp::Stop,
            id: Some(job.id().to_owned()),
            timeout: Duration::from_secs(1),
        };
        let stopped = terminal.job(&stop, &Control::new());
        assert_eq!(
            stopped.output,
            format!("[job {}] cat hold\n\nCommand cancelled", job.id())
        );
        assert!(!stopped.failed);
        assert_eq!(
            terminal.follow(&job, &control),
            Report {
                output: "Command cancelled".to_owned(),
                exit_code: None,
                failed: true,
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_message_backgrounds_the_running_command_with_omps_notice_and_the_job_tool_follows_it() {
        let root = scratch("steered");
        hold(&root);
        let terminal = terminal(&root, Environment::empty());
        let control = Control::new();
        let handle = control.handle();
        let command = "printf 'so far\\n'; touch said; cat hold";
        let running = {
            let terminal = terminal.clone();
            std::thread::spawn(move || kept(terminal.run(&call(command), &control)))
        };
        touched(&root.join("said"));
        handle.interrupt();
        let (report, job) = running.join().expect("the run returns");
        assert_eq!(
            report.output,
            format!(
                "so far\n\n{STEERED}\n\nBackgrounded as job {}; result will be delivered \
                 automatically.",
                job.id()
            )
        );

        let op = |op: JobOp| JobCall {
            op,
            id: Some(job.id().to_owned()),
            timeout: Duration::from_secs(1),
        };
        assert_eq!(
            terminal.job(&op(JobOp::Logs), &Control::new()).output,
            format!(
                "[job {}] {command}\nso far\n\nCommand still running",
                job.id()
            )
        );
        assert_eq!(
            terminal.job(&op(JobOp::List), &Control::new()).output,
            format!("{}  running  {command}\n", job.id())
        );
        let waiting = Control::new();
        waiting.handle().interrupt();
        assert_eq!(
            terminal.job(&op(JobOp::Wait), &waiting).output,
            format!(
                "[job {0}] {command}\nso far\n\nCommand still running\n\nwait interrupted by an \
                 incoming message; job {0} keeps running",
                job.id()
            )
        );
        let missing = JobCall {
            op: JobOp::Logs,
            id: Some("0badcafe".to_owned()),
            timeout: Duration::from_secs(1),
        };
        assert_eq!(
            terminal.job(&missing, &Control::new()),
            Report::refused("no job 0badcafe")
        );

        terminal.job(&op(JobOp::Stop), &Control::new());
        assert_eq!(
            terminal.job(&op(JobOp::List), &Control::new()).output,
            format!("{}  stopped  {command}\n", job.id())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_rtk_runs_the_native_command_and_says_why() {
        let root = scratch("rtk");
        let environment = Environment::new([(
            spice::rtk::EXECUTABLE_VARIABLE,
            "/nonexistent/spice-terminal-rtk",
        )])
        .expect("environment");
        let terminal = terminal(&root, environment);
        let report = ended(terminal.run(&call("printf native"), &Control::new()));
        assert_eq!(report.exit_code, Some(0));
        assert!(!report.failed);
        let (diagnostic, output) = report.output.split_once('\n').expect("diagnostic line");
        assert!(
            diagnostic.starts_with("spice run: RTK rewrite degraded to native executable="),
            "{diagnostic}"
        );
        assert_eq!(output, "native");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_terminal_is_read_from_the_occupants_own_launch_with_the_interception_lifted() {
        let environment = Environment::new([
            ("SHELL", "/hooks/entry.sh"),
            ("HOME", "/nowhere"),
            (steer::hook::BASH_ENV, "/hooks/shell/bash_env"),
            (steer::hook::ZDOTDIR, "/hooks/shell"),
            (steer::hook::HOOKS, "/hooks"),
            (steer::hook::NATIVE_SHELL, "/bin/zsh"),
            (steer::hook::INBOX, "/inbox/lane"),
            (spice::rtk::EXECUTABLE_VARIABLE, "/tools/rtk"),
            (spice::rtk::HISTORY_VARIABLE, "/history/lane.db"),
        ])
        .expect("environment");
        let spec = Spec {
            argv: Argv::new("omp", Vec::<OsString>::new()).expect("argv"),
            cwd: PathBuf::from("/work"),
            environment: environment.clone(),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(
                Duration::from_secs(1),
                Duration::from_millis(250),
                Duration::from_millis(500),
            )
            .expect("deadlines"),
            wait: WaitPolicy::new("test", Cancellation::Never, RestartDisposition::Never)
                .expect("wait"),
            output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
            redaction: Redaction::None,
            grace: Duration::from_secs(1),
        };
        let jobs = || Jobs::new("/jobs");
        let terminal =
            Terminal::from_launch(&spec, jobs()).expect("a launch with a shell has a terminal");
        assert_eq!(terminal.shell, OsString::from("/bin/zsh"));
        assert_eq!(terminal.cwd, PathBuf::from("/work"));
        assert_eq!(
            terminal.environment.entries(),
            Environment::new([
                ("HOME", "/nowhere"),
                (steer::hook::INBOX, "/inbox/lane"),
                (spice::rtk::EXECUTABLE_VARIABLE, "/tools/rtk"),
                (spice::rtk::HISTORY_VARIABLE, "/history/lane.db"),
                ("SHELL", "/bin/zsh"),
            ])
            .expect("environment")
            .entries()
        );
        assert_eq!(
            terminal.foreground,
            Duration::from_secs(vendor::omp::SHELL_FOREGROUND)
        );

        let unhooked = Spec {
            environment: Environment::new([("SHELL", "/bin/sh")]).expect("environment"),
            ..spec.clone()
        };
        let terminal =
            Terminal::from_launch(&unhooked, jobs()).expect("a plain SHELL is the terminal");
        assert_eq!(terminal.shell, OsString::from("/bin/sh"));

        let bare = Spec {
            environment: Environment::empty(),
            ..spec
        };
        assert!(Terminal::from_launch(&bare, jobs()).is_none());
    }
}

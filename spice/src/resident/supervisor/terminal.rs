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
use wire::intent::{LaneJob, LaneJobStanding};

pub(super) mod job;

pub(in crate::resident) use job::{Control, Handle, Job, Jobs};
use job::{Ended, Ending, Observed, Tail};

/// How long after a stop is asked the lane waits for the keeper's ending
/// before answering with the command still ending: the keeper's grace and
/// the reap that follows it.
const STOP_BOUND: Duration = Duration::from_secs(3);

/// omp's margin under a deadline: the foreground window stops this far
/// short of the deadline, as omp's own does.
const FOREGROUND_BUFFER: Duration = Duration::from_secs(1);

/// How long a follow waits between looks when nothing signals: the job's
/// deadline is the keeper's business, so a follow is bounded only in cadence.
const FOLLOW_CADENCE: Duration = Duration::from_secs(60);

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

    /// The foreground window for one deadline: the threshold, or the
    /// deadline less omp's buffer when that is shorter, exactly as omp
    /// bounds its own wait before backgrounding.
    fn window(&self, timeout: Option<Duration>) -> Duration {
        timeout.map_or(self.foreground, |timeout| {
            self.foreground
                .min(timeout.saturating_sub(FOREGROUND_BUFFER))
        })
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
        let mut run = self.run_command(call, control);
        let (Run::Ended(report) | Run::Backgrounded(report, _)) = &mut run;
        for advice in spice::command_advice::for_shell(&call.command, Some(&self.cwd)) {
            append(&mut report.output, advice);
        }
        run
    }

    fn run_command(&self, call: &ShellCall, control: &Control) -> Run {
        let (argv, diagnostic) = self.argv(call);
        let started = self.jobs.start(
            &call.command,
            argv,
            &self.cwd,
            &self.environment,
            call.timeout,
            &call.notices,
        );
        let job = match started {
            Ok(job) => job,
            Err(error) => {
                return Run::Ended(Report::refused(&format!("shell launch failed: {error}")));
            }
        };
        let ended = match job.observe(control, self.window(call.timeout)) {
            Ok(Observed::Ended(ended)) => ended,
            Ok(Observed::Cancelled) => stop(&job, control).unwrap_or(Ended {
                ending: Ending::Stopped,
                wall: None,
            }),
            Ok(Observed::Interrupted) => {
                return Run::Backgrounded(prefixed(kept(&job, Some(STEERED)), diagnostic), job);
            }
            Ok(Observed::Elapsed) => {
                return Run::Backgrounded(prefixed(kept(&job, None), diagnostic), job);
            }
            Err(error) => Ended {
                ending: Ending::Failed {
                    reason: format!("job observation failed: {error}"),
                },
                wall: None,
            },
        };
        let _heard = job.mark_delivered();
        Run::Ended(prefixed(finished(&job, &ended), diagnostic))
    }

    /// The jobs whose endings the lane has not heard: kept commands still
    /// running, and those that ended while nobody was there to be told.
    pub(in crate::resident) fn undelivered(&self) -> Vec<Job> {
        undelivered(&self.jobs)
    }

    /// Follow a kept command to its ending: what the delivery says, or
    /// nothing for a stopped job, since whoever stopped it was answered and
    /// omp delivers no cancelled job.
    pub(in crate::resident) fn follow(&self, job: &Job, control: &Control) -> Option<Report> {
        loop {
            match job.observe(control, FOLLOW_CADENCE) {
                Ok(Observed::Ended(Ended {
                    ending: Ending::Stopped,
                    ..
                })) => return None,
                Ok(Observed::Ended(ended)) => return Some(finished(job, &ended)),
                Ok(Observed::Cancelled | Observed::Interrupted | Observed::Elapsed) => {}
                Err(error) => {
                    return Some(Report::refused(&format!(
                        "job {} could not be followed: {error}",
                        job.id()
                    )));
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
            (JobOp::Logs, Some(job)) => snapshot(&job, job.ended().as_ref()),
            (JobOp::Wait, Some(job)) => match job.observe(control, call.timeout) {
                Ok(Observed::Ended(ended)) => snapshot(&job, Some(&ended)),
                Ok(Observed::Elapsed) => snapshot(&job, None),
                Ok(Observed::Interrupted) => still(&job, "wait interrupted by an incoming message"),
                Ok(Observed::Cancelled) => still(&job, "wait cancelled"),
                Err(error) => {
                    Report::refused(&format!("job {} could not be observed: {error}", job.id()))
                }
            },
            (JobOp::Stop, Some(job)) => match stop(&job, control) {
                Some(ended) => snapshot(&job, Some(&ended)),
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
            let status = status(job.ended().as_ref().map(|ended| &ended.ending));
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

/// The jobs a lane has yet to hear the end of, oldest first: still running,
/// or ended while nobody was there.
pub(in crate::resident) fn undelivered(jobs: &Jobs) -> Vec<Job> {
    jobs.kept().unwrap_or_default()
}

/// What a lane is still waiting on, as its row in the lane table lists it.
pub(in crate::resident) fn unheard(jobs: &Jobs) -> Vec<LaneJob> {
    undelivered(jobs)
        .into_iter()
        .map(|job| LaneJob {
            id: job.id().to_owned(),
            command: job.command().to_owned(),
            standing: if job.ended().is_some() {
                LaneJobStanding::Unheard
            } else {
                LaneJobStanding::Running
            },
        })
        .collect()
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
fn stop(job: &Job, control: &Control) -> Option<Ended> {
    job.stop().ok()?;
    match job.observe(control, STOP_BOUND) {
        Ok(Observed::Ended(ended)) => Some(ended),
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

/// A finished command as its result, laid out as omp lays out its own: what
/// it said, the wall time and notices, then how it ended when that was not
/// a clean exit. A nonzero exit is a completed command that failed; a
/// deadline, a stop, a signal, a launch failure, and a lost keeper are each
/// stated as such, and every one of them is an error to the model.
fn finished(job: &Job, ended: &Ended) -> Report {
    let said = transcript(job);
    let said = said.trim_end();
    if ended.ending == Ending::Stopped {
        let output = if said.is_empty() {
            "Command aborted".to_owned()
        } else {
            format!("{said}\n\n[Command aborted]")
        };
        return Report {
            output,
            exit_code: None,
            failed: true,
        };
    }
    let mut lines = vec![if said.is_empty() { "(no output)" } else { said }.to_owned()];
    let wall = ended
        .wall
        .map(|wall| format!("Wall time: {:.2} seconds", wall.as_secs_f64()));
    let notices: Vec<String> = wall
        .into_iter()
        .chain(job.notices().iter().cloned())
        .collect();
    if !notices.is_empty() {
        lines.push(String::new());
        lines.extend(notices);
    }
    let (exit_code, footer) = footer(&ended.ending, job.timeout());
    let failed = footer.is_some();
    if let Some(footer) = footer {
        lines.push(String::new());
        lines.push(footer);
    }
    Report {
        output: lines.join("\n"),
        exit_code,
        failed,
    }
}

/// A command answered while it keeps running, as omp answers its own
/// backgrounded `bash`: the preview, the notices, and the job it became.
fn kept(job: &Job, notice: Option<&str>) -> Report {
    let mut lines = Vec::new();
    let preview = transcript(job);
    let preview = preview.trim_end();
    if !preview.is_empty() {
        lines.push(preview.to_owned());
        lines.push(String::new());
    }
    let notices: Vec<&str> = job
        .notices()
        .iter()
        .map(String::as_str)
        .chain(notice)
        .collect();
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

/// One job as the `job` tool reads it: named, then its result so far.
fn snapshot(job: &Job, ended: Option<&Ended>) -> Report {
    let mut output = format!("[job {}] {}\n", job.id(), job.command());
    let exit_code = match ended {
        Some(ended) => {
            let finished = finished(job, ended);
            output.push_str(&finished.output);
            finished.exit_code
        }
        None => {
            let mut said = transcript(job);
            append(&mut said, "Command still running");
            output.push_str(&said);
            None
        }
    };
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

/// A bounded log preview; omitted bytes remain readable in the keeper's log.
fn transcript(job: &Job) -> String {
    let tail = job.tail().unwrap_or_else(|error| Tail {
        text: format!("log unreadable: {error}"),
        truncated: false,
    });
    let mut output = String::new();
    if tail.truncated {
        output.push_str(&format!(
            "[Showing the last {} KiB; full output: {}]\n",
            job::LOG_TAIL / 1024,
            job.log_path().display()
        ));
    }
    output.push_str(&tail.text);
    output
}

/// The exit code a result carries and the footer that states an ending
/// other than a clean exit, in omp's own words where it has them.
fn footer(ending: &Ending, timeout: Option<Duration>) -> (Option<i32>, Option<String>) {
    match ending {
        Ending::Exited { code: 0 } => (Some(0), None),
        Ending::Exited { code } => (
            Some(*code),
            Some(format!("Command exited with code {code}")),
        ),
        Ending::Signalled { signal } => (None, Some(format!("Command ended by signal {signal}"))),
        Ending::TimedOut => (
            None,
            Some(match timeout {
                Some(timeout) => format!("[Command timed out after {} seconds]", timeout.as_secs()),
                None => "[Command timed out]".to_owned(),
            }),
        ),
        Ending::Stopped => (None, Some("[Command aborted]".to_owned())),
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
    use super::*;

    /// How long a test waits for a command to answer before it fails.
    const TEST_BOUND: Duration = Duration::from_secs(2);

    fn call(command: &str) -> ShellCall {
        ShellCall {
            command: command.to_owned(),
            timeout: Some(Duration::from_secs(vendor::omp::SHELL_TIMEOUT_DEFAULT)),
            notices: Vec::new(),
        }
    }

    /// The output with its wall time, which no two runs share, made one.
    fn timeless(output: &str) -> String {
        let Some((head, tail)) = output.split_once("Wall time: ") else {
            return output.to_owned();
        };
        let (_, rest) = tail
            .split_once(" seconds")
            .expect("a wall time ends in seconds");
        format!("{head}Wall time: N seconds{rest}")
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
        let directory = path.parent().expect("the file has a directory");
        loop {
            let watch = crate::resident::daemon::observer::DirectObserver::arm(directory)
                .expect("the directory is watched");
            if path.exists() {
                return;
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || drop(sender.send(watch.wait())));
            receiver
                .recv_timeout(TEST_BOUND)
                .unwrap_or_else(|_| panic!("{} never appears", path.display()))
                .expect("the directory is observed");
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
            timeless(&report.output),
            "out\nerr\n\nWall time: N seconds\n\nCommand exited with code 3"
        );
        assert_eq!((report.exit_code, report.failed), (Some(3), true));

        let mut noted = call("printf 'fine\\n'");
        noted
            .notices
            .push("Timeout clamped to 3600s (requested 9999s; allowed range 1-3600s).".to_owned());
        let report = ended(terminal.run(&noted, &Control::new()));
        assert_eq!(
            timeless(&report.output),
            "fine\n\nWall time: N seconds\nTimeout clamped to 3600s (requested 9999s; allowed \
             range 1-3600s)."
        );
        assert_eq!((report.exit_code, report.failed), (Some(0), false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_foreground_window_is_omps_threshold_bounded_by_the_deadline_less_its_buffer() {
        let root = scratch("window");
        let terminal = terminal(&root, Environment::empty());
        assert_eq!(terminal.window(None), Duration::from_secs(60));
        assert_eq!(
            terminal.window(Some(Duration::from_secs(30))),
            Duration::from_secs(29)
        );
        assert_eq!(
            terminal.window(Some(Duration::from_secs(300))),
            Duration::from_secs(60)
        );
        assert_eq!(
            terminal.window(Some(Duration::from_secs(1))),
            Duration::ZERO
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_deadline_shorter_than_the_buffer_opens_no_window_and_its_expiry_is_reported() {
        let root = scratch("deadline");
        hold(&root);
        let terminal = terminal(&root, Environment::empty());
        let control = Control::new();
        let mut brief = call("cat hold");
        brief.timeout = Some(Duration::from_millis(20));
        // The window is empty, so the command is kept at once unless its
        // deadline has already passed by the first look; either way the
        // expiry is reported in omp's words.
        let delivered = match terminal.run(&brief, &control) {
            Run::Backgrounded(report, job) => {
                assert_eq!(
                    report.output,
                    format!(
                        "Backgrounded as job {}; result will be delivered automatically.",
                        job.id()
                    )
                );
                terminal
                    .follow(&job, &control)
                    .expect("an expiry is delivered")
            }
            Run::Ended(report) => report,
        };
        assert_eq!(
            timeless(&delivered.output),
            "(no output)\n\nWall time: N seconds\n\n[Command timed out after 0 seconds]"
        );
        assert_eq!((delivered.exit_code, delivered.failed), (None, true));
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
                output: "Command aborted".to_owned(),
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
            format!("[job {}] cat hold\nCommand aborted", job.id())
        );
        assert!(!stopped.failed);
        assert_eq!(terminal.follow(&job, &control), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_lane_table_lists_what_the_lane_has_yet_to_hear() {
        let root = scratch("unheard");
        hold(&root);
        let terminal = terminal(&root, Environment::empty()).foreground(Duration::from_millis(50));
        let control = Control::new();
        let (_, job) = kept(terminal.run(&call("cat hold"), &control));
        let row = |standing| {
            vec![LaneJob {
                id: job.id().to_owned(),
                command: "cat hold".to_owned(),
                standing,
            }]
        };
        assert_eq!(unheard(&terminal.jobs), row(LaneJobStanding::Running));
        std::fs::write(root.join("hold"), "\n").expect("the fifo takes a line");
        assert!(terminal.follow(&job, &control).is_some());
        assert_eq!(unheard(&terminal.jobs), row(LaneJobStanding::Unheard));
        job.mark_delivered().expect("the ending is heard");
        assert!(unheard(&terminal.jobs).is_empty());
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
        assert_eq!(timeless(output), "native\n\nWall time: N seconds");
        let _ = std::fs::remove_dir_all(&root);
    }
}

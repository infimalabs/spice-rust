use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Instant;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

pub(super) fn required_read(root: &Path, args: &[&str], action: &str) -> Result<String, Error> {
    let output = run(root, args)?;
    if output.success() {
        Ok(output.stdout.trim().to_owned())
    } else {
        Err(Error::fault("git::command-failed", output.detail(action)))
    }
}

pub(super) fn try_read(root: &Path, args: &[&str]) -> Result<Option<String>, Error> {
    let output = run(root, args)?;
    Ok(output.success().then(|| output.stdout.trim().to_owned()))
}

pub(super) fn run(root: &Path, args: &[&str]) -> Result<GitOutput, Error> {
    run_bounded(root, args, None, &[])
}

pub(super) fn run_with_input(root: &Path, args: &[&str], input: &[u8]) -> Result<GitOutput, Error> {
    run_bounded(root, args, Some(input), &[])
}

pub(super) fn run_with_environment(
    root: &Path,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<GitOutput, Error> {
    run_bounded(root, args, None, environment)
}

fn run_bounded(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    environment: &[(&str, &str)],
) -> Result<GitOutput, Error> {
    run_bounded_observed(root, args, input, environment, None)
}

fn run_bounded_observed(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    environment: &[(&str, &str)],
    descendant_grace_started: Option<&mut dyn FnMut()>,
) -> Result<GitOutput, Error> {
    let limit = command_limit(args);
    let mut process = spawn_git(root, args, input.is_some(), environment)?;
    let workers = match CommandWorkers::start(&mut process, input) {
        Ok(workers) => workers,
        Err(error) => {
            process.terminate()?;
            return Err(error);
        }
    };
    let (status, stdout, stderr) =
        supervise_git(&mut process, workers, args, limit, descendant_grace_started)?;
    Ok(GitOutput {
        status: status.code().unwrap_or(-1),
        stdout: decode_output(stdout, "stdout")?,
        stderr: decode_output(stderr, "stderr")?,
    })
}

fn spawn_git(
    root: &Path,
    args: &[&str],
    piped_input: bool,
    environment: &[(&str, &str)],
) -> Result<GitProcess, Error> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=5",
        )
        .envs(environment.iter().copied())
        .stdin(if piped_input {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    command
        .spawn()
        .map(GitProcess::new)
        .map_err(|error| Error::fault("git::launch-failed", error.to_string()))
}

fn command_limit(args: &[&str]) -> Duration {
    if args
        .first()
        .is_some_and(|name| matches!(*name, "fetch" | "push"))
    {
        NETWORK_COMMAND_LIMIT
    } else {
        LOCAL_COMMAND_LIMIT
    }
}

pub(super) fn supervise_git(
    process: &mut GitProcess,
    mut workers: CommandWorkers,
    args: &[&str],
    limit: Duration,
    mut descendant_grace_started: Option<&mut dyn FnMut()>,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), Error> {
    let runtime_started = Instant::now();
    let mut output_drain_started = None;
    let mut status = None;
    loop {
        if status.is_none() {
            match poll_supervised_exit(process, args, &mut descendant_grace_started) {
                Ok(Some((exit, drain_started))) => {
                    status = Some(exit);
                    output_drain_started = Some(drain_started);
                }
                Ok(None) => {}
                Err(error) => {
                    return fail_supervision(process, workers, error);
                }
            }
        }

        while let Ok(event) = workers.events.try_recv() {
            if let Err(error) = workers.accept(event, args) {
                return fail_supervision(process, workers, error);
            }
        }
        if let Some(status) = status
            && workers.complete()
        {
            workers.join()?;
            return Ok((
                status,
                workers.stdout.take().expect("complete workers have stdout"),
                workers.stderr.take().expect("complete workers have stderr"),
            ));
        }

        if let Some(error) = supervision_deadline_error(
            args,
            status.is_none(),
            runtime_started,
            output_drain_started,
            limit,
        ) {
            return fail_supervision(process, workers, error);
        }

        let runtime_remaining = limit.saturating_sub(runtime_started.elapsed());
        let drain_remaining = output_drain_started.map_or(runtime_remaining, |started| {
            OUTPUT_DRAIN_LIMIT.saturating_sub(started.elapsed())
        });
        let wait = runtime_remaining
            .min(drain_remaining)
            .min(Duration::from_millis(10));
        match workers.events.recv_timeout(wait) {
            Ok(event) => {
                if let Err(error) = workers.accept(event, args) {
                    return fail_supervision(process, workers, error);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) if !workers.complete() => {
                let error = Error::fault(
                    "git::worker-lost",
                    format!("git {} lost an I/O worker", args.join(" ")),
                );
                return fail_supervision(process, workers, error);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(wait);
            }
        }
    }
}

fn supervision_deadline_error(
    args: &[&str],
    running: bool,
    runtime_started: Instant,
    output_drain_started: Option<Instant>,
    limit: Duration,
) -> Option<Error> {
    if running && runtime_started.elapsed() >= limit {
        return Some(Error::fault(
            "git::command-timeout",
            format!(
                "git {} exceeded {} seconds",
                args.join(" "),
                limit.as_secs(),
            ),
        ));
    }
    output_drain_started
        .is_some_and(|started| started.elapsed() >= OUTPUT_DRAIN_LIMIT)
        .then(|| {
            Error::fault(
                "git::output-drain-timeout",
                format!(
                    "git {} output did not close within {} milliseconds",
                    args.join(" "),
                    OUTPUT_DRAIN_LIMIT.as_millis(),
                ),
            )
        })
}

fn poll_supervised_exit(
    process: &mut GitProcess,
    args: &[&str],
    descendant_grace_started: &mut Option<&mut dyn FnMut()>,
) -> Result<Option<(std::process::ExitStatus, Instant)>, Error> {
    let Some(status) = process.poll_exit()? else {
        return Ok(None);
    };
    let output_drain_started = Instant::now();
    // A descendant merely on its way out is not a survivor. Git has exited,
    // so nothing can join its group and only an observed group keeps this
    // post-exit grace open.
    if let Some(observer) = descendant_grace_started.as_mut() {
        observer();
    }
    if process.settle(DESCENDANT_EXIT_GRACE, GroupProbeSite::GitExited)? {
        return Ok(Some((status, output_drain_started)));
    }
    Err(Error::fault(
        "git::descendant-survived",
        format!(
            "git {} exited and its process group still had live descendants {} milliseconds later",
            args.join(" "),
            DESCENDANT_EXIT_GRACE.as_millis(),
        ),
    ))
}

fn fail_supervision<T>(
    process: &mut GitProcess,
    mut workers: CommandWorkers,
    error: Error,
) -> Result<T, Error> {
    let cleanup = process.terminate();
    let joined = workers.join();
    cleanup?;
    joined?;
    Err(error)
}

pub(super) struct GitProcess {
    child: std::process::Child,
    pid: u32,
    status: Option<std::process::ExitStatus>,
    #[cfg(unix)]
    group_probe: Box<dyn FnMut(GroupProbeSite, rustix::process::Pid) -> GroupPresence>,
}

/// What the kernel actually established about Git's process group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GroupPresence {
    Present,
    Absent,
    #[cfg_attr(
        windows,
        expect(
            dead_code,
            reason = "Windows process probes are always determinate but share the evidence rule"
        )
    )]
    Indeterminate,
}

/// The evidence rule at one of the three process-group observation sites.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GroupProbeSite {
    GitExited,
    TermSentBeforeKill,
    KillSent,
}

impl GitProcess {
    fn new(child: std::process::Child) -> Self {
        let pid = child.id();
        Self {
            child,
            pid,
            status: None,
            #[cfg(unix)]
            group_probe: Box::new(|_, group| {
                classify_group_probe(rustix::process::test_kill_process_group(group))
            }),
        }
    }

    fn poll_exit(&mut self) -> Result<Option<std::process::ExitStatus>, Error> {
        if self.status.is_none() {
            self.status = self
                .child
                .try_wait()
                .map_err(|error| Error::fault("git::wait-failed", error.to_string()))?;
        }
        Ok(self.status)
    }

    pub(super) fn terminate(&mut self) -> Result<(), Error> {
        self.signal_group(false)?;
        // After TERM but before KILL, only observed absence may suppress
        // escalation. Each unreadable observation is re-read once immediately;
        // continued uncertainty spends the remaining grace and proceeds to
        // KILL rather than claiming cleanup already finished.
        if self.settle(PROCESS_GROUP_GRACE, GroupProbeSite::TermSentBeforeKill)? {
            return Ok(());
        }
        self.signal_group(true)?;
        // KILL has already been sent, so an unreadable answer is not evidence
        // of a survivor. Only a group positively observed here can hold the
        // post-KILL reap open until its deadline.
        if self.settle(PROCESS_GROUP_REAP_LIMIT, GroupProbeSite::KillSent)? {
            Ok(())
        } else {
            Err(Error::fault(
                "git::process-group-reap-timeout",
                format!(
                    "Git process group {} remained after {} milliseconds",
                    self.pid,
                    PROCESS_GROUP_REAP_LIMIT.as_millis(),
                ),
            ))
        }
    }

    fn settle(&mut self, limit: Duration, probe_site: GroupProbeSite) -> Result<bool, Error> {
        let started = Instant::now();
        loop {
            let exited = self.poll_exit()?.is_some();
            if exited && group_is_settled_against(probe_site, || self.group_present(probe_site))? {
                return Ok(true);
            }
            if started.elapsed() >= limit {
                return Ok(false);
            }
            std::thread::sleep(
                limit
                    .saturating_sub(started.elapsed())
                    .min(Duration::from_millis(5)),
            );
        }
    }

    #[cfg(unix)]
    fn signal_group(&mut self, kill: bool) -> Result<(), Error> {
        let signal = if kill {
            rustix::process::Signal::KILL
        } else {
            rustix::process::Signal::TERM
        };
        match rustix::process::kill_process_group(self.process_group()?, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(Error::fault(
                "git::process-group-signal-failed",
                error.to_string(),
            )),
        }
    }

    #[cfg(not(unix))]
    fn signal_group(&mut self, _kill: bool) -> Result<(), Error> {
        self.child
            .kill()
            .or_else(|error| {
                (error.kind() == std::io::ErrorKind::InvalidInput)
                    .then_some(())
                    .ok_or(error)
            })
            .map_err(|error| Error::fault("git::process-kill-failed", error.to_string()))
    }

    #[cfg(unix)]
    fn group_present(&mut self, probe_site: GroupProbeSite) -> Result<GroupPresence, Error> {
        let group = self.process_group()?;
        Ok((self.group_probe)(probe_site, group))
    }

    #[cfg(unix)]
    fn process_group(&self) -> Result<rustix::process::Pid, Error> {
        let pid = i32::try_from(self.pid)
            .map_err(|error| Error::fault("git::process-group-invalid", error.to_string()))?;
        rustix::process::Pid::from_raw(pid).ok_or_else(|| {
            Error::fault("git::process-group-invalid", "Git returned process ID zero")
        })
    }

    #[cfg(not(unix))]
    fn group_present(&mut self, _probe_site: GroupProbeSite) -> Result<GroupPresence, Error> {
        Ok(if self.status.is_none() {
            GroupPresence::Present
        } else {
            GroupPresence::Absent
        })
    }
}

/// Apply the evidence rule for a process-group probe at one consumer site.
///
/// The pre-KILL consumer cannot let an unreadable answer skip escalation, but
/// one immediate re-read lets transient unreadability expose an already-absent
/// group without spending the whole grace. The two post-exit consumers have
/// already observed Git's exit or delivered KILL, so only positive presence is
/// evidence that their settling work remains unfinished.
pub(super) fn group_is_settled_against(
    probe_site: GroupProbeSite,
    mut observe: impl FnMut() -> Result<GroupPresence, Error>,
) -> Result<bool, Error> {
    let presence = observe()?;
    match probe_site {
        GroupProbeSite::TermSentBeforeKill => match presence {
            GroupPresence::Absent => Ok(true),
            GroupPresence::Present => Ok(false),
            GroupPresence::Indeterminate => Ok(matches!(observe()?, GroupPresence::Absent)),
        },
        GroupProbeSite::GitExited | GroupProbeSite::KillSent => {
            Ok(!matches!(presence, GroupPresence::Present))
        }
    }
}

#[cfg(unix)]
pub(super) fn classify_group_probe(result: Result<(), rustix::io::Errno>) -> GroupPresence {
    match result {
        Ok(()) | Err(rustix::io::Errno::PERM) => GroupPresence::Present,
        Err(rustix::io::Errno::SRCH) => GroupPresence::Absent,
        Err(_) => GroupPresence::Indeterminate,
    }
}

enum WorkerEvent {
    Stdout(Result<Vec<u8>, OutputReadError>),
    Stderr(Result<Vec<u8>, OutputReadError>),
    Stdin(Result<(), std::io::Error>),
    Panicked(&'static str),
}

pub(super) struct CommandWorkers {
    events: mpsc::Receiver<WorkerEvent>,
    handles: Vec<JoinHandle<()>>,
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    stdin_complete: bool,
}

impl CommandWorkers {
    pub(super) fn start(process: &mut GitProcess, input: Option<&[u8]>) -> Result<Self, Error> {
        let stdout = process.child.stdout.take().ok_or_else(|| {
            Error::fault("git::stdout-missing", "Git stdout pipe was unavailable")
        })?;
        let stderr = process.child.stderr.take().ok_or_else(|| {
            Error::fault("git::stderr-missing", "Git stderr pipe was unavailable")
        })?;
        let stdin = input
            .map(|bytes| {
                process
                    .child
                    .stdin
                    .take()
                    .map(|stdin| (stdin, bytes.to_vec()))
                    .ok_or_else(|| {
                        Error::fault("git::stdin-missing", "Git stdin pipe was unavailable")
                    })
            })
            .transpose()?;
        let (sender, events) = mpsc::channel();
        let total = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![
            spawn_reader(stdout, Arc::clone(&total), sender.clone(), true),
            spawn_reader(stderr, total, sender.clone(), false),
        ];
        if let Some((stdin, bytes)) = stdin {
            handles.push(spawn_writer(stdin, bytes, sender.clone()));
        }
        drop(sender);
        Ok(Self {
            events,
            handles,
            stdout: None,
            stderr: None,
            stdin_complete: input.is_none(),
        })
    }

    fn accept(&mut self, event: WorkerEvent, args: &[&str]) -> Result<(), Error> {
        match event {
            WorkerEvent::Stdout(result) => {
                self.stdout = Some(output_result(result, "stdout", args)?);
            }
            WorkerEvent::Stderr(result) => {
                self.stderr = Some(output_result(result, "stderr", args)?);
            }
            WorkerEvent::Stdin(Ok(())) => self.stdin_complete = true,
            WorkerEvent::Stdin(Err(error)) => {
                return Err(Error::fault("git::stdin-failed", error.to_string()));
            }
            WorkerEvent::Panicked(name) => {
                return Err(Error::fault(
                    "git::io-worker-panicked",
                    format!("Git {name} worker panicked"),
                ));
            }
        }
        Ok(())
    }

    fn complete(&self) -> bool {
        self.stdout.is_some() && self.stderr.is_some() && self.stdin_complete
    }

    fn join(&mut self) -> Result<(), Error> {
        let mut panicked = false;
        for handle in self.handles.drain(..) {
            panicked |= handle.join().is_err();
        }
        if panicked {
            Err(Error::fault(
                "git::io-worker-panicked",
                "Git I/O worker panicked",
            ))
        } else {
            Ok(())
        }
    }
}

fn spawn_reader<R>(
    input: R,
    total: Arc<AtomicUsize>,
    sender: mpsc::Sender<WorkerEvent>,
    stdout: bool,
) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_bounded(input, &total)));
        let event = match result {
            Ok(result) if stdout => WorkerEvent::Stdout(result),
            Ok(result) => WorkerEvent::Stderr(result),
            Err(_) if stdout => WorkerEvent::Panicked("stdout"),
            Err(_) => WorkerEvent::Panicked("stderr"),
        };
        let _sent = sender.send(event);
    })
}

fn spawn_writer(
    mut stdin: std::process::ChildStdin,
    input: Vec<u8>,
    sender: mpsc::Sender<WorkerEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stdin.write_all(&input)));
        let event = match result {
            Ok(result) => WorkerEvent::Stdin(result),
            Err(_) => WorkerEvent::Panicked("stdin"),
        };
        let _sent = sender.send(event);
    })
}

enum OutputReadError {
    Limit,
    Read(std::io::Error),
}

fn read_bounded(
    mut input: impl std::io::Read,
    total: &AtomicUsize,
) -> Result<Vec<u8>, OutputReadError> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(OutputReadError::Read)?;
        if count == 0 {
            return Ok(output);
        }
        let previous = total.fetch_add(count, Ordering::AcqRel);
        if previous.saturating_add(count) > OUTPUT_LIMIT {
            return Err(OutputReadError::Limit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn output_result(
    result: Result<Vec<u8>, OutputReadError>,
    name: &str,
    args: &[&str],
) -> Result<Vec<u8>, Error> {
    match result {
        Ok(bytes) => Ok(bytes),
        Err(OutputReadError::Limit) => Err(Error::fault(
            "git::output-limit",
            format!("git {} exceeded {} bytes", args.join(" "), OUTPUT_LIMIT,),
        )),
        Err(OutputReadError::Read(error)) => Err(Error::fault(
            "git::output-read-failed",
            format!("Git {name}: {error}"),
        )),
    }
}

fn decode_output(bytes: Vec<u8>, name: &str) -> Result<String, Error> {
    String::from_utf8(bytes)
        .map_err(|error| Error::fault("git::output-not-utf8", format!("Git {name}: {error}")))
}

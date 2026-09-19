//! Driving a vendor over its stdin: the opening request, interjected
//! steering, and the close that ends its one turn.
//!
//! A vendor launched over a command stream reads nothing from its arguments
//! after start. The supervisor owns the pipe: it writes the opening request
//! the vendor contract composed, interjects this lane's rendered steering
//! inbox whenever the resident republishes it, answers the dialogs the agent
//! opens, and drops the pipe once the agent's turn completes so the child
//! drains and exits on its own. A drain interrupts the turn over the same
//! pipe before any signal is sent. A confirm dialog becomes one consent request
//! the resident decides; a call on the lane's shell runs in the lane's own
//! terminal and answers with what the command said; every other dialog is
//! declined, since nobody at the lane can compose a choice or text for it.
//! Steering never waits on a command: direction the lane has not heard,
//! arriving while a command runs, interrupts the call, which answers with
//! what the command said so far while the command keeps running as a job,
//! and the job's ending is delivered as steering of its own when it comes;
//! an ending nobody was there to hear waits for the lane's next occupant.
//! The resident reaches the driver through one datagram kick per inbox
//! publication or consent answer; a lane with no command-driven occupant has
//! nobody listening, and the kick is lost by design.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use listen::{Decoded, Dialog, Event, TurnMoment};
use serde::{Deserialize, Serialize};
use vendor::omp::{JobCall, ShellCall};
use wire::CanonicalUuid;

use super::super::RuntimePaths;
use super::terminal::{Control, Handle, Job, Report, Run, Terminal};

/// Bytes one kick datagram may carry, including an occupant-scoped narration prod.
const KICK_BYTES: usize = 256;

/// One child's command stream and what has already been said on it.
pub(in crate::resident) struct Driver<W: Write + Send = std::process::ChildStdin> {
    stdin: Mutex<Option<W>>,
    inbox: PathBuf,
    sequence: AtomicU64,
    delivered: Mutex<Option<Vec<u8>>>,
    /// The confirm dialog each open consent request will answer.
    asks: Mutex<HashMap<CanonicalUuid, String>>,
    /// Where the occupant's commands run, for an occupant that has a shell.
    terminal: Option<Terminal>,
    /// The calls being answered now, by the host tool call each answers.
    calls: Mutex<HashMap<String, Handle>>,
}

/// What one host tool call asks of the lane.
enum Work {
    Shell(ShellCall),
    Job(JobCall),
}

/// A confirm dialog stated as the consent request the resident decides.
pub(in crate::resident) struct Ask {
    pub(in crate::resident) request: CanonicalUuid,
    pub(in crate::resident) operation: String,
}

/// What one datagram at the lane's kick door says.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::resident) enum Kick {
    /// The lane's steering inbox was republished.
    Inbox,
    /// Consent ruled on a request the occupant raised.
    Answer {
        request: CanonicalUuid,
        confirmed: bool,
    },
    /// A committed progress reminder for this exact occupant.
    Narration(wire::body::NarrationProdded),
    /// The lane is being drained: interrupt the turn and end the input.
    Abort,
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Own a child's stdin, the inbox path this lane's steering renders to,
    /// and the terminal its commands run in.
    pub(in crate::resident) fn new(
        stdin: W,
        inbox: impl Into<PathBuf>,
        terminal: Option<Terminal>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            inbox: inbox.into(),
            sequence: AtomicU64::new(0),
            delivered: Mutex::new(None),
            asks: Mutex::new(HashMap::new()),
            terminal,
            calls: Mutex::new(HashMap::new()),
        })
    }

    /// Write the opening request the vendor contract composed.
    ///
    /// # Errors
    ///
    /// Returns the pipe's error, or `BrokenPipe` once the stream is closed.
    pub(in crate::resident) fn open(&self, opening: &[u8]) -> io::Result<()> {
        self.write(opening)
    }

    /// Interject the rendered inbox unless it is empty, missing, or exactly
    /// what the agent already received. Every call being answered is
    /// interrupted first, so the message is read as soon as it is written
    /// rather than once the command ends.
    pub(in crate::resident) fn deliver_inbox(&self) {
        self.interject(true);
    }

    fn deliver_narration(&self, prod: &wire::body::NarrationProdded) {
        for handle in self
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            handle.interrupt();
        }
        let urgency = if prod.count > 1 {
            "Progress update overdue"
        } else {
            "Progress update due"
        };
        let message = format!(
            "<system-notice>\n{urgency}: {} tool calls since your last visible prose \
             (reminder {}). Give the operator one short status line now, including \
             any job you are awaiting. Keep healthy work running; do not cancel it \
             to answer this reminder. No acknowledgment key is required.\n</system-notice>",
            prod.moments, prod.count,
        );
        let _sent = self.steer(&message);
    }

    /// Interject the rendering the inbox holds now, once per rendering.
    ///
    /// Direction the lane has not heard interrupts the calls in flight, so the
    /// agent sees it as early as it can; a rendering that only restates what
    /// the lane heard, a reminder or a re-render, waits for the turn's next
    /// boundary like any steering. After a compaction nothing is news.
    fn interject(&self, news: bool) {
        let Ok(rendered) = fs::read(&self.inbox) else {
            return;
        };
        if rendered.is_empty() {
            return;
        }
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if delivered.as_deref() == Some(rendered.as_slice()) {
            return;
        }
        let Ok(message) = std::str::from_utf8(&rendered) else {
            return;
        };
        let heard = delivered
            .as_deref()
            .and_then(|heard| std::str::from_utf8(heard).ok())
            .unwrap_or_default();
        let unheard =
            steer::inbox::keys(message).any(|key| !steer::inbox::keys(heard).any(|h| h == key));
        if news && unheard {
            for handle in self
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
            {
                handle.interrupt();
            }
        }
        if self.steer(message).is_ok() {
            *delivered = Some(rendered);
        }
    }

    /// Interject one message as steering, in sequence with the inbox.
    fn steer(&self, message: &str) -> io::Result<()> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.write(&vendor::omp::steer_request(sequence, message))
    }

    /// Let one decoded record shape delivery, and state the consent request a
    /// confirm dialog raises.
    ///
    /// A confirm holds the turn until consent answers it through
    /// [`Self::answer`]; a call on the lane's shell holds it until the
    /// command ends; any other dialog is declined at once, since no operator
    /// sees a lane's prompts and the turn would otherwise hold. A withdrawn
    /// call ends its command. A compaction discards what the agent was told,
    /// so the standing inbox is interjected again; the end of the agent's
    /// turn ends its input, so the child drains and exits rather than idling
    /// for a prompt nobody sends.
    pub(in crate::resident) fn observe(self: &Arc<Self>, decoded: &Decoded) -> Option<Ask> {
        let context = decoded.context();
        let asked = match context.and_then(|context| context.dialog.as_ref()) {
            Some(Dialog::Confirm { id, title, message }) => Some(self.ask(id, title, message)),
            Some(Dialog::HostTool {
                id,
                tool,
                arguments,
            }) => {
                self.run(id, tool, arguments);
                None
            }
            Some(dialog) => {
                let _declined = self.write(&vendor::omp::dialog_declined(dialog.id()));
                None
            }
            None => None,
        };
        if let Some(withdrawn) = context.and_then(|context| context.withdrawn.as_deref()) {
            self.cancel(withdrawn);
        }
        let compacted = decoded
            .events()
            .iter()
            .any(|event| matches!(event, Event::Compaction(_)));
        if compacted {
            self.delivered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            self.interject(false);
        }
        let completed = decoded
            .context()
            .and_then(|context| context.turn)
            .is_some_and(|turn| turn == TurnMoment::Completed);
        if completed {
            self.close();
        }
        asked
    }

    /// Answer the confirm dialog one consent request stands for.
    ///
    /// A request this driver never raised, or one it already answered, is
    /// nobody's dialog here and says nothing.
    pub(in crate::resident) fn answer(&self, request: CanonicalUuid, confirmed: bool) {
        let dialog = self
            .asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request);
        if let Some(dialog) = dialog {
            let _answered = self.write(&vendor::omp::dialog_confirmed(&dialog, confirmed));
        }
    }

    /// Answer one host tool call: a command run in the lane's terminal, or a
    /// request on the lane's jobs.
    ///
    /// The work runs on its own thread, so the stream the call arrived on
    /// keeps being read: the withdrawal that ends the call early and the
    /// message that interrupts it arrive there. A tool the lane never
    /// registered, arguments that name no command, or an occupant with no
    /// terminal are refused at once.
    fn run(
        self: &Arc<Self>,
        id: &str,
        tool: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) {
        let work = match tool {
            vendor::omp::SHELL_TOOL => ShellCall::parse(arguments).map(Work::Shell),
            vendor::omp::JOB_TOOL => JobCall::parse(arguments).map(Work::Job),
            _ => Err(format!("{tool} is not a tool this lane registered")),
        };
        let (terminal, work) = match (self.terminal.clone(), work) {
            (Some(terminal), Ok(work)) => (terminal, work),
            (_, work) => {
                let reason = work
                    .err()
                    .unwrap_or_else(|| "this lane has no terminal".to_owned());
                let _refused = self.write(&vendor::omp::host_tool_refused(id, &reason));
                return;
            }
        };
        let control = Control::new();
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), control.handle());
        let driver = Arc::clone(self);
        let answered = id.to_owned();
        let started = std::thread::Builder::new()
            .name("spice-shell".to_owned())
            .spawn(move || driver.resolve(&terminal, &answered, &work, &control));
        if let Err(error) = started {
            self.cancel(id);
            let _refused = self.write(&vendor::omp::host_tool_refused(
                id,
                &format!("shell thread failed: {error}"),
            ));
        }
    }

    /// Answer one call with what its work said, then deliver the ending of a
    /// command the answer left running.
    fn resolve(&self, terminal: &Terminal, id: &str, work: &Work, control: &Control) {
        let (report, kept) = match work {
            Work::Shell(call) => match terminal.run(call, control) {
                Run::Ended(report) => (report, None),
                Run::Backgrounded(report, job) => (report, Some(job)),
            },
            Work::Job(call) => (terminal.job(call, control), None),
        };
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        let _answered = self.write(&vendor::omp::host_tool_result(
            id,
            &report.output,
            report.exit_code,
            report.failed,
        ));
        if let Some(job) = kept {
            self.deliver(&job, terminal.follow(&job, control).as_ref());
        }
    }

    /// Deliver a kept command's ending as steering, in omp's own notice, and
    /// record that the lane heard it; a stopped job has nothing to deliver.
    fn deliver(&self, job: &Job, report: Option<&Report>) {
        let heard = report.is_none_or(|report| {
            self.steer(&vendor::omp::job_delivery(job.id(), &report.output))
                .is_ok()
        });
        if heard {
            let _recorded = job.mark_delivered();
        }
    }

    /// Say what the lane is owed at its start: the standing inbox, and the
    /// ending of every job the lane has not heard, each followed to its end
    /// on a thread of its own.
    pub(in crate::resident) fn catch_up(self: &Arc<Self>) {
        self.deliver_inbox();
        let Some(terminal) = self.terminal.clone() else {
            return;
        };
        for job in terminal.undelivered() {
            let driver = Arc::clone(self);
            let terminal = terminal.clone();
            let _followed = std::thread::Builder::new()
                .name("spice-catch-up".to_owned())
                .spawn(move || {
                    let report = terminal.follow(&job, &Control::new());
                    driver.deliver(&job, report.as_ref());
                });
        }
    }

    /// End the work one withdrawn call is being answered with, if it still runs.
    fn cancel(&self, id: &str) {
        let handle = self
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        if let Some(handle) = handle {
            handle.cancel();
        }
    }

    fn ask(&self, dialog: &str, title: &str, message: &str) -> Ask {
        let request = super::super::new_uuid();
        self.asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request, dialog.to_owned());
        Ask {
            request,
            operation: match (title.trim(), message.trim()) {
                (title, "") => title.to_owned(),
                ("", message) => message.to_owned(),
                (title, message) => format!("{title}: {message}"),
            },
        }
    }

    /// Interrupt the agent's turn and end its input: how a drain begins.
    ///
    /// omp stops the turn where it stands and records the assistant message
    /// as aborted, then reads the end of its input and exits on its own. The
    /// resident's signal is only for an occupant that does neither.
    pub(in crate::resident) fn abort(&self) {
        let _aborted = self.write(&vendor::omp::abort_request());
        let running =
            std::mem::take(&mut *self.calls.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in running.into_values() {
            handle.cancel();
        }
        self.close();
    }

    /// End the child's input; further deliveries are dropped.
    pub(in crate::resident) fn close(&self) {
        self.stdin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    /// Interjections the driver has written so far.
    #[cfg(test)]
    fn interjections(&self) -> u64 {
        self.sequence.load(Ordering::Relaxed)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the child's command stream is closed",
            ));
        };
        stream.write_all(bytes)?;
        stream.flush()
    }
}

/// Where this lane's driver listens for republished-inbox kicks.
///
/// `sockaddr_un.sun_path` is only 104 bytes on Darwin, so the door is one
/// content-addressed leaf beneath the short system temporary path, exactly
/// as the capture socket is.
pub(in crate::resident) fn kick_path(paths: &RuntimePaths, lane: CanonicalUuid) -> PathBuf {
    let mut identity = blake3::Hasher::new();
    identity.update(paths.common.as_os_str().as_encoded_bytes());
    identity.update(lane.to_string().as_bytes());
    kick_path_beneath(Path::new("/tmp"), &identity.finalize().to_string())
}

fn kick_path_beneath(root: &Path, identity: &str) -> PathBuf {
    root.join(format!("spice-k-{identity}.sock"))
}

/// Tell whichever driver listens for this lane that its inbox changed.
///
/// Nothing listens for a lane without a command-driven occupant, and a
/// driver that misses a kick reads the current inbox on the next one, so
/// the send is fire-and-forget.
pub(in crate::resident) fn kick(paths: &RuntimePaths, lane: CanonicalUuid) {
    send(paths, lane, &Kick::Inbox);
}

/// A committed prod is also available at the next shell/hook meter boundary.
/// Command-driven occupants can hear it now without cancelling their jobs.
pub(in crate::resident) fn narrate(paths: &RuntimePaths, prod: &wire::body::NarrationProdded) {
    send(paths, prod.lane, &Kick::Narration(prod.clone()));
}

/// Tell whichever driver listens for this lane how consent ruled on one of
/// its requests.
///
/// An occupant that has since left took its dialog with it, so the answer is
/// as fire-and-forget as an inbox kick.
pub(in crate::resident) fn answer(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    request: CanonicalUuid,
    confirmed: bool,
) {
    send(paths, lane, &Kick::Answer { request, confirmed });
}

/// Interrupt the driver listening at `door`, if one does.
///
/// A lane without a command-driven occupant has no door, and a door whose
/// supervisor has gone refuses the datagram; either way the error says
/// nobody is there to interrupt, so a signal is the only word left.
///
/// # Errors
///
/// Returns the send error when no driver takes the kick.
pub(in crate::resident) fn abort(door: &Path) -> io::Result<()> {
    send_to(door, &Kick::Abort)
}

fn send(paths: &RuntimePaths, lane: CanonicalUuid, kick: &Kick) {
    let _sent = send_to(&kick_path(paths, lane), kick);
}

fn send_to(door: &Path, kick: &Kick) -> io::Result<()> {
    #[cfg(unix)]
    {
        let bytes = serde_json::to_vec(kick).expect("a kick is representable as JSON");
        std::os::unix::net::UnixDatagram::unbound()?.send_to(&bytes, door)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (door, kick);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "kick doors require Unix",
        ))
    }
}

/// Bind this lane's kick door and let every kick reach the driver.
///
/// A stale door from an earlier supervisor is replaced. The listening thread
/// lives as long as the supervisor process; the door is removed once the
/// child has exited. A datagram that says nothing this door understands is
/// dropped, since the sender never waits for it.
///
/// # Errors
///
/// Returns the bind error when the door cannot be opened.
pub(in crate::resident) fn listen_for_kicks(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    driver: Arc<Driver>,
) -> io::Result<KickDoor> {
    let path = kick_path(paths, lane);
    #[cfg(unix)]
    {
        let _ = fs::remove_file(&path);
        let socket = std::os::unix::net::UnixDatagram::bind(&path)?;
        std::thread::Builder::new()
            .name("spice-kick".to_owned())
            .spawn(move || {
                let mut kick = [0u8; KICK_BYTES];
                while let Ok(length) = socket.recv(&mut kick) {
                    match serde_json::from_slice(&kick[..length]) {
                        Ok(Kick::Inbox) => driver.deliver_inbox(),
                        Ok(Kick::Answer { request, confirmed }) => {
                            driver.answer(request, confirmed);
                        }
                        Ok(Kick::Narration(prod)) if prod.occupant == occupant => {
                            driver.deliver_narration(&prod);
                        }
                        Ok(Kick::Narration(_)) => {}
                        Ok(Kick::Abort) => driver.abort(),
                        Err(_) => {}
                    }
                }
            })?;
    }
    #[cfg(not(unix))]
    let _ = driver;
    Ok(KickDoor { path })
}

/// The bound kick door, removed when the supervisor is done with it.
pub(in crate::resident) struct KickDoor {
    path: PathBuf,
}

impl Drop for KickDoor {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests;

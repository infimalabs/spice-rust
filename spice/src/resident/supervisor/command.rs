//! Driving a vendor over its stdin: the opening request, interjected
//! steering, and the close that ends its one turn.
//!
//! A vendor launched over a command stream reads nothing from its arguments
//! after start. The supervisor owns the pipe: it writes the opening request,
//! obtains the current authenticated steering inbox from the resident when
//! kicked, answers dialogs, and drops the pipe once the agent's turn completes
//! so the child drains and exits on its own. A drain interrupts the turn over
//! the same pipe before any signal is sent. A confirm dialog becomes one
//! consent request the resident decides; a call on the lane's shell runs in
//! the lane's own terminal and answers with what the command said; every other
//! dialog is declined, since nobody at the lane can compose a choice or text
//! for it.
//!
//! Steering never waits on a command: unheard direction arriving while a
//! command runs interrupts the call, which answers with what the command said
//! so far while the command keeps running as a job. The resident reaches the
//! driver through a datagram signal; the payload remains in journal authority.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use listen::{Decoded, Dialog, Event, ProseRole, TurnMoment};
use serde::{Deserialize, Serialize};
use vendor::omp::{JobCall, ShellCall};
use wire::{CanonicalUuid, Disposition};

use super::super::RuntimePaths;
use super::terminal::{Control, Handle, Job, Report, Run, Terminal};

/// Bytes one kick datagram may carry, including private occupant-scoped notices.
const KICK_BYTES: usize = 1024;

/// One child's command stream and what has already been said on it.
type InboxReader = Box<dyn Fn() -> Option<Vec<u8>> + Send + Sync>;

pub(in crate::resident) struct Driver<W: Write + Send = std::process::ChildStdin> {
    stdin: Mutex<Option<W>>,
    read_inbox: InboxReader,
    sequence: AtomicU64,
    delivered: Mutex<Option<Vec<u8>>>,
    /// Keys whose telling this occupant still holds verbatim, so a resend can
    /// point at it instead of spending it again. A compaction drops the
    /// context, and with it this set.
    told: Mutex<BTreeSet<String>>,
    /// Told keys after whose telling the occupant wrote visible prose, so a
    /// resend can say whether it saw a reply at all. Reasoning and tool
    /// arguments are not prose. Cleared with `told`.
    replied: Mutex<BTreeSet<String>>,
    /// Hidden-reasoning attempts already corrected in this context and pending
    /// inbox generation. The text itself is never retained.
    reasoning_reminder: Mutex<ReasoningReminder>,
    /// Latest claimed-task generation privately delivered to this occupant.
    task_generation: Mutex<Option<(String, u64, u64)>>,
    /// The confirm dialog each open consent request will answer.
    asks: Mutex<HashMap<CanonicalUuid, String>>,
    /// Where the occupant's commands run, for an occupant that has a shell.
    terminal: Option<Terminal>,
    /// The calls being answered now, by the host tool call each answers.
    calls: Mutex<HashMap<String, Handle>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum InboxDelivery {
    Update,
    PostCompaction,
}

#[derive(Default)]
struct ReasoningReminder {
    pending: Option<Vec<u8>>,
    nudged: BTreeSet<steer::Key>,
}

struct PendingKeys(BTreeSet<steer::Key>);

impl steer::Standing for PendingKeys {
    fn disposition(&self, key: &steer::Key) -> Option<Disposition> {
        self.0.contains(key).then_some(Disposition::Live)
    }
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
    /// The lane's journal-backed steering inbox changed.
    Inbox,
    /// Consent ruled on a request the occupant raised.
    Answer {
        request: CanonicalUuid,
        confirmed: bool,
    },
    /// A committed progress reminder for this exact occupant.
    Narration(wire::body::NarrationProdded),
    /// A claimed task changed after this occupant's claim acknowledged it.
    TaskGeneration(TaskGenerationNotice),
    /// The lane is being drained: interrupt the turn and end the input.
    Abort,
}

/// One private task-authority reminder, scoped to the claim's exact occupant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct TaskGenerationNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: String,
    pub(in crate::resident) claim: u64,
    pub(in crate::resident) generation: u64,
    pub(in crate::resident) change: String,
    pub(in crate::resident) editor: String,
    pub(in crate::resident) position: u64,
}
impl<W: Write + Send + 'static> Driver<W> {
    /// Own a child's stdin, authenticated resident discovery, lane identity,
    /// steering media root, and the terminal its commands run in.
    pub(in crate::resident) fn new(
        stdin: W,
        discovery: impl Into<PathBuf>,
        steering_media: PathBuf,
        lane: CanonicalUuid,
        terminal: Option<Terminal>,
    ) -> Arc<Self> {
        let discovery = discovery.into();
        let projector = media::Projector::new(steering_media, lane);
        Self::with_inbox_reader(
            stdin,
            move || {
                let inbox = crate::command_client::read_lane_inbox(&discovery, lane).ok()?;
                let lane_inbox = inbox.lanes.iter().find(|row| row.lane == lane)?;
                // Direction outranks its pictures: a projection failure still
                // delivers the text and references, and says why on stderr.
                let projected = projector.project(lane_inbox).unwrap_or_else(|error| {
                    eprintln!("spice supervisor: steering media projection failed: {error}");
                    media::Projected::new()
                });
                let seal = steer::Key::for_identity(lane);
                Some(
                    steer::inbox::render_pending(
                        &seal,
                        &lane_inbox.pending,
                        |directive, ordinal| projected.get(&(directive, ordinal)).cloned(),
                    )
                    .into_bytes(),
                )
            },
            terminal,
        )
    }

    fn with_inbox_reader(
        stdin: W,
        read_inbox: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static,
        terminal: Option<Terminal>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            read_inbox: Box::new(read_inbox),
            sequence: AtomicU64::new(0),
            delivered: Mutex::new(None),
            told: Mutex::new(BTreeSet::new()),
            replied: Mutex::new(BTreeSet::new()),
            reasoning_reminder: Mutex::new(ReasoningReminder::default()),
            task_generation: Mutex::new(None),
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

    /// Read and interject the current authenticated inbox unless it is empty
    /// or exactly what the agent already received. Every call being answered
    /// is interrupted before unheard direction is written.
    pub(in crate::resident) fn deliver_inbox(&self) {
        self.interject(InboxDelivery::Update);
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

    fn deliver_task_generation(&self, notice: &TaskGenerationNotice) {
        let previous = {
            let mut delivered = self
                .task_generation
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref().is_some_and(|(task, claim, generation)| {
                task == &notice.task && *claim == notice.claim && *generation >= notice.generation
            }) {
                return;
            }
            delivered.replace((notice.task.clone(), notice.claim, notice.generation))
        };
        for handle in self
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            handle.interrupt();
        }
        let message = format!(
            "<system-notice>\nTask {} changed after this claim last acknowledged it: \
             generation {} is {} by {} at position {}. Run `spice task reclaim {}` \
             to take the current task before continuing. Completion and review remain \
             refused until the reclaim succeeds. This is a private authority reminder; \
             no acknowledgment key or visible ACK/NAK is required.\n</system-notice>",
            notice.task,
            notice.generation,
            notice.change,
            notice.editor,
            notice.position,
            notice.task,
        );
        if self.steer(&message).is_err() {
            let mut delivered = self
                .task_generation
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref().is_some_and(|(task, claim, generation)| {
                task == &notice.task && *claim == notice.claim && *generation == notice.generation
            }) {
                *delivered = previous;
            }
        }
    }

    /// Interject the journal-backed inbox once per rendering.
    ///
    /// Direction the lane has not heard interrupts the calls in flight, so the
    /// agent sees it as early as it can; a rendering that only restates what
    /// the lane heard waits for the turn's next boundary. After a compaction,
    /// the replay carries one authenticated recovery instruction, and an empty
    /// inbox still hears how to acknowledge inline.
    fn interject(&self, delivery: InboxDelivery) {
        let Some(rendered) = (self.read_inbox)().filter(|rendered| !rendered.is_empty()) else {
            if delivery == InboxDelivery::PostCompaction {
                let _sent = self.steer(steer::inbox::POST_COMPACTION_NOTICE);
            }
            return;
        };
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if delivered.as_deref() == Some(rendered.as_slice()) {
            return;
        }
        let message =
            std::str::from_utf8(&rendered).expect("the in-memory inbox renderer produces UTF-8");
        let post_compaction;
        let message = if delivery == InboxDelivery::PostCompaction {
            post_compaction = steer::inbox::post_compaction(message);
            post_compaction.as_str()
        } else {
            message
        };
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        let unheard = steer::inbox::keys(message).any(|key| !told.contains(key));
        if delivery == InboxDelivery::Update && unheard {
            for handle in self
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
            {
                handle.interrupt();
            }
        }
        let replied = self.replied.lock().unwrap_or_else(PoisonError::into_inner);
        let resent = steer::inbox::keys(message)
            .any(|key| told.contains(key))
            .then(|| {
                steer::inbox::nudge(message, |key| {
                    told.contains(key).then(|| {
                        if replied.contains(key) {
                            steer::inbox::Told::Replied
                        } else {
                            steer::inbox::Told::Silent
                        }
                    })
                })
            });
        drop(replied);
        if self.steer(resent.as_deref().unwrap_or(message)).is_ok() {
            told.extend(steer::inbox::keys(message).map(str::to_owned));
            *delivered = Some(rendered);
        }
    }

    /// Interject one message as steering, in sequence with the inbox.
    fn steer(&self, message: &str) -> io::Result<()> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.write(&vendor::omp::steer_request(sequence, message))
    }

    /// Correct attempts to answer pending steering in hidden reasoning.
    ///
    /// Detection reuses the prose scanner but consumes none of its outcomes:
    /// this path writes only a private notice and never reaches journal
    /// authority.
    fn nudge_hidden_acknowledgements(&self, decoded: &Decoded) {
        let acknowledgement_prose = decoded
            .events()
            .iter()
            .filter_map(|event| {
                let Event::Reasoning(listen::Reasoning {
                    content: listen::ReasoningContent::Plain(parts),
                    ..
                }) = event
                else {
                    return None;
                };
                Some(parts.as_slice())
            })
            .flatten()
            .filter(|prose| steer::has_acknowledgement(prose))
            .collect::<Vec<_>>();
        if acknowledgement_prose.is_empty() {
            return;
        }
        let pending = (self.read_inbox)().unwrap_or_default();
        let Ok(rendered) = std::str::from_utf8(&pending) else {
            return;
        };
        let standing = PendingKeys(
            steer::inbox::keys(rendered)
                .filter_map(|key| key.parse().ok())
                .collect(),
        );
        let mut reminder = self
            .reasoning_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if reminder.pending.as_deref() != Some(pending.as_slice()) {
            reminder.pending = Some(pending);
            reminder.nudged.clear();
        }

        let mut mentioned = BTreeSet::new();
        for prose in acknowledgement_prose {
            mentioned.extend(
                steer::scan(prose, &standing).into_iter().filter_map(
                    |recognition| match recognition.outcome {
                        steer::Outcome::Retired { key, .. } => Some(key),
                        steer::Outcome::Missed { .. } => None,
                    },
                ),
            );
        }
        mentioned.retain(|key| !reminder.nudged.contains(key));
        if mentioned.is_empty() {
            return;
        }
        let keys = mentioned
            .iter()
            .map(steer::Key::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!(
            "<system-notice>\nReasoning is not acknowledgment. Reply in visible assistant prose \
             with `ACK {keys}: response` or `NAK {keys}: reason`.\n</system-notice>"
        );
        if self.steer(&message).is_ok() {
            reminder.nudged.extend(mentioned);
        }
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
        let replied = decoded.events().iter().any(|event| {
            matches!(event, Event::Prose(prose)
                if prose.role == ProseRole::Assistant && !prose.text.trim().is_empty())
        });
        if replied {
            let told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            self.replied
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(told.iter().cloned());
        }
        let compacted = decoded
            .events()
            .iter()
            .any(|event| matches!(event, Event::Compaction(_)));
        if compacted {
            // Match interject's lock order: no delivery may observe only one reset.
            let mut delivered = self
                .delivered
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            told.clear();
            self.replied
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
            drop(told);
            delivered.take();
            drop(delivered);
            self.interject(InboxDelivery::PostCompaction);
            self.reasoning_reminder
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .nudged
                .clear();
        }
        self.nudge_hidden_acknowledgements(decoded);
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

/// Privately notify the exact claimant occupant that its task authority moved.
pub(in crate::resident) fn task_generation(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: TaskGenerationNotice,
) {
    send(paths, lane, &Kick::TaskGeneration(notice));
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
                driver.catch_up();
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
                        Ok(Kick::TaskGeneration(notice)) if notice.occupant == occupant => {
                            driver.deliver_task_generation(&notice);
                        }
                        Ok(Kick::TaskGeneration(_)) => {}
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

mod media;
#[cfg(test)]
mod tests;

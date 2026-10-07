//! Driving a vendor over its stdin: the opening request, interjected
//! steering, and the close that ends its one turn.
//!
//! A vendor launched over a command stream reads nothing from its arguments
//! after start. The supervisor owns the pipe: it writes the opening request,
//! obtains the current authenticated steering inbox from the resident when
//! kicked, answers dialogs, and drops the pipe once the admitted prompt has
//! settled so the child drains and exits on its own. A drain interrupts the
//! turn over the same pipe before any signal is sent. A confirm dialog becomes one
//! consent request the resident decides; a call on the lane's shell runs in
//! the lane's own terminal and answers with what the command said; every other
//! dialog is declined, since nobody at the lane can compose a choice or text
//! for it.
//!
//! Steering never waits on a command: unheard direction arriving while a
//! command runs interrupts the call, which answers with what the command said
//! so far while the command keeps running as a job. The resident reaches the
//! driver through a datagram signal; the payload remains in journal authority.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use listen::{Decoded, Dialog, Event, ProseRole};
use serde::{Deserialize, Serialize};
use vendor::omp::{JobCall, ShellCall};
use wire::{CanonicalUuid, Disposition};

use super::super::RuntimePaths;
use super::terminal::{Control, Handle, Job, Report, Run, Terminal};

/// Bytes one kick datagram may carry, including private occupant-scoped notices.
const KICK_BYTES: usize = 1024;
const RETUNE_REPLY_BYTES: usize = 4096;
const RETUNE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One child's command stream and what has already been said on it.
type InboxReader = Box<dyn Fn() -> Option<Vec<u8>> + Send + Sync>;
/// Composes the lane's folded briefing; `None` when there is none to give.
type BriefingReader = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub(in crate::resident) struct Driver<W: Write + Send = std::process::ChildStdin> {
    stdin: Mutex<Option<W>>,
    read_inbox: InboxReader,
    /// Read once per compaction, off the decoding path, since a briefing
    /// scans the transcript and the journal.
    read_briefing: BriefingReader,
    sequence: AtomicU64,
    delivered: Mutex<Option<Vec<u8>>>,
    /// Latest queued background notice per purpose. Superseded notices are
    /// coalesced in the extension until omp's next step boundary.
    background: Mutex<BTreeMap<String, String>>,
    /// Keys whose original was queued without replacement, or whose recovery
    /// was observed on the transcript. Reminders can point at that telling.
    /// Compaction drops the context and this set.
    told: Mutex<BTreeSet<String>>,
    /// Successfully delivered keys recognized for this occupant's lifetime,
    /// even when compaction drops the verbatim telling.
    recognized: Mutex<BTreeSet<String>>,
    /// Told keys after whose telling the occupant wrote visible prose, so a
    /// resend can say whether it saw a reply at all. Reasoning and tool
    /// arguments are not prose. Cleared with `told`.
    replied: Mutex<BTreeSet<String>>,
    /// Hidden-reasoning attempts already corrected in this context and pending
    /// inbox generation. The text itself is never retained.
    reasoning_reminder: Mutex<ReasoningReminder>,
    /// The seal wrapping this lane's steering, which prose may name without
    /// naming a directive.
    seal: steer::Key,
    /// Tokens a visible acknowledgment header named without holding them, and
    /// already corrected in this context.
    misdirected: Mutex<BTreeSet<String>>,
    /// Latest claimed-task generation privately delivered to this occupant.
    task_generation: Mutex<Option<(String, u64, u64)>>,
    /// The confirm dialog each open consent request will answer.
    asks: Mutex<HashMap<CanonicalUuid, String>>,
    /// Where the occupant's commands run, for an occupant that has a shell.
    terminal: Option<Terminal>,
    /// The calls being answered now, by the host tool call each answers.
    calls: Mutex<HashMap<String, Handle>>,
    /// Correlated live retunes awaiting an OMP response.
    retunes: Mutex<HashMap<String, PendingRetune>>,
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
/// A live OMP setting change. Exact model parts are required because OMP's
/// runtime command does not admit the CLI's fuzzy selector.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "setting", rename_all = "snake_case")]
pub(in crate::resident) enum LiveRetune {
    Effort { effort: String },
    Model { provider: String, model: String },
    Fast { enabled: bool },
}

impl LiveRetune {
    fn command(&self) -> &'static str {
        match self {
            Self::Effort { .. } => "set_thinking_level",
            Self::Model { .. } => "set_model",
            Self::Fast { .. } => "set_fast_mode",
        }
    }

    fn request(&self, id: &str) -> Vec<u8> {
        match self {
            Self::Effort { effort } => vendor::omp::set_thinking_level_request(id, effort),
            Self::Model { provider, model } => vendor::omp::set_model_request(id, provider, model),
            Self::Fast { enabled } => vendor::omp::set_fast_mode_request(id, *enabled),
        }
    }
}

struct PendingRetune {
    command: &'static str,
    reply: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct RetuneReply {
    request: String,
    result: RetuneReplyResult,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
enum RetuneReplyResult {
    Applied,
    Refused { detail: String },
}

/// Confirmed disposition of one attempted live provider retune.
pub(in crate::resident) enum LiveRetuneResult {
    Applied,
    Refused,
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
    /// Change one setting on this exact running occupant and answer through a
    /// one-shot callback door after the correlated provider response.
    Retune {
        occupant: CanonicalUuid,
        request: String,
        retune: LiveRetune,
        reply: PathBuf,
    },
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
        worktree: PathBuf,
        terminal: Option<Terminal>,
    ) -> Arc<Self> {
        let discovery = discovery.into();
        let projector = media::Projector::new(steering_media, lane);
        let environment = crate::session_environment().rooted(worktree);
        Self::with_readers(
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
            move || {
                Some(crate::session_command::lane_briefing(
                    &environment,
                    lane,
                    None,
                ))
            },
            steer::Key::for_identity(lane),
            terminal,
        )
    }

    fn with_readers(
        stdin: W,
        read_inbox: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static,
        read_briefing: impl Fn() -> Option<String> + Send + Sync + 'static,
        seal: steer::Key,
        terminal: Option<Terminal>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            read_inbox: Box::new(read_inbox),
            read_briefing: Box::new(read_briefing),
            sequence: AtomicU64::new(0),
            delivered: Mutex::new(None),
            background: Mutex::new(BTreeMap::new()),
            told: Mutex::new(BTreeSet::new()),
            recognized: Mutex::new(BTreeSet::new()),
            replied: Mutex::new(BTreeSet::new()),
            reasoning_reminder: Mutex::new(ReasoningReminder::default()),
            seal,
            misdirected: Mutex::new(BTreeSet::new()),
            task_generation: Mutex::new(None),
            asks: Mutex::new(HashMap::new()),
            terminal,
            calls: Mutex::new(HashMap::new()),
            retunes: Mutex::new(HashMap::new()),
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
    /// Submit one live setting change after registering its exact correlated
    /// response. A pipe failure is answered through the same callback path as
    /// a provider refusal.
    fn retune(&self, request: String, retune: LiveRetune, reply: PathBuf) {
        let command = retune.command();
        self.retunes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.clone(), PendingRetune { command, reply });
        if let Err(error) = self.write(&retune.request(&request))
            && let Some(pending) = self
                .retunes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&request)
        {
            answer_retune(
                &pending.reply,
                RetuneReply {
                    request,
                    result: RetuneReplyResult::Refused {
                        detail: error.to_string(),
                    },
                },
            );
        }
    }

    /// Settle a pending live retune only from its matching provider response.
    fn observe_retune(&self, decoded: &Decoded) {
        let Some(response) = decoded
            .context()
            .and_then(|context| context.response.as_ref())
        else {
            return;
        };
        let Some(request) = response.id.as_ref() else {
            return;
        };
        let Some(pending) = self
            .retunes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(request)
        else {
            return;
        };
        let result = if response.command != pending.command {
            RetuneReplyResult::Refused {
                detail: format!(
                    "provider answered {} with {}",
                    pending.command, response.command
                ),
            }
        } else if response.success {
            RetuneReplyResult::Applied
        } else {
            RetuneReplyResult::Refused {
                detail: response
                    .error
                    .clone()
                    .unwrap_or_else(|| "provider refused without detail".to_owned()),
            }
        };
        answer_retune(
            &pending.reply,
            RetuneReply {
                request: request.clone(),
                result,
            },
        );
    }

    /// Read and interject the current authenticated inbox unless it is empty
    /// or exactly what the agent already received. Every call being answered
    /// is interrupted before unheard direction is written.
    pub(in crate::resident) fn deliver_inbox(&self) {
        self.interject(InboxDelivery::Update);
    }

    /// Replace the progress reminder waiting for the next step boundary.
    fn deliver_narration(&self, prod: &wire::body::NarrationProdded) {
        let urgency = if prod.count > 1 {
            "Progress update overdue"
        } else {
            "Progress update due"
        };
        let message = format!(
            "<system-notice>\n{urgency}: {} tool calls since your last visible prose \
             (reminder {}). Give the operator one short status line now. Keep healthy \
             work running; do not cancel it to answer this reminder. No acknowledgment \
             key is required.\n</system-notice>",
            prod.moments, prod.count,
        );
        let _sent = self.background("narration", &message);
    }

    /// Replace a stale-claim reminder without interrupting current work.
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
        if self
            .background(&format!("task:{}", notice.task), &message)
            .is_err()
        {
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

    /// Send new direction once; replace reminders in the SDK aside bridge.
    /// An empty inbox cancels a queued reminder. Compaction replays pending
    /// direction in full without interrupting the response that follows it.
    fn interject(&self, delivery: InboxDelivery) {
        let Some(rendered) = (self.read_inbox)() else {
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
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        let mut recognized = self
            .recognized
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let peers = steer::inbox::peer_keys(message).collect::<BTreeSet<_>>();
        let fresh = if delivery == InboxDelivery::Update {
            steer::inbox::retain(message, |key| {
                !recognized.contains(key) && !peers.contains(key)
            })
        } else {
            String::new()
        };
        let standing = steer::inbox::retain(message, |key| {
            recognized.contains(key) || peers.contains(key)
        });
        if !fresh.is_empty() {
            for handle in self
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
            {
                handle.interrupt();
            }
            if self.steer(&fresh).is_err() {
                return;
            }
            // This original is never replaced or removed. A later reminder
            // follows it, whether omp took it live or at a tool boundary.
            told.extend(steer::inbox::keys(&fresh).map(str::to_owned));
            recognized.extend(steer::inbox::keys(&fresh).map(str::to_owned));
        }
        drop(recognized);
        let previous = delivered
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let unchanged = previous == Some(standing.as_str());
        if delivery == InboxDelivery::Update && unchanged && !standing.is_empty() {
            *delivered = Some(rendered);
            return;
        }
        let replied = self.replied.lock().unwrap_or_else(PoisonError::into_inner);
        let reminder = if delivery == InboxDelivery::PostCompaction {
            steer::inbox::post_compaction(message)
        } else {
            steer::inbox::nudge(&standing, |key| {
                told.contains(key).then(|| {
                    if replied.contains(key) {
                        steer::inbox::Told::Replied
                    } else {
                        steer::inbox::Told::Silent
                    }
                })
            })
        };
        drop(replied);
        if self.background("inbox", &reminder).is_ok() {
            self.recognized
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(steer::inbox::keys(message).map(str::to_owned));
            *delivered = Some(rendered);
        }
    }

    /// Interject one message as steering, in sequence with the inbox.
    fn steer(&self, message: &str) -> io::Result<()> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.write(&vendor::omp::steer_request(sequence, message))
    }

    /// Queue the lane's briefing beside the replayed steering once a
    /// compaction has dropped the context it would have been read in.
    ///
    /// The briefing is composed on its own thread: it scans the transcript
    /// and the journal, and the decoding path must keep reading events.
    fn deliver_briefing(self: &Arc<Self>) {
        let driver = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("spice-supervisor-briefing".to_owned())
            .spawn(move || {
                if let Some(briefing) = (driver.read_briefing)() {
                    let _ = driver.background("briefing", &briefing);
                }
            });
        if let Err(error) = spawned {
            eprintln!("spice supervisor: briefing thread unavailable: {error}");
        }
    }

    /// Replace one queued notice without interrupting model or tool work.
    fn background(&self, purpose: &str, message: &str) -> io::Result<()> {
        self.update_background(purpose, |_| message.to_owned())
    }

    fn update_background(
        &self,
        purpose: &str,
        update: impl FnOnce(&str) -> String,
    ) -> io::Result<()> {
        let mut queued = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let previous = queued.get(purpose).map_or("", String::as_str);
        let message = update(previous);
        if previous == message {
            return Ok(());
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.write(&vendor::omp::aside_request(sequence, purpose, &message))?;
        if message.is_empty() {
            queued.remove(purpose);
        } else {
            queued.insert(purpose.to_owned(), message);
        }
        Ok(())
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
            "<system-notice>\nReasoning is not acknowledgment. Lead visible assistant prose, and \
             the first tool intent when the message calls a tool, with `ACK {keys}: response` or \
             `NAK {keys}: reason`.\n</system-notice>"
        );
        if self.background("hidden-ack", &message).is_ok() {
            reminder.nudged.extend(mentioned);
        }
    }

    /// Correct visible acknowledgment headers that name a key the occupant
    /// does not hold.
    ///
    /// A header naming a mistyped key, or one retired before this occupant was
    /// told anything, retires nothing, and an occupant that never hears so
    /// believes it answered. A key still pending, one this occupant was told,
    /// and the lane's own seal stay silent: the first retires, and the others
    /// were really given to this occupant. Like the reasoning correction this
    /// writes only a private notice and never reaches journal authority.
    fn nudge_misdirected_acknowledgements(&self, decoded: &Decoded) {
        let prose = decoded
            .events()
            .iter()
            .filter_map(|event| match event {
                Event::Prose(prose)
                    if prose.role == ProseRole::Assistant
                        && steer::has_acknowledgement(&prose.text) =>
                {
                    Some(prose.text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if prose.is_empty() {
            return;
        }
        let inbox = (self.read_inbox)().unwrap_or_default();
        let pending = std::str::from_utf8(&inbox)
            .map(|rendered| steer::inbox::keys(rendered).collect::<Vec<_>>())
            .unwrap_or_default();
        let standing = PendingKeys(pending.iter().filter_map(|key| key.parse().ok()).collect());
        let recognized = self
            .recognized
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut misdirected = self
            .misdirected
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut named = BTreeSet::new();
        for text in prose {
            for recognition in steer::scan(text, &standing) {
                if !recognition.outcome.placed() {
                    continue;
                }
                let steer::Outcome::Missed { token, .. } = recognition.outcome else {
                    continue;
                };
                if token != self.seal.as_str()
                    && !recognized.contains(&token)
                    && !misdirected.contains(&token)
                {
                    named.insert(token);
                }
            }
        }
        if named.is_empty() {
            return;
        }
        let quoted = |keys: &mut dyn Iterator<Item = &str>| {
            keys.map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pending = if pending.is_empty() {
            "No steering is pending.".to_owned()
        } else {
            format!(
                "Pending: {}; copy the key exactly as delivered.",
                quoted(&mut pending.iter().copied())
            )
        };
        let message = format!(
            "<system-notice>\nYour acknowledgment named {}, which is not a pending steering key; \
             it acknowledged nothing. {pending}\n</system-notice>",
            quoted(&mut named.iter().map(String::as_str)),
        );
        if self.background("misdirected-ack", &message).is_ok() {
            misdirected.extend(named);
        }
    }

    /// Cancel answered reminders before the asynchronous journal catches up.
    /// Durable steering disposition remains the resident's transcript-backed job.
    fn cancel_answered_reminders(&self, decoded: &Decoded) {
        let acknowledgement = decoded
            .events()
            .iter()
            .filter_map(Event::assistant_text)
            .any(steer::has_acknowledgement);
        let pending = acknowledgement.then(|| (self.read_inbox)().unwrap_or_default());
        if let Some(rendered) = pending
            .as_deref()
            .and_then(|pending| std::str::from_utf8(pending).ok())
        {
            let standing = PendingKeys(
                steer::inbox::keys(rendered)
                    .filter_map(|key| key.parse().ok())
                    .collect(),
            );
            let answered = decoded
                .events()
                .iter()
                .filter_map(Event::assistant_text)
                .flat_map(|text| steer::scan(text, &standing))
                .filter_map(|recognition| match recognition.outcome {
                    steer::Outcome::Retired { key, .. } => Some(key),
                    steer::Outcome::Missed { .. } => None,
                })
                .collect::<BTreeSet<_>>();
            if !answered.is_empty() {
                let _cancelled = self.update_background("inbox", |message| {
                    steer::inbox::retain(message, |key| {
                        key.parse().is_ok_and(|key| !answered.contains(&key))
                    })
                });
                if standing.0.is_subset(&answered) {
                    let _cancelled = self.background("hidden-ack", "");
                }
            }
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
        self.observe_retune(decoded);
        // A telling seen on the transcript is now held verbatim. Background
        // delivery also releases its replacement slot for a future notice.
        for prose in decoded.events().iter().filter_map(|event| match event {
            Event::Prose(prose) if prose.role == ProseRole::Operator => Some(prose),
            _ => None,
        }) {
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            let recognized = self
                .recognized
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            told.extend(
                steer::inbox::keys(&prose.text)
                    .filter(|key| recognized.contains(*key))
                    .map(str::to_owned),
            );
            self.background
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|_, message| !prose.text.contains(message.as_str()));
        }
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
            let _cancelled = self.background("narration", "");
        }
        self.cancel_answered_reminders(decoded);
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
            self.deliver_briefing();
            self.reasoning_reminder
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .nudged
                .clear();
            self.misdirected
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        self.nudge_hidden_acknowledgements(decoded);
        self.nudge_misdirected_acknowledgements(decoded);
        let settled = decoded.context().is_some_and(|context| {
            context.session_settled
                || context
                    .prompt_outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.session_settled)
        });
        if settled {
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

    /// Deliver a kept command's ending as steering, in omp's own notice,
    /// unless the lane already heard it: a job answer that reported the
    /// ending claimed the telling first. The claim precedes the notice, so a
    /// racing answer and notice never both stay silent. It is taken under the
    /// input's lock, so a closed seat claims nothing and the next seat's
    /// catch-up owes the ending, and a notice that fails to write gives the
    /// claim back. A stopped job has nothing to deliver; whoever stopped it
    /// was answered.
    fn deliver(&self, job: &Job, report: Option<&Report>) {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = stdin.as_mut() else {
            return;
        };
        // A record that cannot be created is not a telling anyone owns, so
        // the notice goes out rather than risk silence.
        if !job.claim_heard().unwrap_or(true) {
            return;
        }
        let Some(report) = report else {
            return;
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let notice = vendor::omp::aside_request(
            sequence,
            &format!("job:{}", job.id()),
            &vendor::omp::job_delivery(job.id(), &report.output),
        );
        if written(stream, &notice).is_err() {
            let _released = job.release_heard();
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
        written(stream, bytes)
    }
}

fn written(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(bytes)?;
    stream.flush()
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

/// Apply one setting change to the exact running occupant and wait for OMP's
/// correlated response.
///
/// # Errors
///
/// Returns an I/O error when no matching supervisor can receive the request,
/// its callback cannot be read, or the provider does not answer in time.
pub(in crate::resident) fn retune(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    retune: LiveRetune,
) -> io::Result<LiveRetuneResult> {
    #[cfg(unix)]
    {
        let request = format!(
            "{}{}",
            listen::OMP_RETUNE_REQUEST_ID_PREFIX,
            super::super::new_uuid()
        );
        let mut identity = blake3::Hasher::new();
        identity.update(paths.common.as_os_str().as_encoded_bytes());
        identity.update(request.as_bytes());
        let reply = Path::new("/tmp").join(format!("spice-r-{}.sock", identity.finalize()));
        let _ = fs::remove_file(&reply);
        let socket = std::os::unix::net::UnixDatagram::bind(&reply)?;
        socket.set_read_timeout(Some(RETUNE_TIMEOUT))?;
        let kick = Kick::Retune {
            occupant,
            request: request.clone(),
            retune,
            reply: reply.clone(),
        };
        let result = send_to(&kick_path(paths, lane), &kick).and_then(|()| {
            let mut bytes = [0u8; RETUNE_REPLY_BYTES];
            let length = socket.recv(&mut bytes)?;
            let answer: RetuneReply = serde_json::from_slice(&bytes[..length])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if answer.request != request {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "live retune callback named another request",
                ));
            }
            Ok(match answer.result {
                RetuneReplyResult::Applied => LiveRetuneResult::Applied,
                RetuneReplyResult::Refused { .. } => LiveRetuneResult::Refused,
            })
        });
        drop(socket);
        let _ = fs::remove_file(reply);
        result
    }
    #[cfg(not(unix))]
    {
        let _ = (paths, lane, occupant, retune);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "live retuning requires Unix command doors",
        ))
    }
}

fn answer_retune(reply: &Path, answer: RetuneReply) {
    #[cfg(unix)]
    {
        let bytes = serde_json::to_vec(&answer).expect("a retune reply is representable as JSON");
        let _sent = std::os::unix::net::UnixDatagram::unbound()
            .and_then(|socket| socket.send_to(&bytes, reply));
    }
    #[cfg(not(unix))]
    let _ = (reply, answer);
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
                        Ok(Kick::Retune {
                            occupant: target,
                            request,
                            retune,
                            reply,
                        }) if target == occupant => driver.retune(request, retune, reply),
                        Ok(Kick::Retune { .. }) => {}
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

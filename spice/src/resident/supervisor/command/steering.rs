//! What the occupant hears of its pending steering: new direction once,
//! reminders that replace one another until the next step boundary, the
//! replay and briefing after a compaction, and the private corrections an
//! acknowledgment attempt earns.

mod activity;

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use listen::{Decoded, Event, ProseRole};
use wire::Disposition;

use super::super::terminal::Handle;
use super::Driver;
pub(super) use activity::ActivityReminder;

const ACTIVITY_ACK_PURPOSE: &str = "ack-activity";
const ACTIVITY_ACK_MARKER: &str = "[ACK-ACTIVITY]";

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum InboxDelivery {
    Update,
    PostCompaction,
}

#[derive(Default)]
pub(super) struct ReasoningReminder {
    pending: Option<Vec<u8>>,
    pub(super) nudged: BTreeSet<steer::Key>,
}

struct PendingKeys(BTreeSet<steer::Key>);

impl steer::Standing for PendingKeys {
    fn disposition(&self, key: &steer::Key) -> Option<Disposition> {
        self.0.contains(key).then_some(Disposition::Live)
    }
}

fn activity_message<'a>(keys: impl IntoIterator<Item = &'a String>) -> String {
    let keys = keys
        .into_iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if keys.is_empty() {
        return String::new();
    }
    format!(
        "<system-notice>\n{ACTIVITY_ACK_MARKER} Complete visible response left pending \
         steering unanswered: {keys}. Acknowledge it immediately. {}\n</system-notice>",
        steer::inbox::ACK_GUIDANCE,
    )
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Read the inbox current when the first prompt is released and append
    /// every still-unanswered item to that prompt. Recording the same delivery
    /// state as an ordinary interjection prevents the release's trailing inbox
    /// check from sending those items again.
    pub(super) fn startup_inbox(&self) -> Option<String> {
        let rendered = (self.read_inbox)()?;
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if delivered.as_deref() == Some(rendered.as_slice()) {
            return None;
        }
        let message =
            std::str::from_utf8(&rendered).expect("the in-memory inbox renderer produces UTF-8");
        let owed = {
            let mut answered = self.answered.lock().unwrap_or_else(PoisonError::into_inner);
            answered.settle(message);
            steer::inbox::retain(message, |key| !answered.holds(key))
        };
        *delivered = Some(rendered);
        if owed.is_empty() {
            return None;
        }
        let keys = steer::inbox::keys(&owed)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        self.told
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(keys.iter().cloned());
        self.recognized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(keys);
        Some(owed)
    }

    /// Send new direction once; replace reminders in the SDK aside bridge.
    /// Compaction replays pending direction in full without interrupting the
    /// response that follows it.
    ///
    /// Nothing composed here names a key the occupant already answered while
    /// the journal still lists it. A reminder names a standing key only when
    /// its telling changed since the last delivery, by arriving or by climbing
    /// the resend ladder, or when a queued reminder already names it: another
    /// key arriving or retiring says nothing new about it. An inbox with no
    /// such key prunes the queued reminder to what is still owed.
    ///
    /// Nothing is written while the first turn is held: omp would run the
    /// direction as a turn ahead of the first prompt, whose authority would
    /// then be stale by its delivery. That prompt's release interjects the
    /// inbox current at that moment.
    pub(super) fn interject(&self, delivery: InboxDelivery) {
        if self.registration.holds_first_turn() {
            return;
        }
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
        let owed = {
            let mut answered = self.answered.lock().unwrap_or_else(PoisonError::into_inner);
            answered.settle(message);
            steer::inbox::retain(message, |key| !answered.holds(key))
        };
        self.reconcile_activity(&owed);
        let told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        let recognized = self
            .recognized
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let peers = steer::inbox::peer_keys(&owed).collect::<BTreeSet<_>>();
        let fresh = if delivery == InboxDelivery::Update {
            steer::inbox::retain(&owed, |key| {
                !recognized.contains(key) && !peers.contains(key)
            })
        } else {
            String::new()
        };
        let standing =
            steer::inbox::retain(&owed, |key| recognized.contains(key) || peers.contains(key));
        drop(recognized);
        let (reminder, peer_due) = if delivery == InboxDelivery::PostCompaction {
            (steer::inbox::post_compaction(&owed), false)
        } else {
            let previous = delivered
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok());
            let (queued, replaying) = self
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get("inbox")
                .map(|queued| {
                    (
                        steer::inbox::keys(queued)
                            .map(str::to_owned)
                            .collect::<BTreeSet<_>>(),
                        steer::inbox::replays(queued),
                    )
                })
                .unwrap_or_default();
            let due = steer::inbox::retain(&standing, |key| {
                queued.contains(key)
                    || previous.and_then(|previous| steer::inbox::stage(previous, key))
                        != steer::inbox::stage(&standing, key)
            });
            let peer_due = steer::inbox::peer_keys(&due).next().is_some();
            let reminder = if due.is_empty() {
                String::new()
            } else if replaying {
                steer::inbox::post_compaction(&due)
            } else {
                let replied = self.replied.lock().unwrap_or_else(PoisonError::into_inner);
                steer::inbox::nudge(&due, |key| {
                    told.contains(key).then(|| {
                        if replied.contains(key) {
                            steer::inbox::Told::Replied
                        } else {
                            steer::inbox::Told::Silent
                        }
                    })
                })
            };
            (reminder, peer_due)
        };
        drop(told);

        let (aside_sent, steer_sent) = self.publish_inbox(&fresh, &reminder, peer_due);

        if aside_sent && steer_sent {
            *delivered = Some(rendered);
        }
    }

    /// Publish one changed inbox while the step guard closes registration
    /// against steering delivery.
    fn publish_inbox(&self, fresh: &str, reminder: &str, peer_due: bool) -> (bool, bool) {
        // The call map is inspected while this guard is held; dispatch never
        // holds that map while it waits for the step state.
        let mut step =
            (!fresh.is_empty()).then(|| self.step.lock().unwrap_or_else(PoisonError::into_inner));
        let (fresh_calls, peer_waits) = self.delivery_calls(!fresh.is_empty(), peer_due);

        // Register every background notice before releasing a host call: its
        // result creates the boundary where omp flushes the queued aside.
        let aside_sent = self.background("inbox", reminder).is_ok();
        if aside_sent && fresh.is_empty() {
            for handle in peer_waits {
                handle.interrupt();
            }
        }
        if !fresh.is_empty() {
            // Preserve the established human path: signal the host calls
            // before entering steering, so a running command becomes a kept
            // job rather than a withdrawn command.
            for handle in fresh_calls {
                handle.interrupt();
            }
        }
        let steer_sent = fresh.is_empty() || self.steer(fresh).is_ok();
        if steer_sent && !fresh.is_empty() {
            step.as_mut()
                .expect("fresh steering owns the step guard")
                .mark_stale();
            let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
            told.extend(steer::inbox::keys(fresh).map(str::to_owned));
            self.recognize(fresh);
        }
        if aside_sent {
            self.recognize(reminder);
        }
        (aside_sent, steer_sent)
    }

    /// Replay the complete current unanswered inbox after a stale-generation
    /// blocking tool result. This deliberately bypasses rendering equality:
    /// the earlier publication made this generation stale, and the tool
    /// result is the boundary at which OMP can consume the replay next.
    pub(super) fn replay_inbox(&self) {
        let Some(rendered) = (self.read_inbox)() else {
            return;
        };
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut step = self.step.lock().unwrap_or_else(PoisonError::into_inner);
        if !step.claim_replay() {
            return;
        }
        let message =
            std::str::from_utf8(&rendered).expect("the in-memory inbox renderer produces UTF-8");
        let owed = {
            let mut answered = self.answered.lock().unwrap_or_else(PoisonError::into_inner);
            answered.settle(message);
            steer::inbox::retain(message, |key| !answered.holds(key))
        };
        if owed.is_empty() {
            *delivered = Some(rendered);
            return;
        }
        if self.steer(&owed).is_err() {
            step.release_replay();
            return;
        }
        self.told
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(steer::inbox::keys(&owed).map(str::to_owned));
        self.recognize(&owed);
        *delivered = Some(rendered);
    }
    pub(super) fn delivery_calls(
        &self,
        human_due: bool,
        peer_due: bool,
    ) -> (Vec<Handle>, Vec<Handle>) {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let human = if human_due {
            calls.values().map(|call| call.handle.clone()).collect()
        } else {
            Vec::new()
        };
        let peer = if peer_due {
            calls
                .values()
                .filter(|call| call.blocking_wait)
                .map(|call| call.handle.clone())
                .collect()
        } else {
            Vec::new()
        };
        (human, peer)
    }

    fn recognize(&self, message: &str) {
        self.recognized
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(steer::inbox::keys(message).map(str::to_owned));
    }

    /// Only a correction observed on the operator tape arms its pending keys.
    pub(super) fn observe_activity_reminder(&self, text: &str) {
        let (keys, landed) = {
            let delivered = self
                .delivered
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let pending = delivered
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(steer::inbox::keys)
                .into_iter()
                .flatten()
                .collect::<BTreeSet<_>>();
            let background = self
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let mut keys = BTreeSet::new();
            let mut landed = false;
            for (purpose, message) in background.iter() {
                if !text.contains(message.as_str()) {
                    continue;
                }
                match purpose.as_str() {
                    "inbox" if steer::inbox::replays(message) => {
                        keys.extend(
                            steer::inbox::keys(message)
                                .filter(|key| pending.contains(key))
                                .map(str::to_owned),
                        );
                    }
                    "inbox" => {
                        keys.extend(message.lines().filter_map(|line| {
                            let key = line
                                .strip_prefix("[NUDGE] key=")?
                                .split_whitespace()
                                .next()?;
                            pending.contains(key).then(|| key.to_owned())
                        }));
                    }
                    "hidden-ack" | "misdirected-ack" => {
                        keys.extend(
                            message
                                .split(|character: char| !character.is_ascii_alphanumeric())
                                .filter(|key| pending.contains(key))
                                .map(str::to_owned),
                        );
                    }
                    ACTIVITY_ACK_PURPOSE => landed = true,
                    _ => {}
                }
            }
            (keys, landed)
        };
        let mut activity = self
            .activity_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        activity.arm(keys);
        if landed {
            activity.landed();
        }
    }

    /// Prune a queued activity correction when journal movement settles only
    /// some of its keys. New pending keys remain unarmed.
    fn reconcile_activity(&self, rendered: &str) {
        let pending = steer::inbox::keys(rendered)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let mut activity = self
            .activity_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        activity.settle(&pending);
        let message = activity_message(activity.queued());
        let _replaced = self.background(ACTIVITY_ACK_PURPOSE, &message);
    }

    /// Queue one coalesced correction when a complete visible response leaves
    /// an already-reminded key unanswered. Silence, stream fragments, and tool
    /// activity never call this transition.
    pub(super) fn nudge_unanswered_activity(&self) {
        let inbox = (self.read_inbox)().unwrap_or_default();
        let Ok(rendered) = std::str::from_utf8(&inbox) else {
            return;
        };
        let pending = {
            let mut answered = self.answered.lock().unwrap_or_else(PoisonError::into_inner);
            answered.settle(rendered);
            steer::inbox::keys(rendered)
                .filter(|key| !answered.holds(key))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        };
        let mut activity = self
            .activity_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let due = activity.respond(&pending);
        if due.is_empty() {
            return;
        }
        let message = activity_message(activity.queued().union(&due));
        if self.background(ACTIVITY_ACK_PURPOSE, &message).is_ok() {
            activity.delivered(&due);
        }
    }

    /// Compaction starts a new activity window and drops a correction queued
    /// for the context it replaced.
    pub(super) fn reset_activity_reminder(&self) {
        let mut activity = self
            .activity_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        activity.clear();
        let _cancelled = self.background(ACTIVITY_ACK_PURPOSE, "");
    }

    /// Interject one message as steering, in sequence with the inbox.
    pub(super) fn steer(&self, message: &str) -> io::Result<()> {
        self.activate()?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.write(&vendor::omp::steer_request(sequence, message))
    }

    /// Queue the lane's briefing beside the replayed steering once a
    /// compaction has dropped the context it would have been read in. Its
    /// pending steering omits every key the occupant already answered,
    /// judged as it is queued, so an answer landing while it was composed
    /// counts too.
    ///
    /// The briefing is composed on its own thread: it scans the transcript
    /// and the journal, and the decoding path must keep reading events.
    pub(super) fn deliver_briefing(self: &Arc<Self>) {
        let driver = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("spice-supervisor-briefing".to_owned())
            .spawn(move || {
                if let Some(briefing) = (driver.read_briefing)() {
                    let page = driver.for_seat(briefing);
                    let _ = driver.update_background("briefing", |_| {
                        let answered = driver
                            .answered
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        crate::session_command::digest::without_answered(&page, |key| {
                            answered.holds(key)
                        })
                    });
                }
            });
        if let Err(error) = spawned {
            eprintln!("spice supervisor: briefing thread unavailable: {error}");
        }
    }
    /// Correct attempts to answer pending steering in hidden reasoning.
    ///
    /// Detection reuses the prose scanner but consumes none of its outcomes:
    /// this path writes only a private notice and never reaches journal
    /// authority.
    pub(super) fn nudge_hidden_acknowledgements(&self, decoded: &Decoded) -> bool {
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
            return false;
        }
        let pending = (self.read_inbox)().unwrap_or_default();
        let Ok(rendered) = std::str::from_utf8(&pending) else {
            return false;
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
            return false;
        }
        let keys = mentioned
            .iter()
            .map(steer::Key::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let message = format!(
            "<system-notice>\nReasoning did not acknowledge {keys}. {}\n</system-notice>",
            steer::inbox::ACK_GUIDANCE,
        );
        let sent = self.background("hidden-ack", &message).is_ok();
        if sent {
            reminder.nudged.extend(mentioned);
        }
        sent
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
    pub(super) fn nudge_misdirected_acknowledgements(&self, decoded: &Decoded) -> bool {
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
            return false;
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
            return false;
        }
        let quoted = |keys: &mut dyn Iterator<Item = &str>| {
            keys.map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pending_guidance = if pending.is_empty() {
            "No steering is pending.".to_owned()
        } else {
            format!(
                "Pending: {}; copy the key exactly as delivered.",
                quoted(&mut pending.iter().copied())
            )
        };
        let message = format!(
            "<system-notice>\nNot pending: {}. {pending_guidance} {}\n</system-notice>",
            quoted(&mut named.iter().map(String::as_str)),
            steer::inbox::ACK_GUIDANCE,
        );
        let sent = self.background("misdirected-ack", &message).is_ok();
        if sent {
            misdirected.extend(named);
        }
        sent
    }

    /// Hold every key the occupant's tape-backed assistant text just answered,
    /// and strip it from queued reminders and briefings before journal catch-up.
    ///
    /// This runs as the words are decoded, ahead of the step boundary that
    /// flushes queued notices, so it reads only what this driver last
    /// delivered: asking the resident would race that flush. Durable steering
    /// disposition remains the resident's transcript-backed job.
    pub(super) fn cancel_answered_reminders(&self, decoded: &Decoded) {
        let texts = decoded
            .events()
            .iter()
            .filter_map(Event::assistant_text)
            .filter(|text| steer::has_acknowledgement(text))
            .collect::<Vec<_>>();
        if texts.is_empty() {
            return;
        }
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(rendered) = delivered
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
        else {
            return;
        };
        let standing = PendingKeys(
            steer::inbox::keys(rendered)
                .filter_map(|key| key.parse().ok())
                .collect(),
        );
        let heard = texts
            .iter()
            .flat_map(|text| steer::scan(text, &standing))
            .filter_map(|recognition| match recognition.outcome {
                steer::Outcome::Retired { key, .. } => Some(key.as_str().to_owned()),
                steer::Outcome::Missed { .. } => None,
            })
            .collect::<BTreeSet<_>>();
        if heard.is_empty() {
            return;
        }
        let settled = {
            let mut answered = self.answered.lock().unwrap_or_else(PoisonError::into_inner);
            for key in &heard {
                answered.record(rendered, key);
            }
            steer::inbox::keys(rendered).all(|key| answered.holds(key))
        };
        drop(delivered);
        let _cancelled = self.update_background("inbox", |queued| {
            steer::inbox::retain(queued, |key| !heard.contains(key))
        });
        let _cancelled = self.update_background("briefing", |page| {
            crate::session_command::digest::without_answered(page, |key| heard.contains(key))
        });
        let mut activity = self
            .activity_reminder
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        activity.acknowledge(&heard);
        let message = activity_message(activity.queued());
        let _cancelled = self.background(ACTIVITY_ACK_PURPOSE, &message);
        if settled {
            let _cancelled = self.background("hidden-ack", "");
        }
    }
}

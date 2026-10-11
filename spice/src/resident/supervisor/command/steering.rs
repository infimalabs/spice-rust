//! What the occupant hears of its pending steering: new direction once,
//! reminders that replace one another until the next step boundary, the
//! replay and briefing after a compaction, and the private corrections an
//! acknowledgment attempt earns.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use listen::{Decoded, Event, ProseRole};
use wire::Disposition;

use super::Driver;

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

impl<W: Write + Send + 'static> Driver<W> {
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
    pub(super) fn interject(&self, delivery: InboxDelivery) {
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
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        let mut recognized = self
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
        let reminder = if delivery == InboxDelivery::PostCompaction {
            steer::inbox::post_compaction(&owed)
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
            if due.is_empty() {
                // Every queued key is answered or retired by now.
                if self.background("inbox", "").is_ok() {
                    *delivered = Some(rendered);
                }
                return;
            }
            if replaying {
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
            }
        };
        if self.background("inbox", &reminder).is_ok() {
            self.recognized
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(steer::inbox::keys(message).map(str::to_owned));
            *delivered = Some(rendered);
        }
    }

    /// Interject one message as steering, in sequence with the inbox.
    pub(super) fn steer(&self, message: &str) -> io::Result<()> {
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
    pub(super) fn nudge_hidden_acknowledgements(&self, decoded: &Decoded) {
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
    pub(super) fn nudge_misdirected_acknowledgements(&self, decoded: &Decoded) {
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

    /// Hold every key the occupant's visible words just answered, and strip
    /// it from queued reminders and briefings before the journal catches up.
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
        if settled {
            let _cancelled = self.background("hidden-ack", "");
        }
    }
}

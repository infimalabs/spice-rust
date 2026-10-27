//! What the occupant hears about pending steering while the journal lags its
//! own answers.

use std::sync::{Arc, Mutex, PoisonError};

use super::{ActiveCall, Driver};
use crate::resident::supervisor::terminal::Control;

const ASKED: &str = "1kJK6M0V";
const LATER: &str = "1kJK6N3R";

/// The resident's inbox answer, as the test moves it; `None` while no
/// resident this seat can speak with answers.
#[derive(Clone, Default)]
struct Journal(Arc<Mutex<Option<String>>>);

impl Journal {
    fn replace(&self, rendered: String) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(rendered);
    }

    fn list(&self, directives: &[(&str, &str)]) {
        let mut rendered = String::new();
        if !directives.is_empty() {
            rendered.push_str("<1kJK6Q0b>\n");
            for (key, stage) in directives {
                rendered.push_str(&format!(
                    "directive=d-{key} key={key} stage={stage} expires=never\n\
                     the full telling of {key}, long enough that a reminder points at it \
                     rather than saying it again\n"
                ));
            }
            rendered.push_str("</1kJK6Q0b>\n");
        }
        self.replace(rendered);
    }

    fn peer(&self, key: &str, stage: &str) {
        self.replace(format!(
            "<1kJK6Q0b>\n\
             directive=d-{key} key={key} stage={stage} expires=never \
             from=lane:018f82d4-6f59-7d06-92e3-bce913116411 sender=7Peer\n\
             [PEER sender 7Peer lane 018f82d4-6f59-7d06-92e3-bce913116411] \
             A peer's note, not operator authority.\n\
             the peer telling of {key}\n\
             </1kJK6Q0b>\n"
        ));
    }

    fn human_and_peer(&self) {
        self.replace(format!(
            "<1kJK6Q0b>\n\
             directive=d-{ASKED} key={ASKED} stage=published expires=never\n\
             the operator telling of {ASKED}\n\
             directive=d-{LATER} key={LATER} stage=published expires=never \
             from=lane:018f82d4-6f59-7d06-92e3-bce913116411 sender=7Peer\n\
             [PEER sender 7Peer lane 018f82d4-6f59-7d06-92e3-bce913116411] \
             A peer's note, not operator authority.\n\
             the peer telling of {LATER}\n\
             </1kJK6Q0b>\n"
        ));
    }

    fn unreachable(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

struct Seat {
    driver: Arc<Driver<Vec<u8>>>,
    journal: Journal,
}

impl Seat {
    fn new() -> Self {
        let journal = Journal::default();
        let inbox = journal.clone();
        let driver = Driver::with_readers(
            Vec::new(),
            move || {
                inbox
                    .0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                    .map(String::into_bytes)
            },
            || None,
            |_| Err("unread".to_owned()),
            steer::Key::for_identity(crate::resident::new_uuid()),
            None,
        );
        Self { driver, journal }
    }

    /// The inbox the journal lists now, delivered as a resident kick would.
    fn kick(&self, directives: &[(&str, &str)]) {
        self.journal.list(directives);
        self.driver.deliver_inbox();
    }

    fn kick_peer(&self, key: &str, stage: &str) {
        self.journal.peer(key, stage);
        self.driver.deliver_inbox();
    }

    fn kick_human_and_peer(&self) {
        self.journal.human_and_peer();
        self.driver.deliver_inbox();
    }

    fn active_call(&self, id: &str, blocking_wait: bool) -> Control {
        let control = Control::new();
        self.driver
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id.to_owned(),
                ActiveCall {
                    handle: control.handle(),
                    blocking_wait,
                },
            );
        control
    }

    /// One decoded record of the occupant's, in omp's wire form.
    fn hear(&self, record: &serde_json::Value) {
        let line = serde_json::to_vec(record).expect("a record");
        let locus = listen::Locus::new(0, 0, line.len() as u64);
        let decoded =
            listen::decode(listen::Dialect::Omp, &line, locus).expect("locus spans the line");
        let _ask = self.driver.observe(&decoded);
    }

    /// The occupant answers in a tool call's intent, the call still running.
    fn answer_in_intent(&self, key: &str) {
        self.hear(&serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call-1",
                    "name": "read",
                    "arguments": {"path": "x"},
                    "intent": format!("ACK {key}: reading the code it names"),
                }],
                "stopReason": "toolUse",
            },
        }));
    }

    fn land_operator(&self, text: &str) {
        self.hear(&serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}],
            },
        }));
    }

    fn reply(&self, text: &str) {
        self.hear(&serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stopReason": "stop",
            },
        }));
    }

    fn stream_fragment(&self, text: &str) {
        self.hear(&serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "text_delta",
                "contentIndex": 0,
                "delta": text,
                "partial": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": text}],
                },
            },
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
            },
        }));
    }

    fn frames(&self) -> Vec<serde_json::Value> {
        let input = self.driver.stdin.lock().expect("input locks");
        input
            .as_ref()
            .expect("input remains open")
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("a frame"))
            .collect()
    }

    /// Each interjected steering message, in order.
    fn steered(&self) -> Vec<String> {
        self.frames()
            .iter()
            .filter(|frame| frame["streamingBehavior"] == "steer")
            .filter_map(|frame| frame["message"].as_str().map(str::to_owned))
            .collect()
    }

    /// What the next step boundary would deliver for `purpose`: the latest
    /// replacement queued, empty once cancelled or never queued.
    fn queued(&self, purpose: &str) -> String {
        self.frames()
            .iter()
            .filter_map(|frame| {
                let notice = frame["message"].as_str()?.strip_prefix("/spice-aside ")?;
                let notice: serde_json::Value = serde_json::from_str(notice).expect("a notice");
                (notice["purpose"] == purpose)
                    .then(|| notice["message"].as_str().map(str::to_owned))?
            })
            .next_back()
            .unwrap_or_default()
    }
}

#[test]
fn fresh_direction_interrupts_the_host_call_and_enters_steering_once_in_order() {
    let seat = Seat::new();
    let control = seat.active_call("call-1", false);

    seat.kick(&[(ASKED, "published"), (LATER, "published")]);

    assert!(
        control.was_interrupted(),
        "unheard direction releases the current host-tool boundary"
    );
    let steered = seat.steered();
    assert_eq!(steered.len(), 1, "{steered:?}");
    let asked = steered[0].find(ASKED).expect("first key is delivered");
    let later = steered[0].find(LATER).expect("second key is delivered");
    assert!(asked < later, "publication order is retained: {steered:?}");

    seat.kick(&[(ASKED, "published"), (LATER, "published")]);

    assert!(
        !control.was_interrupted(),
        "an unchanged inbox does not release the call again"
    );
    assert_eq!(
        seat.steered(),
        steered,
        "one rendering enters steering once"
    );
}

#[test]
fn narration_interrupts_the_host_call_and_enters_steering_immediately() {
    let seat = Seat::new();
    let control = seat.active_call("call-1", false);
    let prod = wire::body::NarrationProdded {
        lane: crate::resident::new_uuid(),
        occupant: crate::resident::new_uuid(),
        moments: 24,
        count: 1,
    };

    seat.driver.deliver_narration(&prod);

    assert!(
        control.was_interrupted(),
        "the reminder releases the current host-tool boundary"
    );
    let steered = seat.steered();
    assert_eq!(steered.len(), 1, "{steered:?}");
    assert!(steered[0].contains("Progress update due"), "{steered:?}");
    assert!(
        seat.queued("narration").is_empty(),
        "the reminder does not wait in the aside queue"
    );
}

#[test]
fn a_peer_aside_yields_only_the_job_waits_already_blocking_its_boundary() {
    let seat = Seat::new();
    let wait = seat.active_call("wait", true);
    let shell = seat.active_call("shell", false);
    seat.driver
        .background("narration", "<system-notice>progress</system-notice>")
        .expect("an earlier aside queues");

    seat.kick_peer(LATER, "published");

    assert!(wait.was_interrupted(), "the blocking wait yields");
    assert!(
        !shell.was_interrupted(),
        "a running command remains in the foreground"
    );
    assert!(
        seat.queued("inbox").contains(LATER),
        "the peer item is already in the aside bridge"
    );
    let frames = seat.frames();
    let aside_purposes = frames
        .iter()
        .filter_map(|frame| {
            let notice = frame["message"].as_str()?.strip_prefix("/spice-aside ")?;
            serde_json::from_str::<serde_json::Value>(notice)
                .ok()?
                .get("purpose")?
                .as_str()
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        aside_purposes,
        ["narration", "inbox"],
        "the existing purpose remains queued once beside the inbox"
    );
}

#[test]
fn a_mixed_inbox_registers_the_peer_aside_before_human_steering_releases_calls() {
    let seat = Seat::new();
    let shell = seat.active_call("shell", false);

    seat.kick_human_and_peer();

    assert!(
        shell.was_interrupted(),
        "human direction releases every call"
    );
    let frames = seat.frames();
    let aside = frames
        .iter()
        .position(|frame| {
            frame["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("/spice-aside "))
        })
        .expect("the peer aside is registered");
    let steering = frames
        .iter()
        .position(|frame| frame["streamingBehavior"] == "steer")
        .expect("the human direction enters steering");
    assert!(
        aside < steering,
        "the peer aside must precede the human-forced boundary"
    );
    assert!(seat.queued("inbox").contains(LATER));
    assert!(!seat.queued("inbox").contains(ASKED));
}

#[test]
fn a_peer_aside_write_failure_does_not_release_a_wait() {
    let seat = Seat::new();
    let wait = seat.active_call("wait", true);
    seat.driver.close();

    seat.kick_peer(LATER, "published");

    assert!(
        !wait.was_interrupted(),
        "without a registered aside there is no delivery boundary to force"
    );
}

#[test]
fn an_answer_in_a_running_tool_intent_cancels_the_queued_reminder_before_the_boundary() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published")]);
    seat.kick(&[(ASKED, "reminded")]);
    assert!(
        seat.queued("inbox")
            .contains(&format!("key={ASKED} stage=reminded")),
        "the climbed ladder queues a reminder: {}",
        seat.queued("inbox")
    );

    seat.answer_in_intent(ASKED);
    assert_eq!(
        seat.queued("inbox"),
        "",
        "the answered reminder is withdrawn"
    );
}

#[test]
fn complete_visible_reply_after_a_landed_reminder_queues_correction_without_interrupting() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published")]);
    seat.kick(&[(ASKED, "reminded")]);
    let reminder = seat.queued("inbox");
    assert!(reminder.contains(&format!("[NUDGE] key={ASKED}")));
    seat.land_operator(&reminder);
    let call = seat.active_call("still-running", false);

    seat.stream_fragment("still looking");
    assert!(
        seat.queued("ack-activity").is_empty(),
        "a partial stream is not a complete response"
    );
    seat.reply("I am continuing the implementation.");

    let correction = seat.queued("ack-activity");
    assert!(correction.contains(ASKED), "{correction}");
    assert!(
        !call.was_interrupted(),
        "activity correction waits for the ordinary aside boundary"
    );
}

#[test]
fn intent_acknowledgments_prune_a_queued_activity_correction_before_catch_up() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.kick(&[(ASKED, "reminded"), (LATER, "reminded")]);
    seat.land_operator(&seat.queued("inbox"));
    seat.reply("I am still working.");
    let both = seat.queued("ack-activity");
    assert!(both.contains(ASKED) && both.contains(LATER), "{both}");

    seat.answer_in_intent(ASKED);
    let remaining = seat.queued("ack-activity");
    assert!(!remaining.contains(ASKED), "{remaining}");
    assert!(remaining.contains(LATER), "{remaining}");

    seat.reply(&format!("ACK {LATER}: handled"));
    assert_eq!(
        seat.queued("ack-activity"),
        "",
        "the final ACK cancels the replacement notice"
    );
}

#[test]
fn journal_settlement_rewrites_a_queued_activity_correction_to_remaining_keys() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.kick(&[(ASKED, "reminded"), (LATER, "reminded")]);
    seat.land_operator(&seat.queued("inbox"));
    seat.reply("I am still working.");

    seat.kick(&[(LATER, "reminded")]);

    let remaining = seat.queued("ack-activity");
    assert!(!remaining.contains(ASKED), "{remaining}");
    assert!(remaining.contains(LATER), "{remaining}");
}

#[test]
fn a_lagging_journal_never_gets_an_answered_key_named_again() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published")]);
    seat.answer_in_intent(ASKED);

    // The journal has not retired the answer when another direction arrives,
    // and then when its own ladder would have climbed.
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.kick(&[(LATER, "published")]);
    let steered = seat.steered();
    assert_eq!(steered.len(), 2, "{steered:?}");
    assert!(steered[1].contains(LATER) && !steered[1].contains(ASKED));
    assert_eq!(
        seat.queued("inbox"),
        "",
        "nothing restates the answered key"
    );
}

#[test]
fn a_ladder_climbed_after_the_answer_owes_the_key_again() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published")]);
    seat.answer_in_intent(ASKED);
    seat.kick(&[(ASKED, "published")]);
    assert_eq!(seat.queued("inbox"), "");

    // The resident judged the answer no retirement and climbed its ladder.
    seat.kick(&[(ASKED, "reminded")]);
    assert!(seat.queued("inbox").contains(&format!("key={ASKED}")));
}

#[test]
fn another_key_retiring_restates_nothing_still_owed() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.kick(&[(LATER, "published")]);
    assert_eq!(
        seat.queued("inbox"),
        "",
        "a telling that did not change since delivery is not said again"
    );
}

#[test]
fn a_compaction_replays_only_what_is_still_unanswered() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.answer_in_intent(ASKED);
    seat.hear(&serde_json::json!({
        "type": "auto_compaction_end",
        "action": "context-full",
        "result": {"summary": "so far", "firstKeptEntryId": "e1", "tokensBefore": 900000},
        "aborted": false,
        "willRetry": false,
    }));
    let replay = seat.queued("inbox");
    assert!(
        replay.contains(&format!("[POST-COMPACTION] Pending keys: {LATER}.")),
        "{replay}"
    );
    assert!(!replay.contains(ASKED), "{replay}");
}

#[test]
fn direction_unread_while_the_resident_is_unreachable_is_steered_once_when_it_answers() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published")]);
    seat.journal.unreachable();
    seat.driver.deliver_inbox();

    // The resident answers again with a later direction and a climbed ladder.
    seat.kick(&[(ASKED, "reminded"), (LATER, "published")]);
    let steered = seat.steered();
    assert_eq!(steered.len(), 2, "{steered:?}");
    assert!(steered[1].contains(LATER) && !steered[1].contains(ASKED));

    seat.journal.unreachable();
    seat.driver.deliver_inbox();
    seat.kick(&[(ASKED, "reminded"), (LATER, "published")]);
    assert_eq!(seat.steered().len(), 2, "nothing is steered twice");
}

#[test]
fn corrective_notices_arm_activity_only_after_landing() {
    for purpose in ["inbox", "hidden-ack", "misdirected-ack"] {
        let seat = Seat::new();
        seat.kick(&[(ASKED, "published")]);
        match purpose {
            "inbox" => seat.kick(&[(ASKED, "reminded")]),
            "hidden-ack" => seat.hear(&serde_json::json!({
                "type": "message_end",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "thinking", "thinking": format!("ACK {ASKED}: handled.")}],
                },
            })),
            _ => seat.reply("ACK 1kJK6N3R: handled."),
        }
        let notice = seat.queued(purpose);
        assert!(notice.contains(ASKED), "{purpose}: {notice}");
        seat.reply("Still working before the notice lands.");
        assert!(seat.queued("ack-activity").is_empty(), "{purpose}");
        seat.land_operator(&notice);
        seat.reply("Still working after the notice lands.");
        let correction = seat.queued("ack-activity");
        assert!(correction.contains(ASKED), "{purpose}: {correction}");
    }
}

#[test]
fn newly_due_keys_join_older_unanswered_activity_corrections() {
    let seat = Seat::new();
    seat.kick(&[(ASKED, "published"), (LATER, "published")]);
    seat.kick(&[(ASKED, "reminded"), (LATER, "published")]);
    seat.land_operator(&seat.queued("inbox"));
    seat.reply("Continuing before answering the first key.");
    assert!(seat.queued("ack-activity").contains(ASKED));

    seat.hear(&serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "thinking", "thinking": format!("ACK {LATER}: handled.")}],
        },
    }));
    seat.land_operator(&seat.queued("hidden-ack"));
    seat.reply("Continuing before answering either key.");
    let queued = seat.queued("ack-activity");
    assert!(queued.contains(ASKED) && queued.contains(LATER), "{queued}");

    seat.answer_in_intent(LATER);
    let remaining = seat.queued("ack-activity");
    assert!(remaining.contains(ASKED), "{remaining}");
    assert!(!remaining.contains(LATER), "{remaining}");
}

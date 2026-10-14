//! What the occupant hears about pending steering while the journal lags its
//! own answers.

use std::sync::{Arc, Mutex, PoisonError};

use super::Driver;

const ASKED: &str = "1kJK6M0V";
const LATER: &str = "1kJK6N3R";

/// The resident's inbox answer, as the test moves it; `None` while no
/// resident this seat can speak with answers.
#[derive(Clone, Default)]
struct Journal(Arc<Mutex<Option<String>>>);

impl Journal {
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
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(rendered);
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
            .last()
            .unwrap_or_default()
    }
}

#[test]
fn fresh_direction_interrupts_the_host_call_and_enters_steering_once_in_order() {
    let seat = Seat::new();
    let control = crate::resident::supervisor::terminal::Control::new();
    seat.driver
        .calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert("call-1".to_owned(), control.handle());

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

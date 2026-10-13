use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::{Driver, FirstTurn};

/// A first turn as a launch composes it: a cut row naming a read a resource
/// serves, and a placeholder no resource can.
const PROMPT: &str = "# Spice briefing\n- … 3 more: `spice session sweep /t/x.jsonl --limit 4`\n- held: `spice task show HANDLE`";
const OPENING: &[u8] = b"{\"id\":\"spice-state\",\"type\":\"get_state\"}\n";

fn driver() -> Arc<Driver<Vec<u8>>> {
    Driver::with_readers(
        Vec::new(),
        || None,
        || None,
        |_| Err("unread".to_owned()),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    )
}

fn frames(driver: &Driver<Vec<u8>>) -> Vec<serde_json::Value> {
    let input = driver.stdin.lock().expect("input locks");
    input
        .as_ref()
        .expect("input remains open")
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("a frame"))
        .collect()
}

fn answer(driver: &Arc<Driver<Vec<u8>>>, line: &[u8]) {
    let locus = listen::Locus::new(0, 0, line.len() as u64);
    let decoded = listen::decode(listen::Dialect::Omp, line, locus).expect("locus spans the line");
    let _ask = driver.observe(&decoded);
}

fn registration(success: bool) -> Vec<u8> {
    let mut answer = serde_json::json!({
        "id": listen::OMP_URI_SCHEMES_REQUEST_ID,
        "type": "response",
        "command": "set_host_uri_schemes",
        "success": success,
    });
    if !success {
        answer["error"] = "Unknown command: set_host_uri_schemes".into();
    }
    serde_json::to_vec(&answer).expect("JSON")
}

#[test]
fn only_a_seat_whose_registration_succeeded_is_told_the_scheme_and_handed_urls() {
    for success in [true, false] {
        let driver = driver();
        driver
            .open(OPENING, Some(FirstTurn::Fresh(PROMPT.to_owned())))
            .expect("opening writes");
        answer(
            &driver,
            br#"{"id":"spice-effort","type":"response","command":"set_thinking_level","success":true}"#,
        );
        assert_eq!(
            frames(&driver).len(),
            1,
            "the first turn waits for the registration answer"
        );

        answer(&driver, &registration(success));
        answer(&driver, &registration(!success));
        let frames = frames(&driver);
        assert_eq!(
            frames.len(),
            2,
            "one first turn, settled by the first answer"
        );
        assert_eq!(frames[1]["id"], vendor::omp::PROMPT_REQUEST_ID);
        let message = frames[1]["message"].as_str().expect("a prompt message");
        if success {
            assert!(message.contains("# spice:// reads"), "{message}");
            assert!(message.contains("`spice://session/sweep?"), "{message}");
            assert!(!message.contains("`spice session sweep"), "{message}");
            assert!(message.contains("`spice task show HANDLE`"), "{message}");
        } else {
            assert_eq!(message, PROMPT);
        }
    }
}

/// The lane's authority as a resume notice would render it, and how many
/// times a delivery has read it.
#[derive(Clone, Default)]
struct Authority {
    notice: Arc<Mutex<String>>,
    reads: Arc<AtomicUsize>,
}

impl Authority {
    fn set(&self, notice: &str) {
        *self.notice.lock().unwrap_or_else(PoisonError::into_inner) = notice.to_owned();
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn driver(&self) -> Arc<Driver<Vec<u8>>> {
        let authority = self.clone();
        Driver::resuming(Vec::new(), move || {
            authority.reads.fetch_add(1, Ordering::SeqCst);
            authority
                .notice
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        })
    }
}

const NEUTRAL: &str = "[$spice](</skill/SKILL.md>)";

#[test]
fn a_resumed_seat_is_told_the_authority_current_at_delivery_once() {
    let planned = "held claim: TASK-planned in phase todo; pending steering keys: 2";
    for current in [
        "held claim: none; pending steering keys: 0",
        "held claim: TASK-planned in phase review; pending steering keys: 1",
    ] {
        let authority = Authority::default();
        authority.set(planned);
        let driver = authority.driver();
        driver
            .open(OPENING, Some(FirstTurn::Resumed(NEUTRAL.to_owned())))
            .expect("opening writes");
        authority.set(current);
        assert_eq!(authority.reads(), 0, "opening reads no authority");

        answer(&driver, &registration(true));
        answer(&driver, &registration(true));
        let frames = frames(&driver);
        assert_eq!(frames.len(), 2, "one first turn for one resume");
        let message = frames[1]["message"].as_str().expect("a prompt message");
        assert!(message.starts_with(NEUTRAL), "{message}");
        assert_eq!(message.matches(current).count(), 1, "{message}");
        assert!(!message.contains(planned), "{message}");
        assert_eq!(authority.reads(), 1, "the delivery reads authority once");
    }
}

#[test]
fn notices_queued_before_a_resumed_first_turn_follow_its_current_prompt_once() {
    let authority = Authority::default();
    authority.set("held claim: TASK-done in phase review");
    let driver = authority.driver();
    driver
        .open(OPENING, Some(FirstTurn::Resumed(NEUTRAL.to_owned())))
        .expect("opening writes");
    driver
        .background("task:TASK-done", "assigned")
        .expect("a held notice queues");
    driver
        .background("task:TASK-done", "assigned")
        .expect("an unchanged notice queues");
    driver
        .background("narration", "withdrawn")
        .expect("a held notice queues");
    driver
        .background("narration", "")
        .expect("a held notice withdraws");
    assert_eq!(frames(&driver).len(), 1, "notices wait for the first turn");

    // The notice's turn completed the work before the registration answer.
    authority.set("held claim: none");
    answer(&driver, &registration(false));
    driver
        .background("renewal", "later")
        .expect("a later notice writes");
    let frames = frames(&driver);
    let ids: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame["id"].as_str())
        .collect();
    assert_eq!(ids[1], vendor::omp::PROMPT_REQUEST_ID, "{ids:?}");
    let prompt = frames[1]["message"].as_str().expect("a prompt message");
    assert!(prompt.contains("held claim: none"), "{prompt}");
    assert!(!prompt.contains("TASK-done"), "{prompt}");
    let asides: Vec<&str> = frames[2..]
        .iter()
        .map(|frame| frame["message"].as_str().expect("an aside message"))
        .collect();
    assert_eq!(asides.len(), 2, "{asides:?}");
    assert!(
        asides[0].contains(r#""purpose":"task:TASK-done""#),
        "{asides:?}"
    );
    assert!(asides[0].contains(r#""message":"assigned""#), "{asides:?}");
    assert!(asides[1].contains(r#""purpose":"renewal""#), "{asides:?}");
}

#[test]
fn launches_that_never_deliver_and_fresh_seats_read_no_resume_authority() {
    let authority = Authority::default();
    for speculative in [
        "held claim: TASK-a in phase todo",
        "held claim: TASK-b in phase todo",
    ] {
        authority.set(speculative);
        let driver = authority.driver();
        driver
            .open(OPENING, Some(FirstTurn::Resumed(NEUTRAL.to_owned())))
            .expect("opening writes");
        drop(driver);
    }
    assert_eq!(authority.reads(), 0, "an undelivered launch reads nothing");

    let fresh = authority.driver();
    fresh
        .open(OPENING, Some(FirstTurn::Fresh(PROMPT.to_owned())))
        .expect("opening writes");
    answer(&fresh, &registration(false));
    assert_eq!(frames(&fresh)[1]["message"], PROMPT);
    assert_eq!(
        authority.reads(),
        0,
        "a fresh seat carries its own briefing"
    );

    authority.set("held claim: none; pending steering keys: 0");
    let delivered = authority.driver();
    delivered
        .open(OPENING, Some(FirstTurn::Resumed(NEUTRAL.to_owned())))
        .expect("opening writes");
    answer(&delivered, &registration(false));
    let message = frames(&delivered)[1]["message"]
        .as_str()
        .expect("a prompt message")
        .to_owned();
    assert_eq!(message.matches("held claim").count(), 1, "{message}");
    assert!(message.contains("held claim: none"), "{message}");
    assert_eq!(authority.reads(), 1);
}

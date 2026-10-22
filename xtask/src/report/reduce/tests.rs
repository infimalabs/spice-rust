use serde_json::{Value, json};

use super::*;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const PARENT: &str = "dTfxt0T1";
const CORRECTION: &str = "dTfxt0C1";

fn uuid(index: u8) -> String {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{index:02x}")
}

fn hour(hours: u32) -> String {
    format!("2026-01-0{}T{:02}:00:00.000Z", 1 + hours / 24, hours % 24)
}

/// Current lifecycle boundary of a lane with nothing accounted.
fn proven() -> Value {
    json!({"proven": {"heads": []}})
}

fn fact(seq: u64, at: &str, cause: Option<u64>, body: Value) -> StreamFact {
    let mut envelope = json!({
        "v": 1, "seq": seq, "at": at, "actor": format!("lane:{LANE}"), "origin": "task:fixture",
        "fact": body,
    });
    if let Some(cause) = cause {
        envelope["cause"] = json!(cause);
    }
    serde_json::from_value(envelope).expect("fixture fact decodes")
}

fn added(seq: u64, cause: Option<u64>, task: &str) -> StreamFact {
    fact(
        seq,
        &hour(0),
        cause,
        json!({"kind": "task::added", "task": task, "title": "fixture", "project": "report.fixture",
            "pitch": "A report needs a fixture.", "depends": [], "flow": ["todo", "review"],
            "phase": "todo", "revision": 1,
            "creation": {"kind": "unavailable", "reason": "operator-actor"}}),
    )
}

fn claim(seq: u64, at: u32, task: &str, occupant: u8, until: u32) -> StreamFact {
    fact(
        seq,
        &hour(at),
        None,
        json!({"kind": "claim::won", "task": task, "actor": format!("lane:{LANE}"), "lane": LANE,
            "occupant": uuid(occupant), "epoch": EPOCH, "until": hour(until),
            "cell": {"phase": "todo", "project": "report.fixture"}, "source": proven()}),
    )
}

fn done(seq: u64, at: u32, task: &str) -> StreamFact {
    fact(
        seq,
        &hour(at),
        None,
        json!({"kind": "task::done", "task": task, "validation": "fixture", "source": proven()}),
    )
}

fn phased(seq: u64, task: &str) -> StreamFact {
    fact(
        seq,
        &hour(1),
        None,
        json!({"kind": "task::phased", "task": task, "phase": "review"}),
    )
}

fn edited(seq: u64, cause: u64, task: &str) -> StreamFact {
    fact(
        seq,
        &hour(1),
        Some(cause),
        json!({"kind": "task::edited", "task": task, "revision": 2}),
    )
}

fn history(facts: Vec<StreamFact>) -> History {
    let mut history = History::default();
    for fact in facts {
        history.push(fact).expect("fixture history folds");
    }
    history
}

#[test]
fn a_finding_stays_terminal_until_its_parent_statement_is_causally_revised() {
    let finding = json!({"kind": "task::reviewed", "task": PARENT,
        "reviewer": format!("lane:{LANE}"), "verdict": "finding", "status": "pending",
        "finding": "fixture", "source": proven()});
    let mut history = history(vec![
        added(1, None, PARENT),
        done(2, 1, PARENT),
        phased(3, PARENT),
        fact(4, &hour(1), None, finding),
        added(5, Some(4), CORRECTION),
        edited(6, 5, PARENT),
    ]);
    let task = &history.tasks[PARENT];
    assert_eq!(
        task.closure.as_ref().expect("finding closes").kind,
        "task::reviewed"
    );
    assert_eq!(
        task.reviews.last().expect("review recorded").verdict,
        "finding"
    );
    assert_eq!(task.reviews[0].correction_births, [CORRECTION]);

    history.push(edited(7, 4, PARENT)).expect("revision folds");
    let task = &history.tasks[PARENT];
    assert!(
        task.closure.is_none(),
        "a revised finding is no longer a landing"
    );
    assert!(task.reviews[0].parent_statement_changed);
}

#[test]
fn only_the_flow_ending_phase_closes_and_a_later_claim_reopens() {
    let mut history = history(vec![added(1, None, PARENT), done(2, 1, PARENT)]);
    assert!(
        history.tasks[PARENT].closure.is_none(),
        "todo is not the last phase"
    );

    for next in [phased(3, PARENT), done(4, 2, PARENT)] {
        history.push(next).expect("phase folds");
    }
    let closure = history.tasks[PARENT]
        .closure
        .as_ref()
        .expect("flow-ending done closes");
    assert_eq!(closure.kind, "task::done");
    assert_eq!(closure.phase, "review");

    history
        .push(claim(5, 3, PARENT, 1, 4))
        .expect("claim folds");
    assert!(
        history.tasks[PARENT].closure.is_none(),
        "a new claim reopens the task"
    );
}

#[test]
fn claim_episodes_censor_late_admissions_and_never_guess_unwitnessed_holds() {
    let [finished, expired, held, stolen, late] =
        ["dTfxt0d1", "dTfxt0x1", "dTfxt0h1", "dTfxt0s1", "dTfxt0l1"];
    let mut facts = (1..)
        .zip([finished, expired, held, stolen, late])
        .map(|(seq, task)| added(seq, None, task))
        .collect::<Vec<_>>();
    let foreign_lapse = json!({"kind": "claim::lapsed", "task": stolen,
        "occupant": uuid(9), "expiry": hour(6)});
    facts.extend([
        claim(10, 1, finished, 1, 3),
        done(11, 2, finished),
        claim(12, 3, expired, 2, 4),
        claim(13, 4, held, 3, 29),
        claim(14, 5, stolen, 4, 6),
        fact(15, &hour(5), None, foreign_lapse),
        claim(16, 30, late, 5, 31),
    ]);
    let History {
        tasks, lifecycle, ..
    } = history(facts);
    let manifest = serde_json::from_value(json!({
        "version": 1, "upper": 16, "horizon_at": hour(48),
        "pin": {"schema": 1, "decoder": "0".repeat(64), "epoch": EPOCH}, "fact_count": 12, "pages": 1,
        "facts_sha256": "f", "oracle_sha256": "o", "oracle_acquisition": "a",
        "consumer_sha256": "c", "command_record_sha256": "r",
    }))
    .expect("fixture manifest decodes");
    let study = Study {
        version: 1,
        manifest,
        selected: Vec::new(),
        tasks,
        selected_fingerprint: String::new(),
        routes: BTreeMap::new(),
        projects: BTreeMap::new(),
        earliest_delivery: hour(0).parse().expect("instant"),
        latest_delivery: hour(48).parse().expect("instant"),
        lifecycle,
    };

    let outcomes = super::super::episodes::measure(&study).expect("episodes measure");
    let outcome = |task: &str| {
        let episode = outcomes
            .episodes
            .iter()
            .find(|episode| episode.task == task)
            .expect("episode exists");
        (
            episode.outcome.as_str(),
            episode.eligible,
            episode.right_censored,
        )
    };
    assert_eq!(outcome(finished), ("phase completed", true, false));
    assert_eq!(outcome(expired), ("missing history", true, false));
    assert_eq!(outcome(held), ("still held at 24h", true, false));
    assert_eq!(outcome(stolen), ("missing history", true, false));
    assert_eq!(outcome(late), ("unsettled", false, true));
    assert_eq!((outcomes.eligible, outcomes.right_censored), (4, 1));
    assert_eq!(
        outcomes.held_minutes,
        [60.0],
        "only a witnessed end within follow-up is held time"
    );
}

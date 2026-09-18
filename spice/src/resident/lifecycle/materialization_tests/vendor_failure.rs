//! An exit whose only turn ended on the vendor's error.

use std::fs;
use std::path::PathBuf;

use lane::capture::CaptureExit;
use lane::occupant::Seats;

use super::reaped_seat::{
    fact_kinds, fixture_root, occupied_seed, occupied_seed_with_session, residency_row,
    runtime_paths, session,
};
use super::*;
use crate::resident::runtime::reap::account_exit;
use crate::resident::supervisor::test_support::record_exited_for_tests;
use crate::resident::supervisor::{Adoption, adopt};
use crate::resident::{CommandResult, RuntimePaths, lifecycle};

const REFUSAL: &str =
    "Refusal (reasoning_extraction): This request was blocked by our content filtering policy.";

const DECLARATION: &[u8] =
    br#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}
"#;
const PROMPT: &[u8] = br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Resume the claim."}]}}
"#;
const REFUSED_RESULT: &[u8] = br#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["Refusal (reasoning_extraction): This request was blocked by our content filtering policy."]}
"#;
const RATE_LIMIT_ALLOWED: &[u8] = br#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour"}}
"#;
const FINISHED_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"Finished the claim."}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}
"#;
const FINISHED_RESULT: &[u8] =
    br#"{"type":"result","subtype":"success","is_error":false,"result":"Finished the claim."}
"#;

#[test]
fn an_exit_whose_only_turn_the_vendor_refused_holds_the_lane_before_any_reseat() {
    let fixture = VendorExitFixture::new("refused", &[PROMPT, REFUSED_RESULT, RATE_LIMIT_ALLOWED]);
    let config = lane::Config::default();
    let mut lifecycle = lifecycle::Runtime::new(&config);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture
        .exit(&mut lifecycle, &config, &mut diagnostic)
        .expect("the refused exit is accounted");

    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneHeld, wire::FactKind::LaneExited],
        "the hold is journaled before the exit vacates the lane"
    );
    let held = fixture.held_fact();
    assert_eq!(held.lane, lane());
    assert_eq!(
        held.evidence,
        format!(
            "occupant {} ended on a vendor error: {REFUSAL}",
            fixture.occupant
        ),
        "the journal names the occupant and the vendor's own words"
    );
    assert_eq!(
        lifecycle.held_until(lane()),
        Some(held.until),
        "the runtime holds the lane on the journaled deadline"
    );
    assert_eq!(
        residency_row(&fixture.bus).occupant,
        None,
        "the exit is still accounted"
    );
    let projection = fixture.projection();
    let snapshot = fixture.bus.task_snapshot().expect("vacant snapshot reads");
    let holds = replay_holds(fixture.bus.fact_snapshot().expect("facts read").facts());
    assert!(!holds.seats(lane()), "the replayed journal holds the lane");
    assert!(
        project_targets(
            &snapshot,
            &projection,
            instant(50),
            &holds,
            &board::NextConfig::default(),
        )
        .expect("lifecycle targets project")
        .is_empty(),
        "a held lane is no target for a re-seat"
    );
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lifecycle held lane={} until={} evidence={}\n",
            lane(),
            held.until,
            held.evidence
        )
    );
    fixture.finish();
}

#[test]
fn an_exit_that_ended_on_prose_is_one_young_death_and_no_hold() {
    let fixture = VendorExitFixture::new(
        "finished",
        &[PROMPT, FINISHED_PROSE, FINISHED_RESULT, RATE_LIMIT_ALLOWED],
    );
    let config = lane::Config::default();
    let mut lifecycle = lifecycle::Runtime::new(&config);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture
        .exit(&mut lifecycle, &config, &mut diagnostic)
        .expect("the finished exit is accounted");

    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited],
        "an exit on the occupant's own prose journals no hold"
    );
    assert_eq!(lifecycle.held_until(lane()), None);
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
    fixture.finish();
}

#[test]
fn a_vendor_failure_holds_for_the_configured_hold_from_the_injected_clock() {
    let occupant = uuid(3);
    let mut journal = Journal::in_memory().expect("held fixture journal opens");
    journal
        .apply(occupied_seed(occupant))
        .expect("held fixture seeds");
    let bus = Bus::start(journal).expect("held fixture bus starts");
    let config = lane::Config::default();
    let mut lifecycle = lifecycle::Runtime::new(&config);
    let before = fact_kinds(&bus);
    let mut diagnostic = Vec::new();

    lifecycle
        .observe_vendor_failure_at(
            &bus,
            epoch(),
            lane(),
            occupant,
            &format!("  {REFUSAL}\n"),
            &config,
            instant(10),
            &mut diagnostic,
        )
        .expect("the failure holds the lane");

    let until = Instant::new(
        instant(10)
            .get()
            .checked_add(jiff::SignedDuration::from_secs(30 * 60))
            .expect("the hold deadline is representable"),
    )
    .expect("the hold deadline is an instant");
    assert_eq!(fact_kinds(&bus)[before.len()..], [wire::FactKind::LaneHeld]);
    let held = held_fact(&bus);
    assert_eq!(held.lane, lane());
    assert_eq!(
        held.until, until,
        "the hold runs the configured 30m from now"
    );
    assert_eq!(
        held.evidence,
        format!("occupant {occupant} ended on a vendor error: {REFUSAL}"),
        "the vendor text is trimmed and kept whole"
    );
    assert_eq!(lifecycle.held_until(lane()), Some(until));
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lifecycle held lane={} until={until} evidence={}\n",
            lane(),
            held.evidence
        )
    );

    let mut again = Vec::new();
    lifecycle
        .observe_vendor_failure_at(
            &bus,
            epoch(),
            lane(),
            occupant,
            "a second failure while held",
            &config,
            instant(20),
            &mut again,
        )
        .expect("a failure while held is absorbed");
    assert_eq!(
        fact_kinds(&bus)[before.len()..],
        [wire::FactKind::LaneHeld],
        "a lane already held takes no second hold"
    );
    assert_eq!(lifecycle.held_until(lane()), Some(until));
    assert!(again.is_empty(), "{}", String::from_utf8_lossy(&again));
    bus.verify().expect("the held authority verifies");
    bus.shutdown().expect("held fixture stops");
}

#[test]
fn verbose_vendor_text_is_cut_at_a_character_boundary_in_the_evidence() {
    let occupant = uuid(3);
    let mut journal = Journal::in_memory().expect("verbose fixture journal opens");
    journal
        .apply(occupied_seed(occupant))
        .expect("verbose fixture seeds");
    let bus = Bus::start(journal).expect("verbose fixture bus starts");
    let config = lane::Config::default();
    let mut lifecycle = lifecycle::Runtime::new(&config);
    let verbose = "é".repeat(600);
    let mut diagnostic = Vec::new();

    lifecycle
        .observe_vendor_failure_at(
            &bus,
            epoch(),
            lane(),
            occupant,
            &verbose,
            &config,
            instant(10),
            &mut diagnostic,
        )
        .expect("the verbose failure holds the lane");

    let held = held_fact(&bus);
    let prefix = format!("occupant {occupant} ended on a vendor error: ");
    let kept = held
        .evidence
        .strip_prefix(&prefix)
        .expect("the evidence opens on the occupant");
    let kept = kept
        .strip_suffix('…')
        .expect("cut evidence ends on the mark");
    assert_eq!(kept.len(), 512, "the cut lands on the byte budget");
    assert_eq!(
        kept.chars().count(),
        256,
        "the cut lands on a character boundary"
    );
    assert!(kept.chars().all(|character| character == 'é'));
    bus.shutdown().expect("verbose fixture stops");
}

fn held_fact(bus: &Bus) -> body::LaneHeld {
    bus.fact_snapshot()
        .expect("facts read")
        .facts()
        .iter()
        .rev()
        .find_map(|fact| fact.fact.read::<body::LaneHeld>())
        .expect("the hold fact is journaled")
}

/// A seated occupant whose declared session names the transcript it wrote.
struct VendorExitFixture {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    occupant: CanonicalUuid,
}

impl VendorExitFixture {
    fn new(tag: &str, records: &[&[u8]]) -> Self {
        let root = fixture_root(&format!("vendor-failure-{tag}"));
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        let root = fs::canonicalize(root).expect("fixture root canonicalizes");
        let paths = runtime_paths(&root);
        let occupant = uuid(3);
        let tape = tape::Tape::open(paths.tape.join(lane().to_string()).join("claude"))
            .expect("the lane tape opens");
        let declared = tape
            .tee(DECLARATION)
            .expect("the session declaration is captured");
        for record in records {
            tape.tee(record).expect("the transcript record is captured");
        }
        let mut session = session(&root, occupant);
        session.locus = wire::TapeLocus {
            member: declared.member,
            offset: declared.offset,
            length: declared.length,
        };
        let mut journal = Journal::in_memory().expect("vendor exit fixture journal opens");
        journal
            .apply(occupied_seed_with_session(
                occupant,
                &root.to_string_lossy(),
                Some(session.clone()),
            ))
            .expect("vendor exit fixture seeds");
        let bus = Bus::start(journal).expect("vendor exit fixture bus starts");
        record_exited_for_tests(&paths, lane(), occupant, session)
            .expect("the supervisor exit record writes");
        Self {
            root,
            paths,
            bus,
            occupant,
        }
    }

    fn exit(
        &self,
        lifecycle: &mut lifecycle::Runtime,
        config: &lane::Config,
        diagnostic: &mut Vec<u8>,
    ) -> CommandResult<()> {
        let row = residency_row(&self.bus);
        let Some(Adoption::Exited(launch)) =
            adopt(&self.paths, &row).expect("published state adopts")
        else {
            panic!("the published exit was not adoptable");
        };
        let mut seats = Seats::new();
        seats
            .seat_detached(lane(), self.occupant, launch.process)
            .expect("published occupant seats");
        let outcome = account_exit(
            &self.bus,
            &self.paths,
            epoch(),
            &mut seats,
            lifecycle,
            config,
            CaptureExit {
                lane: lane(),
                occupant: self.occupant,
            },
            diagnostic,
        );
        assert!(!seats.occupied(lane()));
        outcome
    }

    fn held_fact(&self) -> body::LaneHeld {
        held_fact(&self.bus)
    }

    fn projection(&self) -> Projection {
        let mut projection = Projection::new();
        projection
            .advance(
                &self.bus.task_snapshot().expect("snapshot reads"),
                &projects(),
            )
            .expect("projection advances");
        projection
    }

    fn finish(self) {
        self.bus.verify().expect("the accounted authority verifies");
        self.bus.shutdown().expect("vendor exit fixture stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

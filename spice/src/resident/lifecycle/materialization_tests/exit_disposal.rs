//! Steering the ended occupant answered in its final transcript.

use std::fs;
use std::path::PathBuf;

use lane::capture::CaptureExit;
use lane::occupant::Seats;
use wire::Disposition;

use super::reaped_seat::{fact_kinds, fixture_root, occupied_seed_at, residency_row, session};
use super::*;
use crate::resident::supervisor::test_support::record_exited_for_tests;
use crate::resident::supervisor::{Adoption, adopt};
use crate::resident::{RuntimePaths, lane_surface, steering};

/// The key `operator_direction_write` addresses to the fixture lane.
const DIRECTION_KEY: &str = "1kKRRsDS";

const NARRATION: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"Reading the request before answering it."}]}}
"#;
const TURN_ENDING_ACK: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"ACK 1kKRRsDS: Resumed and finished the request."}]}}
"#;
const TURN_ENDING_NACK: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"NACK 1kKRRsDS: The request names a lane this worktree does not hold."}]}}
"#;
const TURN_ENDING_SILENCE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"Finished without answering the direction."}]}}
"#;
/// The ack command captured as the only record of its assistant message, with
/// the header spelled inside the tool input and nowhere else on the tape.
const SAME_MESSAGE_TOOL_ACK: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_fixture_ack","name":"Bash","input":{"command":"spice ack 1kKRRsDS","description":"ACK 1kKRRsDS: Resumed and finished the request."}}]}}
"#;
/// The operator's own message carrying the header, captured as a user record
/// newer than anything the occupant said.
const OPERATOR_ACK: &[u8] = br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"ACK 1kKRRsDS: Resumed and finished the request."}]}}
"#;

#[test]
fn a_turn_ending_ack_in_the_final_transcript_retires_the_key_when_the_exit_is_accounted() {
    let fixture = ExitFixture::new("acked", &[NARRATION, TURN_ENDING_ACK]);
    let before = fact_kinds(&fixture.bus);
    assert!(
        fixture.inbox().contains(DIRECTION_KEY),
        "the direction is delivered before the occupant ends"
    );
    let mut diagnostic = Vec::new();

    fixture.exit(&mut diagnostic);

    assert_eq!(residency_row(&fixture.bus).occupant, None);
    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited, wire::FactKind::SteerAcked],
        "the exit journals the acknowledgment the transcript carries"
    );
    assert_eq!(fixture.direction_head().disposition, Disposition::Acked);
    assert!(
        fixture.pending_keys().is_empty(),
        "nothing waits for a successor to run the ack command"
    );
    let locus = fixture.acked_locus();
    assert_eq!(locus.source, Some(fixture.tape_source()));
    assert_eq!(
        fixture.tape_record(&locus),
        TURN_ENDING_ACK,
        "the journaled locus cites the turn-ending header itself"
    );
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lane {} steering {DIRECTION_KEY} acked at exit from its transcript\n",
            lane()
        )
    );
    assert!(
        !fixture.inbox().contains(DIRECTION_KEY),
        "the lane inbox is refolded to match the journal"
    );
    assert!(
        !fixture.projection().has_operator_steering(lane()),
        "no successor is woken for direction the transcript already answered"
    );
    fixture.finish();
}

#[test]
fn a_turn_ending_nack_in_the_final_transcript_declines_the_key_when_the_exit_is_accounted() {
    let fixture = ExitFixture::new("declined", &[NARRATION, TURN_ENDING_NACK]);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture.exit(&mut diagnostic);

    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited, wire::FactKind::SteerDeclined]
    );
    assert_eq!(fixture.direction_head().disposition, Disposition::Declined);
    assert!(fixture.pending_keys().is_empty());
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lane {} steering {DIRECTION_KEY} declined at exit from its transcript\n",
            lane()
        )
    );
    fixture.finish();
}

#[test]
fn a_key_the_final_transcript_does_not_answer_stays_pending_and_still_wakes_a_successor() {
    let fixture = ExitFixture::new("pending", &[NARRATION, TURN_ENDING_SILENCE]);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture.exit(&mut diagnostic);

    assert_eq!(residency_row(&fixture.bus).occupant, None);
    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited],
        "an unanswered key is neither acknowledged nor recorded as missed"
    );
    assert_eq!(fixture.direction_head().disposition, Disposition::Live);
    assert_eq!(fixture.pending_keys(), [DIRECTION_KEY]);
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lane {} steering {DIRECTION_KEY} left pending at exit: \
             steer::transcript-evidence-absent: no assistant transcript record recognizes this token\n",
            lane()
        )
    );
    assert!(
        fixture.inbox().contains(DIRECTION_KEY),
        "the delivered inbox still carries the unanswered direction"
    );
    let projection = fixture.projection();
    let snapshot = fixture.bus.task_snapshot().expect("vacant snapshot reads");
    let targets = project_targets(
        &snapshot,
        &projection,
        instant(50),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("vacant lane targets project");
    assert!(targets[0].pending_inbox);
    assert_eq!(
        resolve(&targets, &Backoff::default(), instant(50)),
        Resolution::Attempt {
            trigger: Trigger::PendingInbox,
            lane: lane(),
        },
        "the unanswered direction wakes a successor to answer it"
    );
    fixture.finish();
}

#[test]
fn a_header_spelled_only_inside_the_final_tool_call_is_not_evidence_and_leaves_the_key_pending() {
    let fixture = ExitFixture::new("tool-call", &[NARRATION, SAME_MESSAGE_TOOL_ACK]);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture.exit(&mut diagnostic);

    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited],
        "the ack command captured as a tool call is not the acknowledgment"
    );
    assert_eq!(fixture.direction_head().disposition, Disposition::Live);
    assert_eq!(fixture.pending_keys(), [DIRECTION_KEY]);
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lane {} steering {DIRECTION_KEY} left pending at exit: \
             steer::transcript-evidence-absent: no assistant transcript record recognizes this token\n",
            lane()
        )
    );
    assert!(
        fixture.inbox().contains(DIRECTION_KEY),
        "the delivered inbox still carries the direction the tool input alone named"
    );
    fixture.finish();
}

#[test]
fn a_header_in_an_operator_record_is_not_the_occupant_answering_and_leaves_the_key_pending() {
    let fixture = ExitFixture::new("operator-record", &[NARRATION, OPERATOR_ACK]);
    let before = fact_kinds(&fixture.bus);
    let mut diagnostic = Vec::new();

    fixture.exit(&mut diagnostic);

    assert_eq!(
        fact_kinds(&fixture.bus)[before.len()..],
        [wire::FactKind::LaneExited],
        "a user record carrying the header is not the occupant answering"
    );
    assert_eq!(fixture.direction_head().disposition, Disposition::Live);
    assert_eq!(fixture.pending_keys(), [DIRECTION_KEY]);
    assert_eq!(
        String::from_utf8(diagnostic).expect("diagnostic is UTF-8"),
        format!(
            "lane {} steering {DIRECTION_KEY} left pending at exit: \
             steer::transcript-evidence-absent: no assistant transcript record recognizes this token\n",
            lane()
        )
    );
    assert!(
        fixture.inbox().contains(DIRECTION_KEY),
        "the delivered inbox still carries the direction only the operator record named"
    );
    fixture.finish();
}

/// One occupied lane whose supervisor published its exit and whose transcript
/// tape ends on the given records.
struct ExitFixture {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    occupant: CanonicalUuid,
}

impl ExitFixture {
    fn new(tag: &str, records: &[&[u8]]) -> Self {
        let root = fixture_root(&format!("exit-disposal-{tag}"));
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        let root = fs::canonicalize(root).expect("fixture root canonicalizes");
        let paths = RuntimePaths::discover(&root).expect("fixture runtime paths resolve");
        let occupant = uuid(3);
        let mut journal = Journal::in_memory().expect("exit fixture journal opens");
        journal
            .apply(occupied_seed_at(occupant, &root.to_string_lossy()))
            .expect("exit fixture seeds");
        let bus = Bus::start(journal).expect("exit fixture bus starts");
        committed(
            bus.commit(operator_direction_write())
                .expect("operator direction commits"),
        );
        let residency = bus
            .residency_head()
            .expect("residency reads")
            .expect("residency exists");
        steering::publish_inbox(&bus, &paths, residency.body(), lane())
            .expect("the direction is delivered to the lane inbox");
        record_exited_for_tests(&paths, lane(), occupant, session(&root, occupant))
            .expect("the supervisor exit record writes");
        let tape = tape::Tape::open(paths.tape.join(lane().to_string()).join("claude"))
            .expect("the lane tape opens");
        for record in records {
            tape.tee(record).expect("the transcript record is captured");
        }
        Self {
            root,
            paths,
            bus,
            occupant,
        }
    }

    fn exit(&self, diagnostic: &mut Vec<u8>) {
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
        lane_surface::occupant_exited(
            &self.bus,
            &self.paths,
            epoch(),
            &mut seats,
            CaptureExit {
                lane: lane(),
                occupant: self.occupant,
            },
            diagnostic,
        )
        .expect("the exit is accounted");
        assert!(!seats.occupied(lane()));
    }

    fn direction_head(&self) -> wire::SteeringHead {
        self.bus
            .read(|read| read.steering_head(uuid(11)))
            .expect("steering index reads")
            .expect("the operator direction is indexed")
            .body()
            .clone()
    }

    fn pending_keys(&self) -> Vec<String> {
        self.bus
            .read(|read| crate::steering_read::pending(read, Some(lane())))
            .expect("pending steering reads")
            .into_iter()
            .map(|head| head.body().key.clone())
            .collect()
    }

    fn acked_locus(&self) -> body::TranscriptLocus {
        let snapshot = self.bus.fact_snapshot().expect("facts read");
        let acked = snapshot
            .facts()
            .iter()
            .rev()
            .find_map(|fact| fact.fact.read::<body::SteerAcked>())
            .expect("the acknowledgment fact is journaled");
        assert_eq!(acked.key, DIRECTION_KEY);
        assert_eq!(acked.lane, lane());
        assert_eq!(acked.epoch, epoch());
        acked.locus
    }

    fn tape(&self) -> tape::Tape {
        tape::Tape::open_existing(self.paths.tape.join(lane().to_string()).join("claude"))
            .expect("the lane tape reopens")
    }

    fn tape_source(&self) -> CanonicalUuid {
        self.tape()
            .source_id()
            .expect("the tape names its source")
            .as_str()
            .parse()
            .expect("the tape source is canonical")
    }

    fn tape_record(&self, locus: &body::TranscriptLocus) -> Vec<u8> {
        self.tape()
            .read(tape::Locus::new(locus.member, locus.offset, locus.length))
            .expect("the cited record reads")
    }

    fn inbox(&self) -> String {
        fs::read_to_string(steer::inbox::path(&self.paths.inbox, lane()))
            .expect("the lane inbox reads")
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
        self.bus.shutdown().expect("exit fixture stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

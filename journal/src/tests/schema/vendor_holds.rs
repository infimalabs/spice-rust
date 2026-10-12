//! Retained vendor-failure holds that omit the input they refused cross with the
//! input their strictly preceding history proves, or refuse without mutation.

use super::*;
use crate::tests::steering_index::{live_head, origin, retire_write, send_write_for, sent};
use crate::{FactCause, JournalWrite};
use wire::body::{
    LaneHeld, LaneLaunchIdentity, LaneRenewalChanged, LaneSessionDeclared, LaneStarted, UpSeated,
    VendorFailureInput,
};
use wire::{Disposition, LaneRenewalRecord, LaneRenewalState, Producer, RenewalIdentity};

const TEAM: &str = "018f82d4-6f59-7d06-92e3-bce913116401";
const LANE_A: &str = "018f82d4-6f59-7d06-92e3-bce913116402";
const LANE_B: &str = "018f82d4-6f59-7d06-92e3-bce913116403";
const FIRST_A: &str = "018f82d4-6f59-7d06-92e3-bce913116404";
const SECOND_A: &str = "018f82d4-6f59-7d06-92e3-bce913116405";
const FIRST_B: &str = "018f82d4-6f59-7d06-92e3-bce913116406";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116407";
const VENDOR_FAILURE_INPUT_RELEASE: &str = "3903331e5";

fn directive(index: u8) -> CanonicalUuid {
    uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131165{index:02}"))
}

fn commit_write(journal: &mut Journal, write: JournalWrite) -> i64 {
    let applied = journal.apply(write).expect("fixture write commits");
    i64::try_from(applied.last().get()).expect("fixture seq fits")
}

fn commit(journal: &mut Journal, producer: Producer, body: impl wire::Body) -> i64 {
    let mut write = JournalWrite::new(Actor::Spice, origin(), instant(), producer);
    write
        .fact(body, FactCause::Root)
        .expect("fixture fact builds");
    commit_write(journal, write)
}

fn up(journal: &mut Journal) {
    commit(
        journal,
        Producer::CommandDoor,
        UpSeated { epoch: uuid(EPOCH) },
    );
}

fn declare(journal: &mut Journal, lane: &str, occupant: &str, session: &str) {
    let declared = LaneSessionDeclared {
        lane: uuid(lane),
        occupant: uuid(occupant),
        worktree: "/fixture/worktree".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: LaneLaunchIdentity::new("sonnet", "high"),
        epoch: uuid(EPOCH),
        stream: "session".to_owned(),
        source: uuid(EPOCH),
        identity: session.to_owned(),
        member: 1,
        offset: 10,
        length: 20,
    };
    commit(journal, Producer::ResidentRecognition, declared);
}

fn start(journal: &mut Journal, lane: &str, occupant: &str) {
    let started = LaneStarted {
        lane: uuid(lane),
        occupant: uuid(occupant),
        epoch: uuid(EPOCH),
        group: 7,
        fresh_session: Some(false),
    };
    commit(journal, Producer::ResidentRecognition, started);
}

fn seat(journal: &mut Journal, lane: &str, occupant: &str, session: &str) {
    declare(journal, lane, occupant, session);
    start(journal, lane, occupant);
}

fn renew(journal: &mut Journal, lane: &str, revision: u64) {
    let record = LaneRenewalRecord {
        team: uuid(TEAM),
        lane: uuid(lane),
        revision,
        state: LaneRenewalState::Requested {
            predecessor: RenewalIdentity {
                occupant: uuid(FIRST_A),
                session: "session-predecessor".to_owned(),
                thread: "thread-predecessor".to_owned(),
            },
            team_slot: 0,
        },
    };
    let changed = LaneRenewalChanged {
        team: uuid(TEAM),
        lane: uuid(lane),
        renewal: Some(record),
    };
    commit(journal, Producer::ResidentRecognition, changed);
}

fn hold(journal: &mut Journal, lane: &str, occupant: &str, input: VendorFailureInput) -> i64 {
    let held = LaneHeld {
        lane: uuid(lane),
        evidence: format!("occupant {occupant} ended on a vendor error: refused"),
        until: instant(),
        vendor_failure: Some(input),
    };
    commit(journal, Producer::BusTimer, held)
}

fn input(
    session: &str,
    directives: Vec<CanonicalUuid>,
    renewal: Option<u64>,
) -> VendorFailureInput {
    VendorFailureInput {
        session: Some(session.to_owned()),
        directives,
        renewal,
    }
}

fn direct(journal: &mut Journal, lane: &str, index: u8) -> CanonicalUuid {
    let directive = directive(index);
    let write = send_write_for(directive, &format!("dKey{index}"), uuid(lane));
    commit_write(journal, write);
    directive
}

/// A live directive the resident authored rather than the operator.
fn direct_as_spice(journal: &mut Journal, lane: &str, index: u8) {
    let mut write = JournalWrite::new(Actor::Spice, origin(), instant(), Producer::CommandDoor);
    let mut body = sent(directive(index), &format!("dKey{index}"));
    body.lane = uuid(lane);
    let key = body.key.clone();
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("resident directive builds");
    write.steering_head(pending, move |seq| live_head(&body, key, seq));
    commit_write(journal, write);
}

fn retire(journal: &mut Journal, directive: CanonicalUuid, disposition: Disposition) {
    let observed = journal
        .read(|read| read.steering_head(directive))
        .expect("directive reads")
        .expect("directive exists");
    commit_write(journal, retire_write(&observed, disposition));
}

/// Three vendor holds on two lanes, each followed by changes that would give
/// it a different input if it were read from later authority.
fn vendor_history(journal: &mut Journal) {
    up(journal);
    let canceled = direct(journal, LANE_A, 1);
    let first_pending = direct(journal, LANE_A, 2);
    let other_lane = direct(journal, LANE_B, 3);
    direct_as_spice(journal, LANE_A, 4);
    retire(journal, canceled, Disposition::Canceled);
    seat(journal, LANE_A, FIRST_A, "session-a1");
    renew(journal, LANE_A, 7);
    hold(
        journal,
        LANE_A,
        FIRST_A,
        input("session-a1", vec![first_pending], Some(7)),
    );

    retire(journal, first_pending, Disposition::Acked);
    let second_pending = direct(journal, LANE_A, 5);
    up(journal);
    seat(journal, LANE_A, SECOND_A, "session-a2");
    hold(
        journal,
        LANE_A,
        SECOND_A,
        input("session-a2", vec![second_pending], None),
    );

    seat(journal, LANE_B, FIRST_B, "session-b1");
    hold(
        journal,
        LANE_B,
        FIRST_B,
        input("session-b1", vec![other_lane], None),
    );

    renew(journal, LANE_A, 9);
    retire(journal, other_lane, Disposition::Acked);
    direct(journal, LANE_B, 6);
    seat(journal, LANE_B, FIRST_A, "session-later");
}

/// Return every vendor hold to the evidence-only shape and the file to its
/// predecessor schema.
fn retain_evidence_only(journal: &Journal) {
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.vendor_failure') AS BLOB) WHERE kind = ?1",
            [FactKind::LaneHeld.as_str()],
        )
        .expect("vendor inputs are removed");
    restore_predecessor(journal, SCHEMA_VERSION - 1);
}

fn table(journal: &Journal, query: &str) -> Vec<(String, Vec<u8>)> {
    let mut statement = journal.connection.prepare(query).expect("table reads");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("table rows read")
        .collect::<Result<_, _>>()
        .expect("table rows decode")
}

fn facts(journal: &Journal) -> Vec<(String, Vec<u8>)> {
    table(
        journal,
        "SELECT CAST(seq AS TEXT), envelope FROM fact_envelopes ORDER BY seq",
    )
}

fn steering_heads(journal: &Journal) -> Vec<(String, Vec<u8>)> {
    table(
        journal,
        "SELECT directive, body FROM steering_heads ORDER BY directive",
    )
}

#[test]
fn writable_open_gives_each_vendor_hold_the_input_its_preceding_history_proves() {
    let scratch = Scratch::new();
    let path = scratch.path().join("vendor-holds.sqlite3");
    let mut journal = authority_file_fixture(&path);
    vendor_history(&mut journal);
    let canonical = facts(&journal);
    let heads = steering_heads(&journal);
    retain_evidence_only(&journal);
    drop(journal);

    let journal = Journal::open(&path).expect("retained vendor holds cross on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((SCHEMA_VERSION - 1, SCHEMA_VERSION)),
    );
    assert_eq!(
        facts(&journal),
        canonical,
        "each hold regains exactly the input recorded at its own position, and every \
         other fact keeps its bytes",
    );
    assert_eq!(steering_heads(&journal), heads);
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(facts(&reopened), canonical);
}

type Underivable = fn(&mut Journal) -> i64;

fn hold_on_unseated_lane(journal: &mut Journal) -> i64 {
    hold(
        journal,
        LANE_B,
        FIRST_B,
        input("session-b1", Vec::new(), None),
    )
}

fn hold_naming_a_replaced_occupant(journal: &mut Journal) -> i64 {
    seat(journal, LANE_A, SECOND_A, "session-a2");
    hold(
        journal,
        LANE_A,
        FIRST_A,
        input("session-a1", Vec::new(), None),
    )
}

fn hold_after_an_undeclared_seat(journal: &mut Journal) -> i64 {
    start(journal, LANE_B, FIRST_B);
    hold(
        journal,
        LANE_B,
        FIRST_B,
        input("session-b1", Vec::new(), None),
    )
}

#[test]
fn an_underivable_vendor_hold_refuses_the_crossing_without_mutation() {
    let cases: [(&str, Underivable); 3] = [
        ("unseated lane", hold_on_unseated_lane),
        ("replaced occupant", hold_naming_a_replaced_occupant),
        ("undeclared seat", hold_after_an_undeclared_seat),
    ];
    for (case, underivable) in cases {
        let scratch = Scratch::new();
        let path = scratch.path().join("vendor-holds.sqlite3");
        let mut journal = authority_file_fixture(&path);
        up(&mut journal);
        seat(&mut journal, LANE_A, FIRST_A, "session-a1");
        hold(
            &mut journal,
            LANE_A,
            FIRST_A,
            input("session-a1", Vec::new(), None),
        );
        let refused = underivable(&mut journal).to_string();
        retain_evidence_only(&journal);
        drop(journal);
        let before = fs::read(&path).expect("authority bytes read");

        let error = Journal::open(&path).expect_err("an underivable hold refuses");
        assert!(
            matches!(
                &error,
                OpenError::Migration(MigrationError::RetainedRow { table, row, release, .. })
                    if *table == "fact_envelopes"
                        && *row == refused
                        && *release == VENDOR_FAILURE_INPUT_RELEASE
            ),
            "{case}: {error}"
        );
        assert_eq!(
            fs::read(&path).expect("authority bytes reread"),
            before,
            "{case}"
        );
    }
}

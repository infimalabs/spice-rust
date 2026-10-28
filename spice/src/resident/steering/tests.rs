use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use journal::{FactCause, JournalWrite};
use serde_json::Value;
use wire::body::{SteerSent, SteeringStage, TranscriptLocus};
use wire::principal::PrincipalRole;
use wire::{Actor, Disposition, Origin, Producer, SteeringHead};

use super::{admit_index, due_stage, mint_key_at, missing_stages, sent_wakes};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
const DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce913116399";
const OTHER_DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce913116398";
const KEY: &str = "1kJK6N3R";
const OTHER_KEY: &str = "1kJK6N3S";

#[test]
fn only_operator_role_holders_and_exact_withheld_packet_controls_wake_a_lane() {
    let lane = DIRECTIVE.parse().expect("fixture lane");
    let locus = TranscriptLocus {
        source: Some(OTHER_DIRECTIVE.parse().expect("fixture source")),
        member: 3,
        offset: 5,
        length: 8,
        event: 13,
        line: 21,
        column: 34,
    };
    let control = wire::packet_withheld_control(&locus).expect("sourced packet control");
    let operator = crate::test_principal::bootstrap_holding(&[PrincipalRole::Operator]);
    let revoked = crate::test_principal::bootstrap_holding(&[]);

    let human = Actor::Human(operator.bootstrap().expect("bootstrap principal"));
    let stranger = Actor::Human(DIRECTIVE.parse().expect("unborn principal"));
    assert!(sent_wakes(&operator, &Actor::Operator, None));
    assert!(sent_wakes(&operator, &human, None));
    assert!(!sent_wakes(&operator, &stranger, None));
    assert!(!sent_wakes(&revoked, &Actor::Operator, None));
    assert!(!sent_wakes(&revoked, &human, None));
    assert!(sent_wakes(&revoked, &Actor::Spice, Some(&control)));
    assert!(!sent_wakes(
        &operator,
        &Actor::Spice,
        Some("review-feedback:PLAN-dV0000000:1")
    ));
    assert!(!sent_wakes(&operator, &Actor::Lane(lane), Some(&control)));
    assert!(wire::parse_packet_withheld_control(&control).is_some());
    assert!(wire::parse_packet_withheld_control(&format!("{control}:extra")).is_none());
}

#[test]
fn steering_key_clock_mints_current_microseconds_and_advances_one_collision() {
    let now = 1_788_810_000_000_000_i64;
    let first = mint_key_at(None, now).expect("current key");
    let collision = mint_key_at(Some(now), now).expect("collision key");
    let future = mint_key_at(Some(now + 10), now).expect("indexed future key");
    assert_eq!(first.as_str().len(), steer::Key::WIDTH);
    assert_eq!(first.minted_micros(), Some(now));
    assert_eq!(collision.minted_micros(), Some(now + 1));
    assert_eq!(future.minted_micros(), Some(now + 11));
}

#[test]
fn elapsed_time_from_publication_selects_each_named_stage_at_its_exact_horizon() {
    let published: wire::Instant = "2026-08-01T06:00:00.000Z"
        .parse()
        .expect("publication instant");
    let elapsed = |seconds: i64| {
        wire::Instant::new(
            published
                .get()
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .expect("fixture instant"),
        )
        .expect("fixture instant")
    };
    let config = steer::Config::default();
    let mut standing = SteeringStage::Published;
    for (rung, due) in config.resend().iter().zip([
        SteeringStage::Reminded,
        SteeringStage::Escalated,
        SteeringStage::Overdue,
    ]) {
        let horizon = i64::try_from(rung.get().as_secs()).expect("a horizon in range");
        assert_eq!(
            due_stage(published, elapsed(horizon - 1), &config).expect("representable horizon"),
            standing,
            "the second before the {due:?} horizon still stands at {standing:?}"
        );
        assert_eq!(
            due_stage(published, elapsed(horizon), &config).expect("representable horizon"),
            due,
            "{due:?} is due exactly at its horizon"
        );
        standing = due;
    }
}

#[test]
fn repeated_delivery_appends_only_the_missing_ordered_stages() {
    use wire::body::SteeringStage::{Escalated, Overdue, Published, Reminded};

    assert_eq!(missing_stages(Published, Published), []);
    assert_eq!(missing_stages(Published, Reminded), [Reminded]);
    assert_eq!(missing_stages(Published, Escalated), [Reminded, Escalated]);
    assert_eq!(
        missing_stages(Published, Overdue),
        [Reminded, Escalated, Overdue]
    );
    assert_eq!(missing_stages(Reminded, Reminded), []);
    assert_eq!(missing_stages(Reminded, Escalated), [Escalated]);
    assert_eq!(missing_stages(Reminded, Overdue), [Escalated, Overdue]);
    assert_eq!(missing_stages(Escalated, Escalated), []);
    assert_eq!(missing_stages(Escalated, Overdue), [Overdue]);
    assert_eq!(missing_stages(Overdue, Overdue), []);
}

#[test]
fn cursor_current_startup_does_not_rebuild_the_transactional_fold() {
    let scratch = Scratch::new();
    let path = scratch.path().join("current.sqlite3");
    let mut journal = journal::Journal::open(&path).expect("steering authority opens");
    journal
        .apply(send_write(DIRECTIVE, KEY))
        .expect("steering head applies");
    drop(journal);

    let connection = rusqlite::Connection::open(&path).expect("current fixture opens");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_redundant_steering_rebuild \
             BEFORE DELETE ON steering_heads BEGIN \
                 SELECT RAISE(ABORT, 'current steering fold was rebuilt'); \
             END;",
        )
        .expect("guard current fold against reconstruction");
    drop(connection);

    let mut journal = journal::Journal::open(&path).expect("current authority reopens");
    assert!(
        journal
            .steering_index_current()
            .expect("current cursor reads")
    );
    admit_index(&mut journal).expect("current transactional fold is admitted directly");
    let pending = journal
        .read(|read| crate::steering_read::pending(read, None))
        .expect("current inbox reads");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body().disposition, Disposition::Live);
}

#[test]
fn startup_replay_repairs_lagging_predicate_corruption() {
    let scratch = Scratch::new();
    let path = scratch.path().join("journal.sqlite3");
    let mut journal = journal::Journal::open(&path).expect("steering authority opens");
    journal
        .apply(send_write(DIRECTIVE, KEY))
        .expect("steering head applies");
    let facts = journal
        .fact_snapshot()
        .expect("pre-corruption facts")
        .facts()
        .len();
    drop(journal);

    let connection = rusqlite::Connection::open(&path).expect("corruption fixture opens");
    connection
        .execute("UPDATE steering_heads SET disposition = 'canceled'", [])
        .expect("corrupt indexed predicate");
    connection
        .execute("UPDATE steering_index SET indexed_seq = NULL", [])
        .expect("make the corrupt fold lag authority");
    drop(connection);

    let mut journal = journal::Journal::open(&path).expect("corrupt authority reopens");
    assert!(
        !journal
            .steering_index_current()
            .expect("lagging corruption reads"),
        "only a lagging fold needs replay"
    );
    journal
        .verify()
        .expect_err("verification detects predicate corruption");

    admit_index(&mut journal).expect("one canonical startup replay repairs fold");
    journal.verify().expect("repaired authority verifies");
    let pending = journal
        .read(|read| crate::steering_read::pending(read, None))
        .expect("repaired inbox reads");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body().disposition, Disposition::Live);
    assert_eq!(
        journal
            .fact_snapshot()
            .expect("post-repair facts")
            .facts()
            .len(),
        facts,
        "index replay must not append immutable history"
    );
}

#[test]
fn contradictory_startup_replay_refuses_without_fold_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("contradictory.sqlite3");
    let mut journal = journal::Journal::open(&path).expect("steering authority opens");
    journal
        .apply(send_write(DIRECTIVE, KEY))
        .expect("first steering head applies");
    journal
        .apply(send_write(OTHER_DIRECTIVE, OTHER_KEY))
        .expect("second steering head applies");
    drop(journal);

    let connection = rusqlite::Connection::open(&path).expect("contradiction fixture opens");
    let envelope = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [i64::from(52 * 52 * 52 + 1)],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("read second steering fact");
    let mut document: Value = serde_json::from_slice(&envelope).expect("decode steering envelope");
    document["fact"]["key"] = Value::String(KEY.to_owned());
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            rusqlite::params![
                serde_json::to_vec(&document).expect("encode contradictory envelope"),
                i64::from(52 * 52 * 52 + 1),
            ],
        )
        .expect("store contradictory steering history");
    connection
        .execute("UPDATE steering_index SET indexed_seq = NULL", [])
        .expect("make the contradictory fold lag authority");
    let before = stored_fold(&connection);
    drop(connection);

    let mut journal = journal::Journal::open(&path).expect("contradictory authority reopens");
    admit_index(&mut journal).expect_err("duplicate canonical key refuses startup replay");
    drop(journal);

    let connection = rusqlite::Connection::open(&path).expect("refused authority reopens");
    assert_eq!(
        stored_fold(&connection),
        before,
        "replay must refuse before mutating any fold row"
    );
}

fn send_write(directive: &str, key: &str) -> JournalWrite {
    let body = SteerSent {
        from: None,
        room: None,
        directive: uuid(directive),
        fanout: None,
        key: key.to_owned(),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        text: "repair the startup fold".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2030-04-12T19:44:09.271Z".parse().expect("expiry"),
    };
    let mut write = JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("sent fact builds");
    write.steering_head(pending, move |seq| SteeringHead {
        from: None,
        room: body.room.clone(),
        directive: body.directive,
        fanout: body.fanout,
        key: body.key,
        lane: body.lane,
        text: body.text,
        control: body.control,
        attachments: body.attachments,
        published: instant(),
        expires: body.expires,
        stage: SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin: origin(),
        created: seq,
        minted_micros: None,
    });
    write
}

fn stored_fold(connection: &rusqlite::Connection) -> StoredFold {
    let cursor = connection
        .query_row("SELECT indexed_seq FROM steering_index", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .expect("read steering cursor");
    let mut statement = connection
        .prepare(
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_micros, \
             room, head_seq, body FROM steering_heads ORDER BY directive",
        )
        .expect("prepare steering fold read");
    let heads = statement
        .query_map([], |row| {
            Ok(StoredHead {
                directive: row.get(0)?,
                key: row.get(1)?,
                lane: row.get(2)?,
                disposition: row.get(3)?,
                created: row.get(4)?,
                minted_micros: row.get(5)?,
                room: row.get(6)?,
                position: row.get(7)?,
                body: row.get(8)?,
            })
        })
        .expect("read steering fold")
        .collect::<Result<Vec<_>, _>>()
        .expect("decode steering fold");
    StoredFold { cursor, heads }
}

#[derive(Debug, Eq, PartialEq)]
struct StoredFold {
    cursor: Option<i64>,
    heads: Vec<StoredHead>,
}

#[derive(Debug, Eq, PartialEq)]
struct StoredHead {
    directive: String,
    key: String,
    lane: String,
    disposition: String,
    created: i64,
    minted_micros: Option<i64>,
    room: Option<String>,
    position: i64,
    body: Vec<u8>,
}

fn uuid(text: &str) -> wire::CanonicalUuid {
    text.parse().expect("canonical fixture uuid")
}

fn origin() -> Origin {
    "resident:018f82d4-6f59-7d06-92e3-bce913116304"
        .parse()
        .expect("fixture origin")
}

fn instant() -> wire::Instant {
    "2030-04-12T18:44:09.271Z".parse().expect("fixture instant")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-steering-admission-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create steering scratch");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove steering scratch");
    }
}

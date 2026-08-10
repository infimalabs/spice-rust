//! Bounded steering authority, atomic publication, and schema-7 admission.

use super::*;
use crate::{CommitError, FactCause, JournalGuard, JournalHead, JournalWrite};
use wire::body::{SteerCanceled, SteerSent, SteeringStage, TaskNoted};
use wire::{Disposition, Producer, SteeringHead};

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const TARGET: &str = "018f82d4-6f59-7d06-92e3-bce913116399";
const TARGET_KEY: &str = "1kJK6N3R";

#[test]
fn steering_fact_and_head_publish_atomically_and_key_collision_rolls_back() {
    let mut journal = Journal::in_memory().expect("steering journal");

    let mut unindexed =
        JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    unindexed
        .fact(sent(uuid(TARGET), TARGET_KEY), FactCause::Root)
        .expect("sent fact builds");
    let error = journal
        .apply(unindexed)
        .expect_err("raw steering fact refuses");
    assert!(matches!(error, CommitError::Refused(_)));
    assert!(
        journal
            .fact_snapshot()
            .expect("unchanged journal")
            .facts()
            .is_empty()
    );

    let mut mismatched =
        JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let body = sent(uuid(TARGET), TARGET_KEY);
    let pending = mismatched
        .fact(body.clone(), FactCause::Root)
        .expect("sent fact builds");
    mismatched.steering_head(pending, move |seq| {
        live_head(
            &sent(
                uuid("018f82d4-6f59-7d06-92e3-bce913116398"),
                "different-key",
            ),
            "different-key".to_owned(),
            seq,
        )
    });
    let error = journal
        .apply(mismatched)
        .expect_err("a head for another directive refuses");
    assert!(matches!(error, CommitError::Refused(_)));
    assert!(
        journal
            .fact_snapshot()
            .expect("mismatch rolls back")
            .facts()
            .is_empty()
    );

    let applied = journal
        .apply(send_write(uuid(TARGET), TARGET_KEY))
        .expect("indexed publication applies");
    let stored = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("indexed key reads")
        .expect("target head exists");
    assert_eq!(stored.position(), applied.last());
    assert_eq!(stored.body().created, applied.first());
    assert!(
        journal
            .steering_index_current()
            .expect("index horizon reads")
    );

    let before = journal.fact_snapshot().expect("pre-collision history");
    let collision = journal
        .apply(send_write(
            uuid("018f82d4-6f59-7d06-92e3-bce913116398"),
            TARGET_KEY,
        ))
        .expect_err("a second directive cannot claim the indexed key");
    assert!(matches!(collision, CommitError::Fault(_)));
    assert_eq!(
        journal.fact_snapshot().expect("post-collision history"),
        before,
        "the fact and unique indexed head must roll back together"
    );
}

#[test]
fn stale_indexed_admission_loses_the_race_without_mutation() {
    let mut journal = Journal::in_memory().expect("steering journal");
    journal
        .apply(send_write(uuid(TARGET), TARGET_KEY))
        .expect("publication applies");
    let observed = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("head reads")
        .expect("head exists");

    let first = cancel_write(&observed);
    let second = cancel_write(&observed);
    let retired = journal.apply(first).expect("first cancellation wins");
    let before_loser = journal.fact_snapshot().expect("winner history");
    let error = journal.apply(second).expect_err("stale cancellation loses");
    assert!(matches!(error, CommitError::Refused(_)));
    assert_eq!(
        journal.fact_snapshot().expect("loser history"),
        before_loser
    );

    let head = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("retired head reads")
        .expect("retired head exists");
    assert_eq!(head.position(), retired.last());
    assert_eq!(head.body().disposition, Disposition::Canceled);
}

#[test]
fn indexed_read_work_is_constant_as_retired_history_grows() {
    let mut journal = Journal::in_memory().expect("steering journal");
    journal
        .apply(send_write(uuid(TARGET), TARGET_KEY))
        .expect("target publication applies");

    let before_key = measured_key(&journal);
    let before_lane = measured_lane(&journal);
    let before_all = measured_all_lanes(&journal);
    for offset in 0_u64..128 {
        let directive = uuid(&format!("00000000-0000-4000-8000-{offset:012x}"));
        let key = format!("retired-{offset:03}");
        journal
            .apply(send_write(directive, &key))
            .expect("retired publication applies");
        let head = journal
            .read(|read| read.steering_key(&key))
            .expect("retired head reads")
            .expect("retired head exists");
        journal
            .apply(cancel_write(&head))
            .expect("retirement applies");
    }

    assert_eq!(measured_key(&journal), before_key);
    assert_eq!(measured_lane(&journal), before_lane);
    assert_eq!(measured_all_lanes(&journal), before_all);
    assert_eq!(before_key.queries, 2, "horizon plus one keyed lookup");
    assert_eq!(before_lane.queries, 2, "horizon plus one live-lane lookup");
    let plan = journal
        .connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT directive, steer_key, lane, disposition, created_seq, \
             minted_millis, head_seq, body FROM steering_heads \
             WHERE disposition = 'live' ORDER BY created_seq",
        )
        .expect("all-lanes plan prepares")
        .query_map([], |row| row.get::<_, String>(3))
        .expect("all-lanes plan reads")
        .collect::<Result<Vec<_>, _>>()
        .expect("all-lanes plan decodes")
        .join("\n");
    assert!(
        plan.contains("steering_heads_disposition_created"),
        "all-lanes inbox must use its disposition/creation index: {plan}"
    );
}

#[test]
fn schema_six_migration_requires_one_explicit_rebuild_then_stays_current() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-six.sqlite3");
    let old = authority_file_fixture(&path);
    drop_steering_index(&old.connection);
    old.connection
        .execute_batch(
            "UPDATE journal_schema SET version = 6 WHERE singleton = 1;
             PRAGMA user_version = 6;",
        )
        .expect("downgrade schema-six fixture");
    drop(old);

    let mut migrated = Journal::open(&path).expect("schema six migrates");
    assert_eq!(
        migrated.migration().map(|step| (step.from(), step.to())),
        Some((6, SCHEMA_VERSION))
    );
    assert!(!migrated.steering_index_current().expect("lag reads"));
    assert!(
        migrated
            .read(|read| read.pending_steering(None).map(drop))
            .expect_err("interactive indexed read refuses lag")
            .to_string()
            .contains("disagrees with journal cursor")
    );

    let snapshot = migrated.fact_snapshot().expect("singular rebuild snapshot");
    migrated
        .rebuild_steering_index(snapshot.horizon(), &[], &[])
        .expect("validated empty steering replay publishes");
    assert!(
        migrated
            .steering_index_current()
            .expect("rebuilt horizon reads")
    );

    let mut ordinary =
        JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    ordinary
        .fact(
            TaskNoted {
                task: "1kKRRsDP".parse().expect("fixture task"),
                note: "normal write after steering rebuild".to_owned(),
            },
            FactCause::Root,
        )
        .expect("ordinary fact builds");
    migrated
        .apply(ordinary)
        .expect("later write needs no replay");
    assert!(
        migrated
            .steering_index_current()
            .expect("advanced horizon reads")
    );
}

fn measured_key(journal: &Journal) -> crate::guard::SteeringReadMetrics {
    crate::guard::reset_steering_read_metrics();
    let head = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("target key reads")
        .expect("target head exists");
    assert_eq!(head.body().directive, uuid(TARGET));
    crate::guard::steering_read_metrics()
}

fn measured_lane(journal: &Journal) -> crate::guard::SteeringReadMetrics {
    crate::guard::reset_steering_read_metrics();
    let pending = journal
        .read(|read| read.pending_steering(Some(lane())))
        .expect("live lane reads");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body().directive, uuid(TARGET));
    crate::guard::steering_read_metrics()
}

fn measured_all_lanes(journal: &Journal) -> crate::guard::SteeringReadMetrics {
    crate::guard::reset_steering_read_metrics();
    let pending = journal
        .read(|read| read.pending_steering(None))
        .expect("all live lanes read");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body().directive, uuid(TARGET));
    crate::guard::steering_read_metrics()
}

fn send_write(directive: CanonicalUuid, key: &str) -> JournalWrite {
    let mut write = JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let body = sent(directive, key);
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("sent fact builds");
    let key = key.to_owned();
    write.steering_head(pending, move |seq| live_head(&body, key, seq));
    write
}

fn cancel_write(observed: &JournalHead<SteeringHead>) -> JournalWrite {
    let expected = observed.clone();
    let mut write = JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    write.guard(JournalGuard::new(move |read| {
        let current = read
            .steering_head(expected.body().directive)?
            .ok_or_else(|| crate::GuardError::refused("directive disappeared"))?;
        if current.position() != expected.position()
            || current.body().disposition != Disposition::Live
        {
            return Err(crate::GuardError::refused("directive standing moved"));
        }
        Ok(())
    }));
    let pending = write
        .fact(
            SteerCanceled {
                key: observed.body().key.clone(),
                lane: observed.body().lane,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("cancel fact builds");
    let mut next = observed.body().clone();
    next.disposition = Disposition::Canceled;
    write.steering_head(pending, move |_| next);
    write
}

fn sent(directive: CanonicalUuid, key: &str) -> SteerSent {
    SteerSent {
        directive,
        key: key.to_owned(),
        lane: lane(),
        text: "hold the indexed line".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2030-04-12T19:44:09.271Z".parse().expect("expiry"),
    }
}

fn live_head(body: &SteerSent, key: String, created: Seq) -> SteeringHead {
    SteeringHead {
        directive: body.directive,
        key,
        lane: body.lane,
        text: body.text.clone(),
        control: body.control.clone(),
        attachments: body.attachments.clone(),
        published: instant(),
        expires: body.expires,
        stage: SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin: origin(),
        created,
        minted_millis: None,
    }
}

fn lane() -> CanonicalUuid {
    uuid(LANE)
}

fn origin() -> Origin {
    "resident:018f82d4-6f59-7d06-92e3-bce913116304"
        .parse()
        .expect("fixture origin")
}

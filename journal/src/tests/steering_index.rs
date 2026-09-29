//! Bounded steering authority, atomic publication, and schema-7 admission.

use super::*;
use crate::{
    CommitError, FactCause, JournalGuard, JournalHead, JournalWrite, MAX_STEERING_PAGE_ROWS,
};
use wire::body::{
    LaneHeld, SteerCanceled, SteerEscalated, SteerSent, SteeringStage, SteeringTransition,
    TaskNoted,
};
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
fn batched_subject_head_targets_its_final_fact_exactly_once() {
    let mut journal = Journal::in_memory().expect("steering journal");
    journal
        .apply(send_write(uuid(TARGET), TARGET_KEY))
        .expect("publication applies");
    let observed = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("published head reads")
        .expect("published head exists");
    let before = journal.fact_snapshot().expect("published history");

    for (label, placement) in [
        ("missing", BatchHead::Missing),
        ("earlier-only", BatchHead::Earlier),
        ("duplicate", BatchHead::Duplicate),
        ("stray", BatchHead::Stray),
    ] {
        let error = journal
            .apply(escalation_batch(&observed, placement))
            .expect_err("invalid batch head placement refuses");
        assert!(matches!(error, CommitError::Refused(_)), "{label}: {error}");
        assert_eq!(
            journal.fact_snapshot().expect("history after refusal"),
            before,
            "{label} head placement must not mutate the journal"
        );
    }

    let applied = journal
        .apply(escalation_batch(&observed, BatchHead::Final))
        .expect("one head on the final subject fact applies");
    let current = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("escalated head reads")
        .expect("escalated head exists");
    assert_eq!(current.position(), applied.last());
    assert_eq!(current.body().stage, SteeringStage::Escalated);
    assert_eq!(
        current
            .body()
            .escalations
            .iter()
            .map(|transition| transition.stage)
            .collect::<Vec<_>>(),
        [SteeringStage::Reminded, SteeringStage::Escalated]
    );
    assert!(
        journal
            .steering_index_current()
            .expect("authority horizon reads")
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
    assert_eq!(before_key.rows, 2, "horizon plus one keyed row");
    assert_eq!(before_lane.queries, 2, "horizon plus one live-lane lookup");
    assert_eq!(before_lane.rows, 2, "horizon plus one live-lane row");
    assert!(
        before_lane.decoded_bytes > 0,
        "the one selected steering body was decoded"
    );
    let plan = journal
        .connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT directive, steer_key, lane, disposition, created_seq, \
             minted_micros, head_seq, body FROM steering_heads \
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
            .read(|read| {
                read.pending_steering(None, FactPageStart::After(None))
                    .map(drop)
            })
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

#[test]
fn steering_recovery_snapshot_excludes_unrelated_world_history() {
    let mut journal = Journal::in_memory().expect("steering journal");
    journal
        .apply(send_write(uuid(TARGET), TARGET_KEY))
        .expect("steering publication applies");
    let mut ordinary =
        JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    ordinary
        .fact(
            TaskNoted {
                task: "1kKRRsDP".parse().expect("fixture task"),
                note: "unrelated world history".to_owned(),
            },
            FactCause::Root,
        )
        .expect("ordinary fact builds");
    journal.apply(ordinary).expect("ordinary fact applies");

    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = X'00' WHERE kind = 'task::noted'",
            [],
        )
        .expect("corrupt only unrelated history");

    let snapshot = journal
        .steering_snapshot()
        .expect("steering recovery ignores unrelated bytes");
    assert_eq!(snapshot.horizon(), Seq::new(2));
    assert_eq!(snapshot.facts().len(), 1);
    assert_eq!(snapshot.facts()[0].fact.kind(), wire::FactKind::SteerSent);
    assert!(
        journal.fact_snapshot().is_err(),
        "a whole-world snapshot would have decoded the unrelated corruption"
    );
}

#[test]
fn live_steering_pages_bound_rows_decode_work_and_preserve_continuation_order() {
    let mut journal = Journal::in_memory().expect("steering journal");
    let total = MAX_STEERING_PAGE_ROWS + 3;
    for index in 0..total {
        journal
            .apply(send_write(
                indexed_uuid(index),
                &format!("bounded-{index:04}"),
            ))
            .expect("live steering publication applies");
    }
    for index in 0..total {
        journal
            .apply(send_write_for(
                indexed_uuid(total + index),
                &format!("lane-{index:04}"),
                indexed_uuid(total * 2 + index),
            ))
            .expect("another lane's live steering publication applies");
    }

    crate::guard::reset_indexed_read_metrics();
    let first = journal
        .read(|read| read.pending_steering(Some(lane()), FactPageStart::After(None)))
        .expect("first bounded page reads");
    let first_metrics = crate::guard::indexed_read_metrics();
    let overflow = first
        .overflow()
        .expect("large live set exposes continuation");
    assert_eq!(first.rows().len(), MAX_STEERING_PAGE_ROWS);
    assert_eq!(overflow.omitted, 3);
    assert_eq!(first.horizon().map(Seq::get), Some((total * 2) as u64));
    assert_eq!(
        first_metrics.queries, 2,
        "index horizon plus one page query"
    );
    assert_eq!(
        first_metrics.rows,
        u64::try_from(MAX_STEERING_PAGE_ROWS + 2).expect("fixed page metrics fit"),
        "index horizon, carried rows, and one continuation row"
    );

    crate::guard::reset_indexed_read_metrics();
    let operator = journal
        .read(|read| read.pending_operator_steering(FactPageStart::After(None)))
        .expect("operator page reads");
    assert_eq!(operator.rows(), first.rows());
    assert_eq!(
        operator.overflow().expect("all lanes overflow").omitted,
        u64::try_from(total * 2 - MAX_STEERING_PAGE_ROWS).expect("omitted count fits")
    );
    assert_eq!(crate::guard::indexed_read_metrics(), first_metrics);

    crate::guard::reset_indexed_read_metrics();
    let all_lanes = journal
        .read(|read| read.pending_steering(None, FactPageStart::After(None)))
        .expect("all-lane page reads");
    assert_eq!(all_lanes.rows(), first.rows());
    assert_eq!(all_lanes.overflow(), operator.overflow());
    assert_eq!(crate::guard::indexed_read_metrics(), first_metrics);

    let second = journal
        .read(|read| read.pending_steering(Some(lane()), FactPageStart::At(overflow.continuation)))
        .expect("continuation page reads");
    assert_eq!(second.rows().len(), 3);
    assert!(second.overflow().is_none());
    let positions = first
        .rows()
        .iter()
        .chain(second.rows())
        .map(JournalHead::position)
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), total);
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(second.rows()[0].body().created, overflow.continuation);
}

fn measured_key(journal: &Journal) -> crate::guard::IndexedReadMetrics {
    crate::guard::reset_indexed_read_metrics();
    let head = journal
        .read(|read| read.steering_key(TARGET_KEY))
        .expect("target key reads")
        .expect("target head exists");
    assert_eq!(head.body().directive, uuid(TARGET));
    crate::guard::indexed_read_metrics()
}

fn measured_lane(journal: &Journal) -> crate::guard::IndexedReadMetrics {
    crate::guard::reset_indexed_read_metrics();
    let pending = journal
        .read(|read| read.pending_steering(Some(lane()), FactPageStart::After(None)))
        .expect("live lane reads");
    assert_eq!(pending.rows().len(), 1);
    assert_eq!(pending.rows()[0].body().directive, uuid(TARGET));
    crate::guard::indexed_read_metrics()
}

fn measured_all_lanes(journal: &Journal) -> crate::guard::IndexedReadMetrics {
    crate::guard::reset_indexed_read_metrics();
    let pending = journal
        .read(|read| read.pending_steering(None, FactPageStart::After(None)))
        .expect("all live lanes read");
    assert_eq!(pending.rows().len(), 1);
    assert_eq!(pending.rows()[0].body().directive, uuid(TARGET));
    crate::guard::indexed_read_metrics()
}

#[derive(Clone, Copy)]
enum BatchHead {
    Missing,
    Earlier,
    Final,
    Duplicate,
    Stray,
}

fn escalation_batch(observed: &JournalHead<SteeringHead>, placement: BatchHead) -> JournalWrite {
    let at = instant();
    let mut write = JournalWrite::new(
        Actor::Operator,
        observed.body().origin.clone(),
        at,
        Producer::BusTimer,
    );
    let mut next = observed.body().clone();
    let mut facts = Vec::new();
    for stage in [SteeringStage::Reminded, SteeringStage::Escalated] {
        let pending = write
            .fact(
                SteerEscalated {
                    directive: observed.body().directive,
                    key: observed.body().key.clone(),
                    lane: observed.body().lane,
                    stage,
                },
                FactCause::Existing(observed.body().created),
            )
            .expect("escalation fact builds");
        facts.push(pending);
        next.stage = stage;
        next.escalations.push(SteeringTransition { stage, at });
    }
    let first = facts[0];
    let final_fact = facts[1];
    match placement {
        BatchHead::Missing => {}
        BatchHead::Earlier => write.steering_head(first, move |_| next),
        BatchHead::Final => write.steering_head(final_fact, move |_| next),
        BatchHead::Duplicate => {
            let duplicate = next.clone();
            write.steering_head(final_fact, move |_| next);
            write.steering_head(final_fact, move |_| duplicate);
        }
        BatchHead::Stray => {
            let final_head = next.clone();
            write.steering_head(final_fact, move |_| final_head);
            let ordinary = write
                .fact(
                    LaneHeld {
                        lane: lane(),
                        evidence: "stray steering head target".to_owned(),
                        until: at,
                    },
                    FactCause::Root,
                )
                .expect("ordinary fact builds");
            write.steering_head(ordinary, move |_| next);
        }
    }
    write
}

fn send_write(directive: CanonicalUuid, key: &str) -> JournalWrite {
    send_write_for(directive, key, lane())
}

fn send_write_for(directive: CanonicalUuid, key: &str, lane: CanonicalUuid) -> JournalWrite {
    let mut write = JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let mut body = sent(directive, key);
    body.lane = lane;
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("sent fact builds");
    let key = key.to_owned();
    write.steering_head(pending, move |seq| live_head(&body, key, seq));
    write
}

fn indexed_uuid(index: usize) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{:012x}", index + 0x100)
        .parse()
        .expect("indexed fixture UUID")
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
        from: None,
        directive,
        fanout: None,
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
        from: None,
        directive: body.directive,
        fanout: body.fanout,
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
        minted_micros: None,
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

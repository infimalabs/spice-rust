//! Bounded steering authority, atomic publication, and indexed admission.

use super::*;
use crate::{
    CommitError, FactCause, JournalGuard, JournalHead, JournalWrite, MAX_STEERING_PAGE_ROWS,
    RetiredSteering,
};
use wire::body::{
    LaneHeld, SteerAcked, SteerCanceled, SteerDeclined, SteerEscalated, SteerLapsed, SteerSent,
    SteeringStage, SteeringTransition, TaskNoted, TranscriptLocus,
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
    assert_eq!(snapshot.horizon(), Seq::new(crate::INITIAL_SEQUENCE + 1));
    assert_eq!(snapshot.facts().len(), 1);
    assert_eq!(snapshot.facts()[0].fact.kind(), wire::FactKind::SteerSent);
    assert!(
        journal.fact_snapshot().is_err(),
        "a whole-world snapshot would have decoded the unrelated corruption"
    );
}

#[test]
fn a_rebuilt_steering_index_restores_typed_rooms() {
    let mut journal = Journal::in_memory().expect("steering journal");
    let [room, _] = rooms();
    let heads = [
        publish_route(&mut journal, 1, lane(), Some(&room)),
        publish_route(&mut journal, 2, lane(), None),
    ];
    journal
        .connection
        .execute("UPDATE steering_heads SET room = NULL", [])
        .expect("forget typed rooms");

    let cursor = journal.read(|read| read.cursor()).expect("cursor reads");
    journal
        .rebuild_steering_index(cursor, &heads, &[])
        .expect("steering index rebuilds");
    assert_eq!(typed_rooms(&journal), [Some(room.to_string()), None]);
    journal.verify().expect("rebuilt rooms verify");
}

#[test]
fn a_predecessor_crossing_backfills_typed_rooms_without_rewriting_authority() {
    let mut journal = Journal::in_memory().expect("steering journal");
    let [room, _] = rooms();
    publish_route(&mut journal, 1, lane(), Some(&room));
    publish_route(&mut journal, 2, lane(), None);
    let authority = |journal: &Journal| {
        journal
            .connection
            .prepare(
                "SELECT body AS bytes FROM steering_heads \
                 UNION ALL SELECT envelope FROM fact_envelopes ORDER BY bytes",
            )
            .expect("authority query prepares")
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("authority reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("authority decodes")
    };
    let before = authority(&journal);
    let predecessor = crate::migrate::STEERING_ROOM_SCHEMA - 1;
    journal
        .connection
        .execute_batch(&format!(
            "DROP TABLE release_corrections;
             DROP TABLE release_landings;
             {PREDECESSOR_STEERING_HEADS}
             UPDATE journal_schema SET version = {predecessor} WHERE singleton = 1;
             PRAGMA user_version = {predecessor};"
        ))
        .expect("restore predecessor steering heads");

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("predecessor admits")
        .expect("predecessor crosses");
    assert_eq!(
        (migration.from(), migration.to()),
        (predecessor, SCHEMA_VERSION)
    );
    assert_eq!(authority(&journal), before);
    assert_eq!(typed_rooms(&journal), [Some(room.to_string()), None]);
    journal.verify().expect("crossed rooms verify");
}

fn rooms() -> [wire::TaskStamp; 2] {
    [
        wire::TaskStamp::for_micros(1_700_000_000_000_000).expect("fixture room"),
        wire::TaskStamp::for_micros(1_700_000_000_000_001).expect("other fixture room"),
    ]
}

fn publish_route(
    journal: &mut Journal,
    index: usize,
    to: CanonicalUuid,
    room: Option<&wire::TaskStamp>,
) -> JournalHead<SteeringHead> {
    let key = format!("route-{index}");
    let mut write = JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let mut body = sent(indexed_uuid(index), &key);
    body.lane = to;
    body.room = room.cloned();
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("routed fact builds");
    let head_key = key.clone();
    write.steering_head(pending, move |seq| live_head(&body, head_key, seq));
    journal.apply(write).expect("room route commits");
    journal
        .read(|read| read.steering_key(&key))
        .expect("room route reads")
        .expect("room route exists")
}

fn typed_rooms(journal: &Journal) -> Vec<Option<String>> {
    journal
        .connection
        .prepare("SELECT room FROM steering_heads ORDER BY created_seq")
        .expect("typed rooms prepare")
        .query_map([], |row| row.get(0))
        .expect("typed rooms read")
        .collect::<Result<_, _>>()
        .expect("typed rooms decode")
}

#[test]
fn retired_window_keeps_each_lanes_newest_retirements_in_retirement_order() {
    const WINDOW: usize = 4;
    let other = indexed_uuid(0xff);
    let fanout = indexed_uuid(0xfe);
    let mut journal = Journal::in_memory().expect("steering journal");
    let publish = |journal: &mut Journal, write: JournalWrite, key: &str| {
        journal.apply(write).expect("publication applies");
        journal
            .read(|read| read.steering_key(key))
            .expect("published head reads")
            .expect("published head exists")
    };
    let heads = (0..6)
        .map(|index| {
            let key = format!("retired-{index}");
            publish(&mut journal, send_write(indexed_uuid(index), &key), &key)
        })
        .collect::<Vec<_>>();
    publish(&mut journal, send_write(indexed_uuid(6), "live"), "live");
    let mut fanned_write =
        JournalWrite::new(Actor::Operator, origin(), instant(), Producer::CommandDoor);
    let mut body = sent(indexed_uuid(7), "fanned");
    body.lane = other;
    body.fanout = Some(fanout);
    body.attachments = vec![image_attachment()];
    let pending = fanned_write
        .fact(body.clone(), FactCause::Root)
        .expect("fanned fact builds");
    fanned_write.steering_head(pending, move |seq| {
        live_head(&body, "fanned".to_owned(), seq)
    });
    let fanned = publish(&mut journal, fanned_write, "fanned");

    // Retire out of creation order so retirement order is observable.
    let order = [
        (3, Disposition::Acked),
        (0, Disposition::Declined),
        (5, Disposition::Lapsed),
        (1, Disposition::DeadLettered),
        (4, Disposition::Canceled),
        (2, Disposition::Acked),
    ];
    let mut retirements = Vec::new();
    for (index, disposition) in order {
        let applied = journal
            .apply(retire_write(&heads[index], disposition))
            .expect("retirement applies");
        retirements.push((
            lane(),
            heads[index].body().key.clone(),
            disposition,
            applied.last(),
        ));
    }
    let applied = journal
        .apply(retire_write(&fanned, Disposition::Canceled))
        .expect("fanned retirement applies");
    let fanned_row = (
        other,
        "fanned".to_owned(),
        Disposition::Canceled,
        applied.last(),
    );

    let summary = |rows: &[RetiredSteering]| {
        rows.iter()
            .map(|row| (row.lane, row.key.clone(), row.disposition, row.retired))
            .collect::<Vec<_>>()
    };
    let every = journal
        .read(|read| read.retired_steering(None, WINDOW))
        .expect("retired window reads");
    let mut expected = retirements
        .iter()
        .rev()
        .take(WINDOW)
        .cloned()
        .collect::<Vec<_>>();
    if other.to_string() < lane().to_string() {
        expected.insert(0, fanned_row.clone());
    } else {
        expected.push(fanned_row.clone());
    }
    assert_eq!(
        summary(&every),
        expected,
        "each lane keeps only its newest retirements, newest first, lanes in canonical order"
    );
    let projected = every
        .iter()
        .find(|row| row.key == "fanned")
        .expect("fanned row present");
    assert_eq!(projected.directive, fanned.body().directive);
    assert_eq!(projected.fanout, Some(fanout));
    assert_eq!(projected.text, fanned.body().text);
    assert_eq!(projected.attachments, fanned.body().attachments);
    assert_eq!(projected.published, fanned.body().published);

    let scoped = journal
        .read(|read| read.retired_steering(Some(other), WINDOW))
        .expect("lane-scoped retired window reads");
    assert_eq!(summary(&scoped), vec![fanned_row]);
    let whole_lane = journal
        .read(|read| read.retired_steering(Some(lane()), usize::from(u16::MAX)))
        .expect("whole-lane retired window reads");
    assert_eq!(
        summary(&whole_lane),
        retirements.iter().rev().cloned().collect::<Vec<_>>(),
        "every retired disposition decodes and the live directive never appears"
    );
    assert!(
        journal
            .read(|read| read.retired_steering(None, 0))
            .expect("empty window reads")
            .is_empty()
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
    assert_eq!(
        first.horizon().map(Seq::get),
        Some(crate::INITIAL_SEQUENCE + (total * 2) as u64 - 1)
    );
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
    assert_eq!(
        crate::guard::indexed_read_metrics(),
        crate::guard::IndexedReadMetrics {
            queries: first_metrics.queries + 1,
            ..first_metrics
        },
        "the operator page first reads the principal role heads"
    );

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
                        vendor_failure: None,
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

pub(super) fn send_write_for(
    directive: CanonicalUuid,
    key: &str,
    lane: CanonicalUuid,
) -> JournalWrite {
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
    retire_write(observed, Disposition::Canceled)
}

pub(super) fn retire_write(
    observed: &JournalHead<SteeringHead>,
    disposition: Disposition,
) -> JournalWrite {
    let expected = observed.clone();
    let (actor, producer) = match disposition {
        Disposition::Acked | Disposition::Declined => {
            (Actor::Lane(observed.body().lane), Producer::AckScanner)
        }
        Disposition::Lapsed => (Actor::Spice, Producer::BusTimer),
        Disposition::DeadLettered => (Actor::Spice, Producer::ResidentRecognition),
        Disposition::Canceled | Disposition::Live => (Actor::Operator, Producer::CommandDoor),
    };
    let mut write = JournalWrite::new(actor, origin(), instant(), producer);
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
    let head = observed.body();
    let (directive, key, lane) = (head.directive, head.key.clone(), head.lane);
    let cause = FactCause::Existing(observed.position());
    let locus = TranscriptLocus {
        source: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116306")),
        member: 1,
        offset: 0,
        length: 1,
        event: 0,
        line: 1,
        column: 1,
    };
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116305");
    let pending = match disposition {
        Disposition::Acked => write.fact(
            SteerAcked {
                directive,
                key,
                lane,
                epoch,
                locus,
            },
            cause,
        ),
        Disposition::Declined => write.fact(
            SteerDeclined {
                directive,
                key,
                lane,
                epoch,
                locus,
                reason: "cannot".to_owned(),
            },
            cause,
        ),
        Disposition::Lapsed => write.fact(
            SteerLapsed {
                key,
                lane,
                expires: head.expires,
            },
            cause,
        ),
        Disposition::DeadLettered => write.fact(
            wire::steering::SteerDeadLettered {
                directive,
                key,
                lane,
                reason: "vacant".to_owned(),
                requeue: "spice steer recover".to_owned(),
            },
            cause,
        ),
        Disposition::Canceled => write.fact(SteerCanceled { key, lane }, cause),
        Disposition::Live => panic!("retirement needs a terminal disposition"),
    }
    .expect("retirement fact builds");
    let mut next = head.clone();
    next.disposition = disposition;
    write.steering_head(pending, move |_| next);
    write
}

fn image_attachment() -> wire::Media {
    wire::Media {
        name: "diagram.png".to_owned(),
        content_type: "image/png".to_owned(),
        data_url: "data:image/png;base64,aW1hZ2U=".to_owned(),
    }
}

pub(super) fn sent(directive: CanonicalUuid, key: &str) -> SteerSent {
    SteerSent {
        from: None,
        room: None,
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

pub(super) fn live_head(body: &SteerSent, key: String, created: Seq) -> SteeringHead {
    SteeringHead {
        from: None,
        room: body.room.clone(),
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

pub(super) fn origin() -> Origin {
    "resident:018f82d4-6f59-7d06-92e3-bce913116304"
        .parse()
        .expect("fixture origin")
}

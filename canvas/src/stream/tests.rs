use super::*;
use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, Disposition, Domain, Producer, SteeringHead, StreamCursor, StreamFact, TrustHead,
    TrustState, body,
};

mod retention;

fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture uuid")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero fixture position")
}

fn accepted(actor: Actor) -> Accepted {
    Accepted {
        wire: WireVersion::CURRENT,
        actor,
        epoch: epoch(),
    }
}

fn epoch() -> wire::CanonicalUuid {
    uuid("018f82d4-6f59-7d06-92e3-bce9131163c3")
}

fn message(lane: u8, offset: u64) -> AgentMessage {
    AgentMessage {
        lane: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{lane:02x}")),
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        locus: wire::MessageLocus {
            member: 1,
            offset,
            length: 20,
            event: 0,
        },
        authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: format!("lane {lane:02x} message {offset}"),
        badges: Default::default(),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        media: Vec::new(),
    }
}

fn request(last: u8, subjects: Vec<ViewSubject>, after: Option<Seq>) -> StreamRequest {
    StreamRequest::Subscribe {
        v: WireVersion::CURRENT,
        id: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")),
        epoch: epoch(),
        subjects,
        after: after.map(|seq| StreamCursor {
            epoch: epoch(),
            seq,
        }),
    }
}

fn fixture(notes: usize) -> Bus {
    let bus = Bus::start(Journal::in_memory().expect("authority opens")).expect("bus starts");
    commit_notes(&bus, notes, true);
    bus
}

fn commit_notes(bus: &Bus, notes: usize, trust: bool) -> JournalCommit {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:STREAM-a1".parse().expect("origin"),
        "2030-04-12T18:44:09.271Z".parse().expect("instant"),
        Producer::CommandDoor,
    );
    let task: wire::TaskStamp = "1kKRRsDP".parse().expect("task stamp");
    let mut cause = FactCause::Root;
    for index in 0..notes {
        let fact = write
            .fact(
                body::TaskNoted {
                    task: task.clone(),
                    note: format!("note {index}"),
                },
                cause,
            )
            .expect("fact builds");
        cause = FactCause::Pending(fact);
    }
    if trust {
        let digest = wire::Digest::compute(Domain::ConfigExecutableSurface, b"surface");
        let fact = write
            .fact(
                body::TrustPended {
                    surface: "config".to_owned(),
                    digest: digest.clone(),
                    worktree: None,
                    previous: None,
                },
                FactCause::Root,
            )
            .expect("trust fact builds");
        write.trust_head(fact, move |_| TrustHead {
            digest,
            state: TrustState::Pended,
            worktree: None,
        });
    }
    match bus.commit(write).expect("commit answers") {
        CommitOutcome::Committed(commit) => commit,
        other => panic!("fixture commit failed: {other:?}"),
    }
}

fn commit_operator_direction(bus: &Bus) -> SteeringHead {
    let directive = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let key = "1kQnrzNZ".to_owned();
    let at = "2030-04-12T18:45:09.271Z".parse().expect("fixture instant");
    let expires = "2030-04-12T19:45:09.271Z".parse().expect("fixture expiry");
    let origin: wire::Origin = "steer:1kQnrzNZ".parse().expect("fixture origin");
    let sent = body::SteerSent {
        directive,
        fanout: None,
        key: key.clone(),
        lane,
        text: "direction survives standing changes".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires,
    };
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let pending = write
        .fact(sent.clone(), FactCause::Root)
        .expect("direction fact builds");
    let expected = sent.clone();
    write.steering_head(pending, move |created| SteeringHead {
        directive,
        fanout: None,
        key,
        lane,
        text: expected.text,
        control: None,
        attachments: Vec::new(),
        published: at,
        expires,
        stage: body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin,
        created,
        minted_micros: None,
    });
    match bus.commit(write).expect("direction commit answers") {
        CommitOutcome::Committed(_) => {}
        other => panic!("direction fixture failed: {other:?}"),
    }
    bus.read(move |read| read.steering_head(directive))
        .expect("direction head reads")
        .expect("direction head exists")
        .into_body()
}

fn cancel_operator_direction(bus: &Bus, current: &SteeringHead) {
    let mut write = JournalWrite::new(
        Actor::Operator,
        current.origin.clone(),
        "2030-04-12T18:46:09.271Z".parse().expect("fixture instant"),
        Producer::CommandDoor,
    );
    let pending = write
        .fact(
            body::SteerCanceled {
                key: current.key.clone(),
                lane: current.lane,
            },
            FactCause::Existing(current.created),
        )
        .expect("cancellation fact builds");
    let mut retired = current.clone();
    retired.disposition = Disposition::Canceled;
    write.steering_head(pending, move |_| retired);
    match bus.commit(write).expect("cancellation commit answers") {
        CommitOutcome::Committed(_) => {}
        other => panic!("cancellation fixture failed: {other:?}"),
    }
}

fn commit_trust_heads(bus: &Bus, surfaces: &[&[u8]]) -> JournalCommit {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:STREAM-a1".parse().expect("origin"),
        "2030-04-12T18:44:09.271Z".parse().expect("instant"),
        Producer::CommandDoor,
    );
    for (index, surface) in surfaces.iter().enumerate() {
        let digest = wire::Digest::compute(Domain::ConfigExecutableSurface, surface);
        let fact = write
            .fact(
                body::TrustPended {
                    surface: format!("surface-{index}"),
                    digest: digest.clone(),
                    worktree: None,
                    previous: None,
                },
                FactCause::Root,
            )
            .expect("trust fact builds");
        write.trust_head(fact, move |_| TrustHead {
            digest,
            state: TrustState::Pended,
            worktree: None,
        });
    }
    match bus.commit(write).expect("commit answers") {
        CommitOutcome::Committed(commit) => commit,
        other => panic!("fixture commit failed: {other:?}"),
    }
}

fn commit_ingestion(bus: &Bus) -> JournalCommit {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("lane:{lane}").parse().expect("lane origin"),
        "2030-04-12T18:44:10.271Z".parse().expect("instant"),
        Producer::ResidentRecognition,
    );
    write
        .fact(
            body::JournalIngested {
                lane,
                vendor: "codex".parse().expect("vendor name"),
                stream: "transcript".to_owned(),
                source: uuid("018f82d4-6f59-7d06-92e3-bce9131163e6"),
                supersedes: None,
                from: body::TranscriptPosition {
                    member: 0,
                    offset: 0,
                },
                to: body::TranscriptPosition {
                    member: 0,
                    offset: 4096,
                },
                usage: Vec::new(),
                non_usage_records: 3,
                residue_records: 0,
            },
            FactCause::Root,
        )
        .expect("ingestion fact builds");
    match bus.commit(write).expect("commit answers") {
        CommitOutcome::Committed(commit) => commit,
        other => panic!("fixture commit failed: {other:?}"),
    }
}

fn facts(frame: &StreamFrame) -> &Packed<StreamFact> {
    let StreamFrame::Facts { packed, .. } = frame else {
        panic!("expected facts, found {frame:?}");
    };
    packed
}

fn fact_sequences(delivery: &LiveDelivery) -> Vec<Seq> {
    delivery
        .frames()
        .iter()
        .map(|frame| {
            let StreamFrame::Fact { fact, .. } = frame else {
                panic!("fact subscription emitted {frame:?}")
            };
            fact.seq()
        })
        .collect()
}

#[test]
fn packed_subscription_continues_exactly_then_becomes_live() {
    let bus = fixture(5);
    let source = Source::new(bus.reader(), epoch(), 2);
    let (mut subscription, frames) = source
        .begin(
            &accepted(Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"))),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");

    assert_eq!(
        facts(&frames[0])
            .rows
            .iter()
            .map(StreamFact::seq)
            .collect::<Vec<_>>(),
        [seq(1), seq(2)]
    );
    assert_eq!(
        facts(&frames[0]).overflow.expect("overflow").continuation,
        seq(3)
    );
    assert_eq!(subscription.cursor(), Some(seq(2)));
    assert!(!subscription.is_live());

    let moved = StreamRequest::Continue {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
        epoch: epoch(),
        subject: ViewSubject::Facts,
        cursor: StreamCursor {
            epoch: epoch(),
            seq: seq(4),
        },
    };
    assert!(matches!(
        subscription.continue_page(&source, &moved),
        Err(Failure::Refusal(ref refusal))
            if refusal.reason() == "stream::continuation-moved"
    ));
    assert_eq!(subscription.cursor(), Some(seq(2)));

    for (id, cursor) in [(0xc6, 3), (0xc7, 5)] {
        let continuation = StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{id:02x}")),
            epoch: epoch(),
            subject: ViewSubject::Facts,
            cursor: StreamCursor {
                epoch: epoch(),
                seq: seq(cursor),
            },
        };
        subscription
            .continue_page(&source, &continuation)
            .expect("exact continuation answers");
    }
    assert!(subscription.is_live());
    assert_eq!(subscription.cursor(), Some(seq(6)));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn fresh_operator_fact_subscription_begins_live_without_history() {
    let bus = fixture(5);
    let source = Source::new(bus.reader(), epoch(), 2);
    let (subscription, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("operator subscription begins");

    assert!(facts(&frames[0]).rows.is_empty());
    assert!(facts(&frames[0]).overflow.is_none());
    assert_eq!(facts(&frames[0]).horizon, seq(6));
    assert_eq!(subscription.cursor(), Some(seq(6)));
    assert!(subscription.is_live());
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn fresh_operator_direction_snapshot_restores_retired_identity() {
    let bus = fixture(1);
    let current = commit_operator_direction(&bus);
    cancel_operator_direction(&bus, &current);
    let source = Source::new(bus.reader(), epoch(), 2);
    let (subscription, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Directions], None),
        )
        .expect("direction subscription begins");

    let StreamFrame::Directions {
        limit, directions, ..
    } = &frames[0]
    else {
        panic!("expected direction snapshot")
    };
    assert_eq!(*limit as usize, OPERATOR_DIRECTION_CAPACITY);
    assert_eq!(directions.len(), 1);
    let direction = &directions[0];
    assert_eq!(direction.authored, current.published);
    assert_eq!(direction.directive, current.directive);
    assert_eq!(direction.key, current.key);
    assert_eq!(direction.lane, current.lane);
    assert_eq!(direction.text, current.text);
    assert!(subscription.is_live());
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn reconnect_is_strictly_after_the_admitted_fact_cursor() {
    let bus = fixture(5);
    let source = Source::new(bus.reader(), epoch(), 10);
    let (subscription, frames) = source
        .begin(
            &accepted(Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"))),
            &request(0xc4, vec![ViewSubject::Facts], Some(seq(4))),
        )
        .expect("same-generation resume begins");

    assert_eq!(
        facts(&frames[0])
            .rows
            .iter()
            .map(StreamFact::seq)
            .collect::<Vec<_>>(),
        [seq(5), seq(6)]
    );
    assert_eq!(subscription.cursor(), Some(seq(6)));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn operator_reconnect_begins_live_instead_of_draining_missed_history() {
    let bus = fixture(5);
    let source = Source::new(bus.reader(), epoch(), 10);
    let (subscription, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], Some(seq(2))),
        )
        .expect("operator resume begins");

    assert!(facts(&frames[0]).rows.is_empty());
    assert!(facts(&frames[0]).overflow.is_none());
    assert_eq!(facts(&frames[0]).horizon, seq(6));
    assert_eq!(subscription.cursor(), Some(seq(6)));
    assert!(subscription.is_live());
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn durable_broadcast_drives_later_updates_without_refresh() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");

    let committed = commit_notes(&bus, 1, false);
    let noticed = broadcasts.recv().await.expect("commit broadcasts");
    assert_eq!(noticed, committed);
    let delivery = subscription
        .advance(&source, noticed)
        .expect("broadcast advances stream")
        .expect("new fact creates a delivery");
    assert_eq!(delivery.frames().len(), 1);
    let StreamFrame::Fact { fact, .. } = &delivery.frames()[0] else {
        panic!("expected live fact: {delivery:?}");
    };
    assert_eq!(fact.seq(), seq(3));
    assert_eq!(
        subscription.cursor(),
        Some(seq(2)),
        "materialization cannot move the resumable cursor before send"
    );
    subscription.admit(delivery);
    assert_eq!(subscription.cursor(), Some(seq(3)));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_ingestion_advances_live_cursor_without_crossing_canvas() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");
    let before = subscription.cursor().expect("operator begins live");

    let committed = commit_ingestion(&bus);
    let noticed = broadcasts.recv().await.expect("commit broadcasts");
    assert_eq!(noticed, committed);
    let delivery = subscription
        .advance(&source, noticed)
        .expect("broadcast advances stream")
        .expect("accounting receipt advances the cursor");
    assert!(
        delivery.frames().is_empty(),
        "resident-only accounting does not cross the Canvas socket"
    );
    assert_eq!(subscription.cursor(), Some(before));
    subscription.admit(delivery);
    assert_eq!(subscription.cursor(), Some(seq(committed.last())));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn one_large_commit_delivers_one_indexed_fact_quantum_per_turn() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 3);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");
    let initial = subscription.cursor().expect("operator begins live");

    let committed = commit_notes(&bus, 11, false);
    let noticed = broadcasts.recv().await.expect("commit broadcasts");
    assert_eq!(noticed, committed);
    let mut delivered = Vec::new();
    let mut first = true;
    loop {
        let before = source.read_counts();
        let delivery = if first {
            first = false;
            subscription
                .advance(&source, noticed)
                .expect("notice advances")
                .expect("notice creates delivery")
        } else {
            subscription
                .continue_live(&source)
                .expect("continuation advances")
                .expect("catch-up retains a delivery")
        };
        let after = source.read_counts();
        assert_eq!(after.1 - before.1, 1, "one fact-page query per turn");
        assert_eq!(
            after.0 - before.0,
            usize::from(delivered.is_empty()),
            "only the notification captures one head barrier"
        );
        assert!(delivery.frames().len() <= 3);
        delivered.extend(fact_sequences(&delivery));
        if delivered.len() == 3 {
            assert_eq!(
                subscription.cursor(),
                Some(initial),
                "materialized frames are not yet resumable"
            );
        }
        subscription.admit(delivery);
        if !subscription.has_continuation() {
            break;
        }
    }

    assert_eq!(
        delivered,
        (initial.get() + 1..=committed.last())
            .map(seq)
            .collect::<Vec<_>>()
    );
    assert_eq!(subscription.cursor(), Some(seq(committed.last())));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn queued_commits_coalesce_to_the_newest_barrier_without_duplicates() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 2);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");
    let initial = subscription.cursor().expect("operator begins live");

    let first = commit_notes(&bus, 5, false);
    let second = commit_notes(&bus, 4, false);
    let first_notice = broadcasts.recv().await.expect("first commit broadcasts");
    let second_notice = broadcasts.recv().await.expect("second commit broadcasts");
    assert_eq!((first_notice, second_notice), (first, second));

    let before = source.read_counts();
    let first_delivery = subscription
        .advance(&source, first_notice)
        .expect("first notice advances")
        .expect("first notice creates delivery");
    let after = source.read_counts();
    assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1));
    let mut delivered = fact_sequences(&first_delivery);
    subscription.admit(first_delivery);

    let before = source.read_counts();
    let second_delivery = subscription
        .advance(&source, second_notice)
        .expect("queued notice advances")
        .expect("queued notice creates one delivery");
    let after = source.read_counts();
    assert_eq!(
        (after.0 - before.0, after.1 - before.1),
        (0, 1),
        "the queued notice reuses the already coalesced newest head"
    );
    delivered.extend(fact_sequences(&second_delivery));
    subscription.admit(second_delivery);

    while subscription.has_continuation() {
        let delivery = subscription
            .continue_live(&source)
            .expect("continuation advances")
            .expect("catch-up retains a delivery");
        delivered.extend(fact_sequences(&delivery));
        subscription.admit(delivery);
    }
    assert_eq!(
        delivered,
        (initial.get() + 1..=second.last())
            .map(seq)
            .collect::<Vec<_>>()
    );
    assert_eq!(subscription.cursor(), Some(seq(second.last())));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn broadcast_lag_during_catch_up_resumes_after_the_last_admitted_quantum() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 3);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Facts], None),
        )
        .expect("subscription begins");
    let initial = subscription.cursor().expect("operator begins live");

    let large = commit_notes(&bus, 11, false);
    let notice = broadcasts.recv().await.expect("large commit broadcasts");
    assert_eq!(notice, large);
    let delivery = subscription
        .advance(&source, notice)
        .expect("notice advances")
        .expect("large commit creates delivery");
    subscription.admit(delivery);
    let admitted = seq(initial.get() + 3);
    assert_eq!(subscription.cursor(), Some(admitted));
    assert!(subscription.has_continuation());

    for _ in 0..70 {
        commit_notes(&bus, 1, false);
    }
    let tokio::sync::broadcast::error::RecvError::Lagged(dropped) = broadcasts
        .recv()
        .await
        .expect_err("the retained notice channel overflows")
    else {
        panic!("the retained broadcast closed instead of lagging")
    };
    let overflow = subscription.overflow(dropped).expect("lag is resumable");
    assert!(matches!(
        overflow,
        StreamFrame::Overflow { after, dropped: observed, .. }
            if after.seq == admitted && observed == dropped
    ));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[tokio::test(flavor = "current_thread")]
async fn live_trust_overflow_retains_its_snapshot_for_exact_continuation() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 2);
    let mut broadcasts = source.subscribe();
    let (mut subscription, _) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Trust], None),
        )
        .expect("subscription begins");
    assert!(subscription.is_live());

    let committed = commit_trust_heads(&bus, &[b"second", b"third"]);
    let noticed = broadcasts.recv().await.expect("commit broadcasts");
    assert_eq!(noticed, committed);
    let delivery = subscription
        .advance(&source, noticed)
        .expect("broadcast advances stream")
        .expect("new trust heads create a delivery");
    let StreamFrame::Trust { packed, .. } = &delivery.frames()[0] else {
        panic!("expected trust replacement: {delivery:?}");
    };
    let continuation = packed.overflow.expect("trust page overflows").continuation;
    let first_positions = packed
        .rows
        .iter()
        .map(|row| row.position)
        .collect::<Vec<_>>();
    assert_eq!(packed.rows.len(), 2);
    subscription.admit(delivery);
    assert!(!subscription.is_live());

    let request = StreamRequest::Continue {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
        epoch: epoch(),
        subject: ViewSubject::Trust,
        cursor: StreamCursor {
            epoch: epoch(),
            seq: continuation,
        },
    };
    let frame = subscription
        .continue_page(&source, &request)
        .expect("exact trust continuation answers");
    let StreamFrame::Trust { packed, .. } = frame else {
        panic!("expected trust continuation");
    };
    assert_eq!(packed.rows.len(), 1);
    assert!(packed.overflow.is_none());
    let positions = first_positions
        .into_iter()
        .chain(packed.rows.iter().map(|row| row.position))
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), 3);
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(subscription.is_live());
    assert_eq!(subscription.cursor(), Some(seq(committed.last())));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn lane_principal_never_admits_operator_only_subjects() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    for subject in [
        ViewSubject::Directions,
        ViewSubject::Trust,
        ViewSubject::Messages,
    ] {
        let failure = source
            .begin(
                &accepted(Actor::Lane(lane)),
                &request(0xc4, vec![subject], None),
            )
            .expect_err("lane operator-only view refuses");

        assert!(matches!(
            failure,
            Failure::Refusal(ref refusal)
                if refusal.reason() == "stream::subject-authority"
        ));
    }
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn thread_request_walks_one_indexed_barrier_and_names_continuation() {
    let bus = fixture(5);
    let source = Source::new(bus.reader(), epoch(), 2);
    let request = StreamRequest::Thread {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch: epoch(),
        cursor: StreamCursor {
            epoch: epoch(),
            seq: seq(5),
        },
    };
    let frame = source
        .thread(&accepted(Actor::Operator), &request)
        .expect("thread answers");
    let StreamFrame::Thread { packed, .. } = frame else {
        panic!("expected thread frame");
    };

    assert_eq!(
        packed.rows.iter().map(StreamFact::seq).collect::<Vec<_>>(),
        [seq(5), seq(4)]
    );
    assert_eq!(packed.overflow.expect("overflow").continuation, seq(3));
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

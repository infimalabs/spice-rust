use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const HISTORY_LENGTH: u64 = 300;
const QUIET_HISTORY_LENGTH: u64 = 40;
const FIRST_RETAINED_BOUNDARY: u64 = 80;
const SECOND_RETAINED_BOUNDARY: u64 = 220;

fn complete_lane_history(
    source: &Source,
    lane: wire::CanonicalUuid,
    initial: &[AgentMessage],
    advertised_more: &[wire::MessageHistoryCursor],
) -> Vec<AgentMessage> {
    let mut messages = initial
        .iter()
        .filter(|message| message.lane == lane)
        .cloned()
        .collect::<Vec<_>>();
    let mut before = advertised_more
        .iter()
        .find(|cursor| cursor.lane == lane)
        .map(|cursor| cursor.before);
    let mut more = before.is_some();
    while more {
        let requested_before = before.expect("paged lane has a frontier");
        let request = StreamRequest::MessageHistory {
            v: WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            epoch: epoch(),
            lane,
            before: requested_before,
        };
        let frame = source
            .message_history(&accepted(Actor::Operator), &request)
            .expect("older message page answers");
        let StreamFrame::MessageHistory {
            lane: answered,
            before: answered_before,
            messages: older,
            more: page_more,
            ..
        } = frame
        else {
            panic!("expected older message page")
        };
        assert_eq!(answered, lane);
        assert_eq!(answered_before, requested_before);
        assert!(older.iter().all(|message| message.lane == lane));
        if let Some(oldest) = older.first() {
            before = Some(Some(oldest.locus));
        }
        messages.splice(0..0, older);
        more = page_more;
    }
    messages
}

#[test]
fn terminal_revision_replaces_one_retained_message_and_broadcasts_the_change() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let ordinary = message(0xe5, 7);
    publisher
        .publish(ordinary.clone())
        .expect("ordinary prose publishes");
    let mut updates = source.subscribe_messages();

    let mut terminal = ordinary.clone();
    terminal.final_answer = true;
    terminal.automatic_speech = Some(wire::AutomaticSpeech {
        kind: wire::AutomaticSpeechKind::Final,
        text: terminal.text.clone(),
    });
    assert!(
        publisher
            .revise_final(terminal.clone())
            .expect("terminal revision publishes")
    );
    assert_eq!(
        updates.try_recv().expect("terminal revision broadcasts"),
        terminal
    );
    assert!(
        !publisher
            .revise_final(terminal.clone())
            .expect("identical terminal revision deduplicates")
    );

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(messages, &[terminal.clone()]);

    let mut conflicting = terminal;
    conflicting.text = "different authored prose".to_owned();
    assert_eq!(
        publisher.revise_final(conflicting),
        Err(MessagePublishError::IdentityConflict),
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

/// Either arrival order leaves the vendor's own record of one compaction.
///
/// The captured stream announces a compaction without a vendor timestamp and
/// the vendor's session file preserves the same compaction with one. Which
/// record reaches the feed first depends on when the session file is polled,
/// so the retained view has to settle on the preserved record either way.
#[test]
fn a_preserved_compaction_replaces_the_announcement_in_either_arrival_order() {
    for announcement_first in [true, false] {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let publisher = source.message_publisher();
        let mut announced = message(0xe5, 7);
        announced.activity = wire::AgentActivityKind::Compaction;
        announced.text = "Context compacted".to_owned();
        announced.authored_synthetic = true;
        announced.boundary = Some(wire::MessageBoundary {
            id: "cmp_03f3".to_owned(),
            preserved: false,
        });
        let mut preserved = announced.clone();
        preserved.locus = wire::MessageLocus {
            member: 0x8000_0000_0000_0001,
            offset: 44,
            length: 20,
            event: 0,
        };
        preserved.authored_synthetic = false;
        preserved.boundary = Some(wire::MessageBoundary {
            id: "cmp_03f3".to_owned(),
            preserved: true,
        });
        let order = if announcement_first {
            [announced.clone(), preserved.clone()]
        } else {
            [preserved.clone(), announced.clone()]
        };
        assert!(
            publisher
                .publish(order[0].clone())
                .expect("first publishes"),
            "the first record of a compaction enters the view",
        );
        assert_eq!(
            publisher.publish(order[1].clone()),
            Ok(announcement_first),
            "only the preserved record changes what the view holds",
        );

        let (_, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Messages], None),
            )
            .expect("message subscription begins");
        let StreamFrame::Messages { messages, .. } = &frames[0] else {
            panic!("expected retained message snapshot")
        };
        assert_eq!(
            messages,
            &[preserved],
            "one compaction is one boundary, and it is the vendor's own record",
        );
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }
}

#[test]
fn presentation_revision_moves_only_text_and_classifications_and_keeps_the_final_state() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let mut spoken = message(0xe5, 7);
    spoken.text = "> [ACK PENDING] Keep the current path.\n\nApplied.".to_owned();
    spoken.classifications = vec![wire::MessageClassification {
        start: 2,
        end: 15,
        kind: wire::MessageClassificationKind::AcknowledgementPending,
    }];
    publisher
        .publish(spoken.clone())
        .expect("spoken acknowledgement publishes");
    let mut terminal = spoken.clone();
    terminal.final_answer = true;
    assert!(
        publisher
            .revise_final(terminal.clone())
            .expect("terminal revision publishes")
    );
    let mut updates = source.subscribe_messages();

    let retired_text = "> [ACK] Keep the current path.\n\nApplied.".to_owned();
    let retired_classifications = vec![wire::MessageClassification {
        start: 2,
        end: 7,
        kind: wire::MessageClassificationKind::Acknowledgement,
    }];
    assert!(
        publisher
            .revise_presentation(
                spoken.lane,
                spoken.locus,
                retired_text.clone(),
                retired_classifications.clone()
            )
            .expect("retired presentation publishes")
    );
    let mut expected = terminal;
    expected.text = retired_text.clone();
    expected
        .classifications
        .clone_from(&retired_classifications);
    assert_eq!(
        updates.try_recv().expect("retired presentation broadcasts"),
        expected,
        "the final state the card earned survives its re-presentation"
    );
    assert!(
        !publisher
            .revise_presentation(
                spoken.lane,
                spoken.locus,
                retired_text.clone(),
                retired_classifications.clone()
            )
            .expect("identical presentation deduplicates")
    );
    let absent = message(0xe6, 1);
    assert!(
        !publisher
            .revise_presentation(
                absent.lane,
                absent.locus,
                retired_text,
                retired_classifications
            )
            .expect("a card retention does not hold is not revised")
    );
    assert!(updates.try_recv().is_err(), "nothing else broadcasts");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(messages, &[expected]);
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn retained_message_capacity_is_independent_for_each_lane() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for lane in [0xe5, 0xe6] {
        for offset in 0..MESSAGE_CAPACITY {
            publisher
                .publish(message(lane, u64::try_from(offset).expect("offset fits")))
                .expect("lane backlog publishes");
        }
    }
    publisher
        .publish(message(
            0xe5,
            u64::try_from(MESSAGE_CAPACITY).expect("capacity fits"),
        ))
        .expect("new lane activity publishes");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages {
        limit,
        messages,
        more,
        ..
    } = &frames[0]
    else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(*limit as usize, MESSAGE_CAPACITY);
    assert_eq!(messages.len(), wire::MESSAGE_FLOOR * 2);
    assert_eq!(more.len(), 2);
    for lane in [0xe5, 0xe6] {
        let lane = message(lane, 0).lane;
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.lane == lane)
                .count(),
            25
        );
        assert_eq!(
            complete_lane_history(&source, lane, messages, more).len(),
            256
        );
    }
    let busy_lane = message(0xe5, 0).lane;
    assert!(
        messages
            .iter()
            .all(|message| message.lane != busy_lane || message.locus.offset > 0),
        "the busy lane evicts only its own oldest row"
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn retained_message_capacity_starts_at_the_oldest_complete_compaction_window() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let busy_lane = message(0xe5, 0).lane;
    let quiet_lane = message(0xe6, 0).lane;
    for offset in 0..HISTORY_LENGTH {
        let mut activity = message(0xe5, offset);
        if matches!(
            offset,
            20 | FIRST_RETAINED_BOUNDARY | SECOND_RETAINED_BOUNDARY
        ) {
            activity.activity = wire::AgentActivityKind::Compaction;
        }
        publisher.publish(activity).expect("busy history publishes");
        if offset < QUIET_HISTORY_LENGTH {
            publisher
                .publish(message(0xe6, offset))
                .expect("interleaved quiet history publishes");
        }
    }

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages {
        limit,
        messages,
        more,
        ..
    } = &frames[0]
    else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(*limit as usize, MESSAGE_CAPACITY);
    let busy = complete_lane_history(&source, busy_lane, messages, more);
    assert_eq!(busy.len(), 220);
    assert_eq!(busy[0].locus.offset, FIRST_RETAINED_BOUNDARY);
    assert_eq!(busy[0].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(busy[140].locus.offset, SECOND_RETAINED_BOUNDARY);
    assert_eq!(busy[140].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(
        busy.last().expect("busy tail").locus.offset,
        HISTORY_LENGTH - 1
    );
    assert_eq!(
        complete_lane_history(&source, quiet_lane, messages, more)
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (0..QUIET_HISTORY_LENGTH).collect::<Vec<_>>(),
        "one lane's compaction floor does not disturb interleaved lanes",
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn late_session_boundaries_are_inserted_before_retention_selects_its_floor() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for offset in 0..HISTORY_LENGTH {
        if matches!(offset, FIRST_RETAINED_BOUNDARY | SECOND_RETAINED_BOUNDARY) {
            continue;
        }
        publisher
            .publish(message(0xe5, offset))
            .expect("stdout history publishes");
    }
    for offset in [FIRST_RETAINED_BOUNDARY, SECOND_RETAINED_BOUNDARY] {
        let mut boundary = message(0xe5, offset);
        boundary.activity = wire::AgentActivityKind::Compaction;
        publisher
            .publish(boundary)
            .expect("preserved-session boundary publishes late");
    }

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    let retained = complete_lane_history(&source, message(0xe5, 0).lane, messages, more);
    assert_eq!(retained[0].locus.offset, FIRST_RETAINED_BOUNDARY);
    assert_eq!(retained[0].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(retained[140].locus.offset, SECOND_RETAINED_BOUNDARY);
    assert_eq!(retained[140].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(
        retained.last().expect("retained tail").locus.offset,
        HISTORY_LENGTH - 1,
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[derive(Debug)]
struct FixtureHistory {
    messages: Vec<AgentMessage>,
    page_limit: usize,
    reads: AtomicUsize,
}

impl MessageHistoryProvider for FixtureHistory {
    fn history(
        &self,
        lane: wire::CanonicalUuid,
        before: Option<&AgentMessage>,
        limit: usize,
    ) -> Result<Option<MessageHistoryPage>, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let visible = self
            .messages
            .iter()
            .filter(|message| {
                message.lane == lane
                    && before
                        .is_none_or(|before| message.chronology_key() < before.chronology_key())
            })
            .cloned()
            .collect::<Vec<_>>();
        let start = visible.len().saturating_sub(limit.min(self.page_limit));
        Ok(Some(MessageHistoryPage {
            messages: visible[start..].to_vec(),
            more: start > 0,
        }))
    }
}

#[test]
fn cold_readers_page_a_full_cache_without_reading_the_provider() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let history = Arc::new(FixtureHistory {
        messages: Vec::new(),
        page_limit: usize::MAX,
        reads: AtomicUsize::new(0),
    });
    publisher.set_history_provider(history.clone()).unwrap();
    for lane in [0xe5, 0xe6] {
        for offset in 0..MESSAGE_CAPACITY as u64 {
            publisher.publish(message(lane, offset)).unwrap();
        }
    }
    for reader in [0xc4, 0xc5] {
        let (_, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(reader, vec![ViewSubject::Messages], None),
            )
            .unwrap();
        let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
            panic!("expected initial messages");
        };
        for lane in [0xe5, 0xe6] {
            let restored = complete_lane_history(&source, message(lane, 0).lane, messages, more);
            assert_eq!(
                restored
                    .iter()
                    .map(|message| message.locus.offset)
                    .collect::<Vec<_>>(),
                (0..MESSAGE_CAPACITY as u64).collect::<Vec<_>>(),
            );
        }
    }
    assert_eq!(history.reads.load(Ordering::Relaxed), 0);
    drop(source);
    bus.shutdown().unwrap();
}

#[test]
fn cold_history_keeps_one_page_and_loads_older_pages_without_live_updates() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let lane = message(0xe5, 0).lane;
    let startup = (0..80)
        .map(|offset| message(0xe5, offset))
        .collect::<Vec<_>>();
    for activity in &startup {
        publisher
            .publish(activity.clone())
            .expect("startup history publishes");
    }
    let mut durable = startup.clone();
    durable[54].text = "stale source projection".to_owned();
    let history = Arc::new(FixtureHistory {
        messages: durable,
        page_limit: usize::MAX,
        reads: AtomicUsize::new(0),
    });
    publisher
        .set_history_provider(history.clone())
        .expect("durable history provider installs");
    publisher
        .bound_initial_history()
        .expect("startup history is bounded");
    let mut updates = source.subscribe_messages();

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(messages.len(), wire::MESSAGE_FLOOR);
    assert_eq!(messages[0].locus.offset, 55);
    assert_eq!(
        more,
        &[wire::MessageHistoryCursor {
            lane,
            before: Some(messages[0].locus),
        }]
    );

    let restored = complete_lane_history(&source, lane, messages, more);
    assert_eq!(
        restored
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (0..80).collect::<Vec<_>>(),
    );
    assert_eq!(history.reads.load(Ordering::Relaxed), 2);
    assert_eq!(restored[54].text, startup[54].text);
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc5, vec![ViewSubject::Messages], None),
        )
        .expect("new reader subscribes after source exhaustion");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected new reader snapshot");
    };
    assert_eq!(
        complete_lane_history(&source, lane, messages, more),
        restored,
        "each reader traverses the same retained history, including current presentation",
    );
    assert_eq!(
        history.reads.load(Ordering::Relaxed),
        2,
        "a new reader does not reopen the exhausted provider",
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn short_provider_pages_continue_until_the_actual_lane_capacity_is_reached() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let lane = message(0xe5, 0).lane;
    let durable = (0..HISTORY_LENGTH)
        .map(|offset| message(0xe5, offset))
        .collect::<Vec<_>>();
    for activity in &durable {
        publisher
            .publish(activity.clone())
            .expect("startup history publishes");
    }
    let history = Arc::new(FixtureHistory {
        messages: durable,
        page_limit: 17,
        reads: AtomicUsize::new(0),
    });
    publisher
        .set_history_provider(history.clone())
        .expect("durable history provider installs");
    publisher
        .bound_initial_history()
        .expect("startup history is bounded");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    let restored = complete_lane_history(&source, lane, messages, more);
    assert_eq!(
        restored
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (HISTORY_LENGTH - MESSAGE_CAPACITY as u64..HISTORY_LENGTH).collect::<Vec<_>>(),
    );
    assert_eq!(
        history.reads.load(Ordering::Relaxed),
        14,
        "a short read with remaining capacity keeps the durable cursor open",
    );

    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn explicit_history_hint_bootstraps_a_lane_without_an_initial_card() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let lane = message(0xe5, 0).lane;
    let durable = (0..20)
        .map(|offset| message(0xe5, offset))
        .collect::<Vec<_>>();
    let history = Arc::new(FixtureHistory {
        messages: durable,
        page_limit: usize::MAX,
        reads: AtomicUsize::new(0),
    });
    publisher
        .set_history_provider(history.clone())
        .expect("durable history provider installs");
    publisher
        .set_history_open([lane])
        .expect("partial startup continuation is exposed");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert!(messages.is_empty());
    assert_eq!(more, &[wire::MessageHistoryCursor { lane, before: None }]);

    let restored = complete_lane_history(&source, lane, messages, more);
    assert_eq!(
        restored
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (0..20).collect::<Vec<_>>(),
    );
    assert_eq!(history.reads.load(Ordering::Relaxed), 1);

    drop(source);
    bus.shutdown().expect("bus shuts down");
}

/// A compaction inside the newest window never costs the lane its floor.
///
/// Trimming below a compaction drops the previous context epoch, which is
/// what an operator wants once newer messages fill the view. Here they do not:
/// the epoch that just ended is all this agent has said.
#[test]
fn cold_history_keeps_its_floor_across_a_compaction_inside_the_initial_window() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for offset in 0..40 {
        let mut activity = message(0xe6, offset);
        if matches!(offset, 30) {
            activity.activity = wire::AgentActivityKind::Compaction;
        }
        publisher
            .publish(activity)
            .expect("startup history publishes");
    }
    publisher
        .bound_initial_history()
        .expect("startup history is bounded");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc5, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(
        messages
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (14..40).collect::<Vec<_>>(),
        "the newest 25 messages stay, and the compaction stands among them",
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.activity.is_prose())
            .count(),
        wire::MESSAGE_FLOOR,
    );
    assert!(more.is_empty());

    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn a_full_page_that_exhausts_capacity_closes_after_compaction_trimming() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    let lane = message(0xe5, 0).lane;
    let mut durable = (0..300)
        .map(|offset| message(0xe5, offset))
        .collect::<Vec<_>>();
    durable[80].activity = wire::AgentActivityKind::Compaction;
    for activity in &durable[244..] {
        publisher
            .publish(activity.clone())
            .expect("retained history publishes");
    }
    let history = Arc::new(FixtureHistory {
        messages: durable,
        page_limit: usize::MAX,
        reads: AtomicUsize::new(0),
    });
    publisher
        .set_history_provider(history.clone())
        .expect("durable history provider installs");
    let mut before = message(0xe5, 244).locus;
    for page_number in 1..=4 {
        let page = publisher
            .feed
            .history(lane, Some(before))
            .expect("older message page reads")
            .expect("retained cursor is known");
        assert_eq!(page.messages.len(), MESSAGE_HISTORY_PAGE_LIMIT);
        assert_eq!(page.more, page_number < 4);
        before = page.messages[0].locus;
    }
    assert_eq!(
        history.reads.load(Ordering::Relaxed),
        4,
        "four full pages exhaust the cumulative 256-card budget",
    );
    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc5, vec![ViewSubject::Messages], None),
        )
        .expect("new reader subscribes after capacity closure");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected new reader snapshot");
    };
    let restored = complete_lane_history(&source, lane, messages, more);
    assert_eq!(
        restored
            .iter()
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (80..300).collect::<Vec<_>>(),
        "compaction trimming does not make retained pages inaccessible",
    );
    let oldest = restored.first().unwrap().locus;
    let page = publisher.feed.history(lane, Some(oldest)).unwrap().unwrap();
    assert!(page.messages.is_empty());
    assert!(!page.more);
    assert_eq!(
        history.reads.load(Ordering::Relaxed),
        4,
        "space freed by compaction does not reopen the source budget",
    );

    drop(source);
    bus.shutdown().expect("bus shuts down");
}

#[test]
fn retained_tool_status_does_not_consume_visible_lane_capacity() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for offset in 0..MESSAGE_CAPACITY {
        publisher
            .publish(message(0xe5, u64::try_from(offset).expect("offset fits")))
            .expect("visible backlog publishes");
    }
    for (offset, activity) in [
        (10_000, wire::AgentActivityKind::Tool),
        (10_001, wire::AgentActivityKind::Tool),
        (10_002, wire::AgentActivityKind::Reasoning),
    ] {
        let mut status = message(0xe5, offset);
        status.activity = activity;
        publisher
            .publish(status)
            .expect("presence status publishes");
    }

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.activity.is_prose())
            .count(),
        wire::MESSAGE_FLOOR,
    );
    assert_eq!(more.len(), 1);
    assert_eq!(
        messages
            .iter()
            .filter(|message| !message.activity.is_prose())
            .count(),
        1,
        "only the latest non-card status is retained",
    );
    let latest = messages.last().expect("latest status");
    assert_eq!(latest.locus.offset, 10_002);
    assert_eq!(latest.activity, wire::AgentActivityKind::Reasoning);
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

/// Lifecycle markers ride beside messages; they never spend the floor.
///
/// Turn and compaction cards are boundaries between messages, not messages.
/// An agent whose recent transcript is dense with them still owes the operator
/// its newest messages, so the markers are carried in addition to the floor.
#[test]
fn interleaved_markers_do_not_spend_an_agents_message_floor() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for offset in 0..60 {
        let mut card = message(0xe5, offset);
        if offset % 3 == 2 {
            card.activity = wire::AgentActivityKind::Turn;
        }
        publisher.publish(card).expect("lane backlog publishes");
    }
    publisher
        .bound_initial_history()
        .expect("startup bounds the retained page");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.activity.is_prose())
            .count(),
        wire::MESSAGE_FLOOR,
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

/// A compaction never leaves an agent showing less than its message floor.
///
/// Trimming below a compaction drops the previous context epoch, which is
/// right while messages newer than the compaction still fill the floor. A
/// compaction that just happened has nothing newer, and the operator still
/// needs the agent's newest messages.
#[test]
fn a_fresh_compaction_does_not_evict_an_agents_message_floor() {
    let bus = fixture(1);
    let source = Source::new(bus.reader(), epoch(), 10);
    let publisher = source.message_publisher();
    for offset in 0..40 {
        publisher
            .publish(message(0xe5, offset))
            .expect("lane backlog publishes");
    }
    let mut compaction = message(0xe5, 40);
    compaction.activity = wire::AgentActivityKind::Compaction;
    publisher
        .publish(compaction)
        .expect("the compaction publishes");
    publisher
        .bound_initial_history()
        .expect("startup bounds the retained page");

    let (_, frames) = source
        .begin(
            &accepted(Actor::Operator),
            &request(0xc4, vec![ViewSubject::Messages], None),
        )
        .expect("message subscription begins");
    let StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.activity.is_prose())
            .count(),
        wire::MESSAGE_FLOOR,
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

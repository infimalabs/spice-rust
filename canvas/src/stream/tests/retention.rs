use super::*;

const HISTORY_LENGTH: u64 = 300;
const QUIET_HISTORY_LENGTH: u64 = 40;
const FIRST_RETAINED_BOUNDARY: u64 = 80;
const SECOND_RETAINED_BOUNDARY: u64 = 220;

fn complete_lane_history(
    source: &Source,
    lane: wire::CanonicalUuid,
    initial: &[AgentMessage],
    advertised_more: &[wire::CanonicalUuid],
) -> Vec<AgentMessage> {
    let mut messages = initial
        .iter()
        .filter(|message| message.lane == lane)
        .cloned()
        .collect::<Vec<_>>();
    let mut more = advertised_more.contains(&lane);
    while more {
        let before = messages
            .first()
            .expect("paged lane has an oldest card")
            .locus;
        let request = StreamRequest::MessageHistory {
            v: WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            epoch: epoch(),
            lane,
            before,
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
        assert_eq!(answered_before, before);
        assert!(older.iter().all(|message| message.lane == lane));
        messages.splice(0..0, older);
        more = page_more;
    }
    messages
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
    assert_eq!(messages.len(), INITIAL_MESSAGE_LIMIT * 2);
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

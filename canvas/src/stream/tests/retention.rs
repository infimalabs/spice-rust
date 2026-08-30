use super::*;

const HISTORY_LENGTH: u64 = 300;
const QUIET_HISTORY_LENGTH: u64 = 40;
const FIRST_RETAINED_BOUNDARY: u64 = 80;
const SECOND_RETAINED_BOUNDARY: u64 = 220;

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
        limit, messages, ..
    } = &frames[0]
    else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(*limit as usize, MESSAGE_CAPACITY);
    assert_eq!(messages.len(), MESSAGE_CAPACITY * 2);
    for lane in [0xe5, 0xe6] {
        let lane = message(lane, 0).lane;
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.lane == lane)
                .count(),
            MESSAGE_CAPACITY,
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
        limit, messages, ..
    } = &frames[0]
    else {
        panic!("expected retained message snapshot")
    };
    assert_eq!(*limit as usize, MESSAGE_CAPACITY);
    let busy = messages
        .iter()
        .filter(|message| message.lane == busy_lane)
        .collect::<Vec<_>>();
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
        messages
            .iter()
            .filter(|message| message.lane == quiet_lane)
            .map(|message| message.locus.offset)
            .collect::<Vec<_>>(),
        (0..QUIET_HISTORY_LENGTH).collect::<Vec<_>>(),
        "one lane's compaction floor does not disturb interleaved lanes",
    );
    drop(source);
    bus.shutdown().expect("bus shuts down");
}

//! Restart reconstruction of the bounded Canvas conversation backlog.

use super::*;

const CODEX_PROSE: &[u8] = br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"durable restart backlog"}}
"#;
const CODEX_APPEND: &[u8] = br#"{"type":"item.completed","item":{"id":"item-2","type":"agent_message","text":"appended after initial backlog"}}
"#;

#[test]
fn each_resident_generation_restores_durable_prose_before_canvas_subscription() {
    let (root, paths) = resident_fixture("message-backlog");
    let bus = Bus::start(journal::Journal::in_memory().expect("backlog authority opens"))
        .expect("backlog bus starts");
    let epoch = test_uuid(4);
    let lane = test_uuid(5);
    let empty_lane = test_uuid(6);
    let author = test_uuid(7);
    seat_backlog_residency(&bus, epoch, lane, empty_lane, author, &root);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the backlog tape opens");
    tape.tee(CODEX_PROSE).expect("durable prose is captured");

    for generation in 0..2 {
        let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
        let publisher = source.message_publisher();
        let mut reader = message_surface::BacklogReader::default();
        let mut diagnostic = Vec::new();
        let residency = current_residency(&bus, epoch).expect("residency reads");
        message_surface::restore_agent_messages(
            &paths,
            residency.body(),
            &publisher,
            &mut reader,
            &mut diagnostic,
        )
        .expect("durable backlog restores");
        assert!(
            diagnostic.is_empty(),
            "generation {generation} restore diagnosed: {}",
            String::from_utf8_lossy(&diagnostic),
        );

        let accepted = wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
        };
        let request = wire::StreamRequest::Subscribe {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(8),
            epoch,
            subjects: vec![wire::ViewSubject::Messages],
            after: None,
        };
        let (_, frames) = source
            .begin(&accepted, &request)
            .expect("Canvas subscription begins");
        let wire::StreamFrame::Messages {
            limit, messages, ..
        } = &frames[0]
        else {
            panic!("expected message snapshot")
        };
        assert_eq!(*limit, 256, "a missing sibling tape is not zero capacity");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].lane, lane);
        assert_eq!(messages[0].author, author);
        assert_eq!(messages[0].text, "durable restart backlog");
        drop(source);
    }

    bus.shutdown().expect("backlog bus stops");
    fs::remove_dir_all(root).expect("backlog fixture cleanup");
}

#[test]
fn repeated_backlog_read_resumes_at_its_source_checkpoint_without_missing_an_append() {
    let (root, paths) = resident_fixture("message-backlog-resume");
    let bus = Bus::start(journal::Journal::in_memory().expect("backlog authority opens"))
        .expect("backlog bus starts");
    let epoch = test_uuid(14);
    let lane = test_uuid(15);
    let empty_lane = test_uuid(16);
    let author = test_uuid(17);
    seat_backlog_residency(&bus, epoch, lane, empty_lane, author, &root);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the backlog tape opens");
    tape.tee(CODEX_PROSE).expect("initial prose is captured");
    let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = source.message_publisher();
    let residency = current_residency(&bus, epoch).expect("residency reads");
    let mut reader = message_surface::BacklogReader::default();
    let mut diagnostic = Vec::new();

    message_surface::restore_agent_messages(
        &paths,
        residency.body(),
        &publisher,
        &mut reader,
        &mut diagnostic,
    )
    .expect("initial backlog restores");
    tape.tee(CODEX_APPEND).expect("appended prose is captured");
    message_surface::restore_agent_messages(
        &paths,
        residency.body(),
        &publisher,
        &mut reader,
        &mut diagnostic,
    )
    .expect("appended backlog resumes");

    let accepted = wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    };
    let request = wire::StreamRequest::Subscribe {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(18),
        epoch,
        subjects: vec![wire::ViewSubject::Messages],
        after: None,
    };
    let (_, frames) = source
        .begin(&accepted, &request)
        .expect("Canvas subscription begins");
    let wire::StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected message snapshot")
    };
    assert_eq!(
        messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        ["durable restart backlog", "appended after initial backlog"],
    );
    assert!(
        diagnostic.is_empty(),
        "checkpoint resume diagnosed: {}",
        String::from_utf8_lossy(&diagnostic),
    );

    drop(source);
    bus.shutdown().expect("backlog bus stops");
    fs::remove_dir_all(root).expect("backlog fixture cleanup");
}

fn seat_backlog_residency(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    empty_lane: CanonicalUuid,
    author: CanonicalUuid,
    worktree: &Path,
) {
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    let seated = write
        .fact(wire::body::UpSeated { epoch }, FactCause::Root)
        .expect("resident fact");
    let worktree = worktree.display().to_string();
    write.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: [lane, empty_lane]
            .into_iter()
            .map(|lane| wire::LaneResidency {
                lane,
                lineage_id: lane,
                name: None,
                worktree: worktree.clone(),
                life: "steer".to_owned(),
                vendor: "codex".parse().expect("fixture vendor"),
                occupant: None,
                lineage: vec![author],
                capabilities: Vec::new(),
                session: None,
                disposition: "vacant".to_owned(),
            })
            .collect(),
        subscriptions: Vec::new(),
    });
    bus.commit(write).expect("residency seeds");
}

//! Restart reconstruction of the bounded Canvas conversation backlog.

use super::*;

const CODEX_PROSE: &[u8] = br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"durable restart backlog"}}
"#;
const CODEX_PROSE_WITHOUT_TIME: &[u8] = br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"durable restart backlog"}}
"#;
const CODEX_TURN_COMPLETE_WITHOUT_TIME: &[u8] = br#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}
"#;
const CODEX_COMPACTION_WITHOUT_TIME: &[u8] = br#"{"type":"compacted"}
"#;
const CODEX_APPEND: &[u8] = br#"{"timestamp":"2030-04-12T18:45:09.271Z","type":"item.completed","item":{"id":"item-2","type":"agent_message","text":"appended after initial backlog"}}
"#;
const CODEX_DISTRIBUTED_ACK: &[u8] = br#"{"timestamp":"2030-04-12T18:46:09.271Z","type":"item.completed","item":{"id":"item-acks","type":"agent_message","text":"ACK\t1kQwPZtK nope 1kQwPZtM: One response.\nNACK\t1kQwPZtL: Unsafe."}}
"#;

fn complete_message_history(
    source: &canvas::stream::Source,
    accepted: &wire::Accepted,
    epoch: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    initial: &[wire::AgentMessage],
) -> Vec<wire::AgentMessage> {
    let mut window = initial.to_vec();
    let mut before = window[0].locus;
    let mut more = true;
    let mut page = 0_u8;
    while more {
        let frame = source
            .message_history(
                accepted,
                &wire::StreamRequest::MessageHistory {
                    v: wire::WireVersion::CURRENT,
                    id: test_uuid(70 + page),
                    epoch,
                    lane,
                    before,
                },
            )
            .expect("older compaction history reads");
        let wire::StreamFrame::MessageHistory {
            messages,
            more: page_more,
            ..
        } = frame
        else {
            panic!("expected message history page")
        };
        assert!(messages.len() <= canvas::stream::MESSAGE_HISTORY_PAGE_LIMIT);
        before = messages[0].locus;
        more = page_more;
        window.splice(0..0, messages);
        page += 1;
    }
    let activities = window
        .iter()
        .map(|message| message.activity)
        .collect::<Vec<_>>();
    let floor =
        wire::AgentActivityKind::retention_start(&activities, canvas::stream::MESSAGE_CAPACITY);
    window.drain(..floor);
    window
}

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
    tape.tee(CODEX_PROSE_WITHOUT_TIME)
        .expect("durable prose is captured");
    tape.tee(CODEX_TURN_COMPLETE_WITHOUT_TIME)
        .expect("durable turn completion is captured");
    tape.tee(CODEX_COMPACTION_WITHOUT_TIME)
        .expect("durable compaction is captured");
    let mut ingest_diagnostic = Vec::new();
    let observations = ingest_transcripts(&bus, &paths, Some(lane), &mut ingest_diagnostic)
        .expect("durable tape is accounted");
    assert_eq!(observations.len(), 1);
    assert!(ingest_diagnostic.is_empty());
    let observed = observations[0].at;

    for generation in 0..2 {
        let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
        let publisher = source.message_publisher();
        let snapshot = bus.fact_snapshot().expect("durable message times load");
        let mut reader = message_surface::BacklogReader::from_snapshot(&snapshot)
            .expect("durable steering context loads");
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
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].lane, lane);
        assert_eq!(messages[0].author, author);
        assert_eq!(messages[0].text, "durable restart backlog");
        assert!(messages[0].final_answer);
        assert_eq!(
            messages[0]
                .automatic_speech
                .as_ref()
                .map(|speech| speech.kind),
            Some(wire::AutomaticSpeechKind::Final),
        );
        assert!(messages[0].authored_synthetic);
        assert_eq!(messages[0].observed.as_ref(), Some(&observed));
        assert_eq!(messages[1].lane, lane);
        assert_eq!(messages[1].author, author);
        assert_eq!(messages[1].activity, wire::AgentActivityKind::Compaction);
        assert_eq!(messages[1].text, "Context compacted");
        assert!(messages[1].authored_synthetic);
        assert_eq!(messages[1].observed.as_ref(), Some(&observed));
        drop(source);
    }

    bus.shutdown().expect("backlog bus stops");
    fs::remove_dir_all(root).expect("backlog fixture cleanup");
}

#[test]
fn each_resident_generation_restores_the_same_complete_compaction_window() {
    let (root, paths) = resident_fixture("message-backlog-compaction-window");
    let bus = Bus::start(journal::Journal::in_memory().expect("backlog authority opens"))
        .expect("backlog bus starts");
    let epoch = test_uuid(44);
    let lane = test_uuid(45);
    let empty_lane = test_uuid(46);
    let author = test_uuid(47);
    seat_backlog_residency(&bus, epoch, lane, empty_lane, author, &root);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the backlog tape opens");
    let filler = "x".repeat(1_800);
    let mut history = Vec::new();
    for index in 0..300 {
        if matches!(index, 20 | 80 | 220) {
            history.extend_from_slice(b"{\"type\":\"compacted\"}\n");
        } else {
            history.extend_from_slice(
                format!(
                    "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"message {index} {filler}\"}}}}\n"
                )
                .as_bytes(),
            );
        }
    }
    tape.tee(&history).expect("compaction history is durable");
    let mut ingest_diagnostic = Vec::new();
    ingest_transcripts(&bus, &paths, Some(lane), &mut ingest_diagnostic)
        .expect("durable tape is accounted");
    assert!(ingest_diagnostic.is_empty());

    let mut generation_windows = Vec::new();
    for generation in 0..2 {
        let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
        let publisher = source.message_publisher();
        let snapshot = bus.fact_snapshot().expect("durable message times load");
        let mut reader = message_surface::BacklogReader::from_snapshot(&snapshot)
            .expect("durable steering context loads");
        let mut diagnostic = Vec::new();
        let residency = current_residency(&bus, epoch).expect("residency reads");
        message_surface::restore_agent_messages(
            &paths,
            residency.body(),
            &publisher,
            &mut reader,
            &mut diagnostic,
        )
        .expect("durable compaction window restores");
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
            id: test_uuid(48 + generation),
            epoch,
            subjects: vec![wire::ViewSubject::Messages],
            after: None,
        };
        let (_, frames) = source
            .begin(&accepted, &request)
            .expect("Canvas subscription begins");
        let wire::StreamFrame::Messages {
            limit,
            messages,
            more,
            ..
        } = &frames[0]
        else {
            panic!("expected message snapshot")
        };
        assert_eq!(*limit as usize, canvas::stream::MESSAGE_CAPACITY);
        assert_eq!(messages.len(), canvas::stream::INITIAL_MESSAGE_LIMIT);
        assert_eq!(more, &[lane]);
        let window = complete_message_history(&source, &accepted, epoch, lane, messages);
        assert_eq!(window.len(), 220);
        assert_eq!(window[0].activity, wire::AgentActivityKind::Compaction);
        assert_eq!(window[0].text, "Context compacted");
        assert_eq!(window[140].activity, wire::AgentActivityKind::Compaction);
        assert!(window[219].text.starts_with("message 299 "));
        generation_windows.push(window);
        drop(source);
    }
    assert_eq!(
        generation_windows[0], generation_windows[1],
        "restart and reload select the identical compaction horizon",
    );

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

#[test]
fn history_refuses_a_cursor_after_the_durable_tape_source_is_replaced() {
    let (root, paths) = resident_fixture("message-history-source-replaced");
    let bus = Bus::start(journal::Journal::in_memory().expect("backlog authority opens"))
        .expect("backlog bus starts");
    let epoch = test_uuid(64);
    let lane = test_uuid(65);
    let empty_lane = test_uuid(66);
    let author = test_uuid(67);
    seat_backlog_residency(&bus, epoch, lane, empty_lane, author, &root);
    let tape_path = paths.tape.join(lane.to_string()).join("codex");
    let tape = tape::Tape::open(&tape_path).expect("the backlog tape opens");
    let history = (0..80)
        .map(|index| {
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"message {index}\"}}}}\n"
            )
        })
        .collect::<String>();
    tape.tee(history.as_bytes())
        .expect("durable history is captured");
    drop(tape);

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
    assert!(diagnostic.is_empty());

    let accepted = wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    };
    let (_, frames) = source
        .begin(
            &accepted,
            &wire::StreamRequest::Subscribe {
                v: wire::WireVersion::CURRENT,
                id: test_uuid(68),
                epoch,
                subjects: vec![wire::ViewSubject::Messages],
                after: None,
            },
        )
        .expect("Canvas subscription begins");
    let wire::StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("expected message snapshot")
    };
    let before = messages[0].locus;

    fs::remove_dir_all(&tape_path).expect("old tape source is removed");
    let error = source
        .message_history(
            &accepted,
            &wire::StreamRequest::MessageHistory {
                v: wire::WireVersion::CURRENT,
                id: test_uuid(69),
                epoch,
                lane,
                before,
            },
        )
        .expect_err("a missing authenticated source is refused");
    assert!(
        format!("{error:?}").contains("transcript source disappeared"),
        "source disappearance remains explicit: {error:?}",
    );

    let replacement = tape::Tape::open(&tape_path).expect("replacement tape opens");
    replacement
        .tee(history.as_bytes())
        .expect("replacement history is captured");

    let error = source
        .message_history(
            &accepted,
            &wire::StreamRequest::MessageHistory {
                v: wire::WireVersion::CURRENT,
                id: test_uuid(70),
                epoch,
                lane,
                before,
            },
        )
        .expect_err("a stale source cursor is refused");
    assert!(
        format!("{error:?}").contains("transcript source changed"),
        "replacement diagnosis preserves the source mismatch: {error:?}",
    );

    drop(source);
    bus.shutdown().expect("backlog bus stops");
    fs::remove_dir_all(root).expect("backlog fixture cleanup");
}

#[test]
fn horizontal_acknowledgements_publish_live_and_replay_with_stable_context() {
    let (root, paths) = resident_fixture("message-backlog-acknowledgements");
    let bus = Bus::start(journal::Journal::in_memory().expect("backlog authority opens"))
        .expect("backlog bus starts");
    let epoch = test_uuid(24);
    let lane = test_uuid(25);
    let empty_lane = test_uuid(26);
    let author = test_uuid(27);
    seat_backlog_residency(&bus, epoch, lane, empty_lane, author, &root);
    seed_steering(
        &bus,
        epoch,
        lane,
        test_uuid(28),
        "1kQwPZtK",
        "First direction.",
    );
    seed_steering(
        &bus,
        epoch,
        lane,
        test_uuid(29),
        "1kQwPZtL",
        "Second direction.",
    );
    let record = mixed_acknowledgement_record(lane, author);
    let expected = concat!(
        "> First direction.\n\n",
        "Nope 1kQwPZtM: One response.\n",
        "> Second direction.\n\n",
        "Unsafe."
    );

    let live = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = live.message_publisher();
    let steering = message_surface::steering_ledger(&bus, &record)
        .expect("live steering context reads")
        .expect("canonical horizontal acknowledgement requests context");
    message_surface::publish_agent_messages(
        &publisher,
        &record,
        &mut MessageState::default(),
        Some(&steering),
    )
    .expect("mixed acknowledgements publish live");
    assert_mixed_acknowledgement_message(&live, epoch, test_uuid(30), expected);
    drop(live);

    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the backlog tape opens");
    tape.tee(CODEX_DISTRIBUTED_ACK)
        .expect("distributed acknowledgements are captured");

    for generation in 0..2 {
        restore_mixed_acknowledgement_message(
            &bus,
            &paths,
            epoch,
            test_uuid(31 + generation),
            expected,
        );
    }

    bus.shutdown().expect("backlog bus stops");
    fs::remove_dir_all(root).expect("backlog fixture cleanup");
}

fn mixed_acknowledgement_record(
    lane: CanonicalUuid,
    author: CanonicalUuid,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(1, 0, 1);
    lane::capture::CaptureRecord {
        lane,
        occupant: author,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: listen::RecordContext {
                timestamp: Some("2030-04-12T18:46:09.271Z".to_owned()),
                ..listen::RecordContext::default()
            },
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: concat!(
                    "ACK\t1kQwPZtK nope 1kQwPZtM: One response.\n",
                    "NACK\t1kQwPZtL: Unsafe."
                )
                .to_owned(),
                phase: None,
                media: Vec::new(),
                locus,
            })],
        },
    }
}

fn restore_mixed_acknowledgement_message(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    request: CanonicalUuid,
    expected: &str,
) {
    let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = source.message_publisher();
    let snapshot = bus.fact_snapshot().expect("steering context reads");
    let mut reader =
        message_surface::BacklogReader::from_snapshot(&snapshot).expect("steering context reduces");
    let residency = current_residency(bus, epoch).expect("residency reads");
    let mut diagnostic = Vec::new();
    message_surface::restore_agent_messages(
        paths,
        residency.body(),
        &publisher,
        &mut reader,
        &mut diagnostic,
    )
    .expect("mixed acknowledgements restore");
    assert!(diagnostic.is_empty());
    assert_mixed_acknowledgement_message(&source, epoch, request, expected);
}

fn assert_mixed_acknowledgement_message(
    source: &canvas::stream::Source,
    epoch: CanonicalUuid,
    request: CanonicalUuid,
    expected: &str,
) {
    let accepted = wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    };
    let request = wire::StreamRequest::Subscribe {
        v: wire::WireVersion::CURRENT,
        id: request,
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
    assert_eq!(messages.len(), 1, "one event remains one card");
    assert_eq!(messages[0].text, expected);
    assert!(!messages[0].text.contains("ACK 1kQwPZtK"));
    assert!(!messages[0].text.contains("NACK 1kQwPZtL"));
    assert_eq!(messages[0].text.matches("> First direction.").count(), 1);
    assert_eq!(messages[0].text.matches("> Second direction.").count(), 1);
    assert_eq!(messages[0].text.matches("Nope").count(), 1);
    assert_eq!(messages[0].text.matches("1kQwPZtM").count(), 1);
    assert_eq!(messages[0].text.matches("One response.").count(), 1);
    assert_eq!(messages[0].text.matches("Unsafe.").count(), 1);
}

fn seed_steering(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    key: &str,
    text: &str,
) {
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("steering instant");
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .expect("steering origin");
    let body = wire::body::SteerSent {
        directive,
        fanout: None,
        key: key.to_owned(),
        lane,
        text: text.to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2030-04-12T19:44:09.271Z".parse().expect("steering expiry"),
    };
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let sent = write
        .fact(body.clone(), FactCause::Root)
        .expect("steering fact builds");
    write.steering_head(sent, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("steering head builds")
    });
    assert!(matches!(bus.commit(write), Ok(CommitOutcome::Committed(_))));
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

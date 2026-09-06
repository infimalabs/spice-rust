//! Resident media admission and durable steering replay regressions.

use super::*;

#[test]
fn direct_steer_send_refuses_invalid_media_before_journal_mutation() {
    let (root, paths) = resident_fixture("steer-media-admission");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let worktree = repository.worktree_root().to_string_lossy().into_owned();
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    seed_lane_in_worktree(&bus, epoch, lane, occupant, &worktree);

    let request = wire::intent::SteerSend {
        fanout: test_uuid(90),
        lane,
        message: "inspect this capture".to_owned(),
        attachments: vec![wire::Media {
            name: "malformed.png".to_owned(),
            content_type: "image/png".to_owned(),
            data_url: "not-a-data-url".to_owned(),
        }],
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(30),
        epoch,
        kind: CommandKind::SteerSend,
        expected: vec![Witness::Residency {
            lane,
            occupant,
            epoch,
        }],
        body: serde_json::to_value(&request).expect("steer request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("crossing admission leaves resident media validation authoritative");
    let before = bus.fact_snapshot().expect("journal reads before refusal");

    let outcome = steering::send(
        &bus,
        &paths,
        epoch,
        &steer::Config::default(),
        &command,
        &request,
    );
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "steer::media-invalid"
    ));

    let after = bus.fact_snapshot().expect("journal reads after refusal");
    assert_eq!(after.horizon(), before.horizon());
    assert_eq!(after.facts().len(), before.facts().len());

    bus.verify().expect("refused authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn operator_media_replays_after_restart_into_the_browsable_pending_directive() {
    let (root, paths) = resident_fixture("steer-media-restart");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let worktree = repository.worktree_root().to_string_lossy().into_owned();
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let bus = Bus::start(journal::Journal::open(&paths.journal).expect("fixture journal opens"))
        .expect("fixture bus starts");
    let epoch = test_uuid(1);
    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    seed_lane_in_worktree(&bus, epoch, lane, occupant, &worktree);
    let media = wire::Media::from_base64("operator-capture.png", "image/png", "iVBORw0KGgo=")
        .expect("operator image admits");
    let send = wire::intent::SteerSend {
        fanout: test_uuid(31),
        lane,
        message: "inspect this capture".to_owned(),
        attachments: vec![media.clone()],
    };
    let send_command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(30),
        epoch,
        kind: CommandKind::SteerSend,
        expected: vec![Witness::Residency {
            lane,
            occupant,
            epoch,
        }],
        body: serde_json::to_value(&send).expect("steer request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator send admits");
    assert!(matches!(
        steering::send(
            &bus,
            &paths,
            epoch,
            &steer::Config::default(),
            &send_command,
            &send,
        ),
        Outcome::Reply(_)
    ));
    let inbox = paths
        .inbox_for_worktree(&worktree)
        .expect("fixture worktree inbox resolves");
    let rendered = fs::read_to_string(steer::inbox::path(&inbox, lane))
        .expect("canonical lane inbox is published");
    assert!(
        rendered.contains(&format!("directive={}", send_command.id())),
        "lane delivery lost directive causality: {rendered}",
    );
    let reference = wire::AgentMediaReference::from_media(send_command.id(), 0, &media)
        .expect("fixture media reference derives");
    let encoded_reference =
        String::from_utf8(wire::encode(&reference).expect("fixture reference encodes"))
            .expect("reference JSON is UTF-8");
    assert!(rendered.contains(&encoded_reference));
    assert_eq!(
        rendered.matches(&media.data_url).count(),
        0,
        "typed direct-send authority bytes stay out of disposable lane prose",
    );
    bus.verify()
        .expect("published directive authority verifies");
    bus.shutdown().expect("first resident generation stops");

    let bus = Bus::start(journal::Journal::open(&paths.journal).expect("journal reopens"))
        .expect("replacement bus starts");
    let inbox_request = wire::intent::SteerInbox { lane: Some(lane) };
    let inbox_command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(31),
        epoch,
        kind: CommandKind::SteerInbox,
        expected: Vec::new(),
        body: serde_json::to_value(&inbox_request).expect("inbox request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator inbox admits");
    let Outcome::Reply(reply) =
        steering::inbox(&bus, &paths, epoch, &inbox_command, &inbox_request)
    else {
        panic!("restarted inbox reads durable authority");
    };
    let CommandAnswer::Inbox(inbox) = reply.body else {
        panic!("inbox read returns the inbox answer");
    };
    assert_eq!(inbox.lanes.len(), 1);
    assert_eq!(inbox.lanes[0].pending.len(), 1);
    assert_eq!(
        inbox.lanes[0].pending[0].attachments.as_slice(),
        std::slice::from_ref(&media),
    );
    assert_eq!(
        inbox.lanes[0].pending[0].attachments[0]
            .identity()
            .expect("replayed media stays admitted"),
        media.identity().expect("authored media has identity"),
    );

    bus.verify().expect("replayed authority verifies");
    bus.shutdown().expect("replacement bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn codex_image_tool_capture_reaches_live_reconnect_and_older_history_once() {
    const NEWER_MESSAGES: u64 = 30;
    let (root, paths) = resident_fixture("codex-tool-media");
    let bus = Bus::start(journal::Journal::in_memory().expect("message authority opens"))
        .expect("message bus starts");
    let epoch = test_uuid(40);
    let lane = test_uuid(41);
    let occupant = test_uuid(42);
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let trust =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    commit_startup(&bus, &repository, &trust, epoch, None).expect("resident seats");
    seed_lane(&bus, epoch, lane, occupant, &root);
    let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("capture fixture occupant starts");
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher);
    let mut diagnostics = Vec::new();
    let mut message_state = MessageState::default();

    let mut offset = 120_u64;
    for bytes in codex_image_tool_records() {
        let length = u64::try_from(bytes.len()).expect("fixture record length fits");
        let locus = listen::Locus::new(4, offset, length);
        let record = lane::capture::CaptureRecord {
            lane,
            occupant,
            stream: vendor::RecordStream::Stdout,
            boundary: sill::RecordBoundary::Complete,
            locus,
            declaration: None,
            decoded: listen::decode(listen::Dialect::Codex, &bytes, locus)
                .expect("captured Codex media record decodes"),
        };
        accept_capture_record(
            &bus,
            epoch,
            &mut seats,
            &surfaces,
            None,
            &record,
            CaptureViews::new(&mut message_state, &mut diagnostics),
        )
        .expect("captured Codex media record projects");
        offset = offset.checked_add(length).expect("fixture locus advances");
    }

    let call = updates.try_recv().expect("tool call publishes");
    assert_eq!(call.activity, wire::AgentActivityKind::Tool);
    assert!(call.media.is_empty());
    let result = updates.try_recv().expect("tool image result publishes");
    assert_eq!(result.activity, wire::AgentActivityKind::Tool);
    assert!(result.text.is_empty());
    let [media] = result.media.as_slice() else {
        panic!("one captured image reaches the live result");
    };
    assert_eq!(media.content_type, "image/png");
    let identity = media.identity().expect("live image identity derives");
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert_reconnect_media(&source, epoch, lane, &identity);

    for index in 0..NEWER_MESSAGES {
        publisher
            .publish(filler_message(lane, occupant, index))
            .expect("newer filler message publishes");
    }
    assert_older_media(&source, epoch, lane, &identity);

    seats
        .vacate(lane)
        .expect("capture fixture remains supervised")
        .wait()
        .expect("capture fixture stops");
    drop(source);
    bus.verify().expect("media capture authority verifies");
    bus.shutdown().expect("message bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn codex_image_tool_records() -> Vec<Vec<u8>> {
    const CALL: &str = "call_N4XyMbDlajHB5WdorSMM6UBb";
    const PNG: &str = "iVBORw0KGgo=";
    [
        serde_json::json!({
            "timestamp": "2026-09-06T18:15:34.681Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "id": "ctc-image",
                "status": "completed",
                "call_id": CALL,
                "name": "exec",
                "input": "const r = await tools.view_image({ path: image }); image(r.image_url);",
            },
        }),
        serde_json::json!({
            "timestamp": "2026-09-06T18:15:34.867Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call_output",
                "id": "ctco-image",
                "call_id": CALL,
                "output": [
                    {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds"},
                    {
                        "type": "input_image",
                        "detail": "original",
                        "image_url": format!("data:image/png;base64,{PNG}"),
                    },
                ],
            },
        }),
        serde_json::json!({
            "timestamp": "2026-09-06T18:15:34.868Z",
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {
                    "type": "ImageView",
                    "id": "exec-image",
                    "path": "file:///tmp/source-capture.png",
                },
            },
        }),
    ]
    .into_iter()
    .map(|record| serde_json::to_vec(&record).expect("Codex media fixture encodes"))
    .collect()
}

fn accepted(epoch: wire::CanonicalUuid) -> wire::Accepted {
    wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    }
}

fn subscribe(epoch: wire::CanonicalUuid, id: wire::CanonicalUuid) -> wire::StreamRequest {
    wire::StreamRequest::Subscribe {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        subjects: vec![wire::ViewSubject::Messages],
        after: None,
    }
}

fn assert_reconnect_media(
    source: &canvas::stream::Source,
    epoch: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    identity: &wire::Digest,
) {
    let (_, frames) = source
        .begin(&accepted(epoch), &subscribe(epoch, test_uuid(43)))
        .expect("message reconnect begins");
    let wire::StreamFrame::Messages { messages, .. } = &frames[0] else {
        panic!("message reconnect returns its snapshot");
    };
    let identities = media_identities(messages, lane);
    assert_eq!(identities.as_slice(), std::slice::from_ref(identity));
}

fn assert_older_media(
    source: &canvas::stream::Source,
    epoch: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    identity: &wire::Digest,
) {
    let (_, frames) = source
        .begin(&accepted(epoch), &subscribe(epoch, test_uuid(44)))
        .expect("paged reconnect begins");
    let wire::StreamFrame::Messages { messages, more, .. } = &frames[0] else {
        panic!("paged reconnect returns its snapshot");
    };
    assert_eq!(more, &[lane]);
    assert!(media_identities(messages, lane).is_empty());
    let before = messages
        .iter()
        .find(|message| message.lane == lane && message.activity.is_prose())
        .expect("initial page has a lane message")
        .locus;
    let request = wire::StreamRequest::MessageHistory {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(45),
        epoch,
        lane,
        before,
    };
    let wire::StreamFrame::MessageHistory { messages, .. } = source
        .message_history(&accepted(epoch), &request)
        .expect("older message page reads")
    else {
        panic!("older request returns one message page");
    };
    let identities = media_identities(&messages, lane);
    assert_eq!(identities.as_slice(), std::slice::from_ref(identity));
}

fn media_identities(
    messages: &[wire::AgentMessage],
    lane: wire::CanonicalUuid,
) -> Vec<wire::Digest> {
    messages
        .iter()
        .filter(|message| message.lane == lane)
        .flat_map(|message| &message.media)
        .map(|media| media.identity().expect("retained image identity derives"))
        .collect()
}

fn filler_message(
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    index: u64,
) -> wire::AgentMessage {
    wire::AgentMessage {
        lane,
        author: occupant,
        locus: wire::MessageLocus {
            member: 4,
            offset: 10_000 + index,
            length: 1,
            event: 0,
        },
        authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        authored_synthetic: false,
        observed: None,
        activity: wire::AgentActivityKind::Prose,
        final_answer: false,
        text: format!("newer message {index}"),
        automatic_speech: None,
        unavailable_tool_terminals: Vec::new(),
        media: Vec::new(),
    }
}

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

    let mut effects = CommandEffects::default();
    let outcome = steering::send(
        &bus,
        epoch,
        &steer::Config::default(),
        &command,
        &request,
        &mut effects,
    );
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "steer::media-invalid"
    ));
    assert!(effects.inbox_projection.is_none());

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
    send_and_project(&bus, &paths, epoch, &send_command, &send);
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

    assert_media_replays(&paths, epoch, lane, &media);
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn send_and_project(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    send: &wire::intent::SteerSend,
) {
    let mut effects = CommandEffects::default();
    assert!(matches!(
        steering::send(
            bus,
            epoch,
            &steer::Config::default(),
            command,
            send,
            &mut effects,
        ),
        Outcome::Reply(Reply {
            body: CommandAnswer::Directive(answer),
            ..
        }) if matches!(*answer.delivery, wire::intent::DeliveryStanding::Unknown)
    ));
    let projected = effects
        .inbox_projection
        .expect("durable send schedules its lane projection");
    let residency = current_residency(bus, epoch).expect("current residency reads");
    steering::publish_inbox(bus, paths, residency.body(), projected)
        .expect("post-receipt projection publishes the inbox");
}

fn assert_media_replays(
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    media: &wire::Media,
) {
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
        std::slice::from_ref(media),
    );
    assert_eq!(
        inbox.lanes[0].pending[0].attachments[0]
            .identity()
            .expect("replayed media stays admitted"),
        media.identity().expect("authored media has identity"),
    );

    bus.verify().expect("replayed authority verifies");
    bus.shutdown().expect("replacement bus stops");
}

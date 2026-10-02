//! Resident media admission and durable steering replay regressions.

use super::*;
use base64::Engine as _;

#[test]
fn direct_steer_send_refuses_unjournalable_content_before_mutation() {
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
    let media = large_media();
    let malformed = wire::Media {
        name: "malformed.png".to_owned(),
        content_type: "image/png".to_owned(),
        data_url: "not-a-data-url".to_owned(),
    };
    let cases = [
        (
            "inspect".to_owned(),
            vec![malformed],
            "steer::media-invalid",
        ),
        (
            "inspect".to_owned(),
            vec![media.clone(), media.clone()],
            "steer::media-invalid",
        ),
        (
            "\0".repeat(journal::MAX_DURABLE_FACT_ENVELOPE_BYTES / 6),
            Vec::new(),
            "steer::envelope-too-large",
        ),
        (
            "x".repeat(2 * 1024 * 1024),
            vec![media],
            "steer::envelope-too-large",
        ),
    ];
    let before = bus.fact_snapshot().expect("journal reads before refusals");
    for (message, attachments, reason) in cases {
        let request = wire::intent::SteerSend {
            fanout: test_uuid(90),
            lane,
            message,
            attachments,
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(30),
            epoch,
            kind: CommandKind::SteerSend,
            expected: vec![Witness::Residency {
                lane,
                occupant: Some(occupant),
                epoch,
            }],
            body: serde_json::to_value(&request).expect("steer request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
        })
        .expect("resident content admission remains authoritative");
        let mut effects = CommandEffects::default();
        let outcome = steering::send(
            &bus,
            epoch,
            &steer::Config::default(),
            &command,
            &request,
            &mut effects,
        );
        let Outcome::Refusal(refusal) = outcome else {
            panic!("unjournalable direction must refuse at admission: {outcome:?}");
        };
        assert_eq!(refusal.reason(), reason);
        let maximum = if reason == "steer::media-invalid" {
            wire::media::MAX_MEDIA_DATA_URL_BYTES
        } else {
            journal::MAX_DURABLE_FACT_ENVELOPE_BYTES
        };
        assert!(
            refusal
                .diagnostic()
                .unwrap()
                .expected()
                .unwrap()
                .contains(&maximum.to_string()),
            "operator refusal must name the violated byte bound"
        );
        assert!(effects.inbox_delivery.is_none());
        let after = bus.fact_snapshot().expect("journal reads after refusal");
        assert_eq!(after.horizon(), before.horizon());
        assert_eq!(after.facts(), before.facts());
    }
    bus.verify().expect("refused authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn large_media() -> wire::Media {
    let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
    let payload_bytes =
        (wire::media::MAX_MEDIA_DATA_URL_BYTES - "data:image/webp;base64,".len()) / 4 * 3;
    bytes.resize(payload_bytes, 0);
    wire::Media::from_base64(
        "operator-capture.webp",
        "image/webp",
        &base64::engine::general_purpose::STANDARD.encode(bytes),
    )
    .expect("operator image validates")
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
    let media = large_media();
    let send = wire::intent::SteerSend {
        fanout: test_uuid(31),
        lane,
        message: format!(
            "inspect this capture\n\n{}",
            media
                .reference_markdown()
                .expect("placement marker renders"),
        ),
        attachments: vec![media.clone()],
    };
    let send_command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(30),
        epoch,
        kind: CommandKind::SteerSend,
        expected: vec![Witness::Residency {
            lane,
            occupant: Some(occupant),
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
    send_direction(&bus, epoch, &send_command, &send);
    let inbox = read_inbox(&bus, &paths, epoch, lane, test_uuid(31));
    let rendered = steer::inbox::render_pending(
        &steer::Key::for_identity(lane),
        &inbox.lanes[0].pending,
        |_, _| None,
    );
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
        "authenticated lane prose excludes authority bytes",
    );
    let snapshot = bus.fact_snapshot().expect("sent authority reads");
    let sent = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<wire::body::SteerSent>())
        .expect("directive was retained");
    let encoded = serde_json::to_string(&sent).expect("directive encodes");
    assert_eq!(
        encoded.matches(&media.data_url).count(),
        1,
        "durable directive must retain the image bytes exactly once",
    );
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
    assert_eq!(
        ledger
            .resolve_media(lane, encoded_reference.as_bytes())
            .unwrap(),
        media.decoded_bytes().unwrap(),
    );
    let display = wire::media::with_media_payloads(&sent.text, &sent.attachments).unwrap();
    assert_eq!(
        wire::media::parse_markdown(&display),
        [Ok(media.clone())],
        "app display must recover the original image",
    );
    bus.verify()
        .expect("published directive authority verifies");
    bus.shutdown().expect("first resident generation stops");

    assert_media_replays(&paths, epoch, lane, &media);
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn send_direction(
    bus: &Bus,
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
            body: CommandAnswer::Directive(_),
            ..
        })
    ));
    assert_eq!(effects.inbox_delivery, Some(send.lane));
}

fn read_inbox(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    command_id: CanonicalUuid,
) -> wire::intent::InboxAnswer {
    let request = wire::intent::SteerInbox {
        lane: Some(lane),
        retired: false,
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id: command_id,
        epoch,
        kind: CommandKind::SteerInbox,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("inbox request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator inbox admits");
    let Outcome::Reply(reply) = steering::inbox(bus, paths, epoch, &command, &request) else {
        panic!("inbox reads durable authority");
    };
    let CommandAnswer::Inbox(inbox) = reply.body else {
        panic!("inbox read returns the inbox answer");
    };
    inbox
}
fn assert_media_replays(
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    media: &wire::Media,
) {
    let bus = Bus::start(journal::Journal::open(&paths.journal).expect("journal reopens"))
        .expect("replacement bus starts");
    let inbox = read_inbox(&bus, paths, epoch, lane, test_uuid(32));
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

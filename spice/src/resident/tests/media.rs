//! Resident media admission and durable steering replay regressions.

use super::*;

#[test]
fn direct_steer_send_refuses_every_invalid_media_shape_before_journal_mutation() {
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

    let valid = wire::Media::from_base64("capture.png", "image/png", "iVBORw0KGgo=")
        .expect("fixture image admits");
    let oversized_payload = format!(
        "UklGRgAAAABXRUJQ{}",
        "AAAA".repeat((wire::media::MAX_MEDIA_BYTES + 1 - 12) / 3),
    );
    let invalid_sets = [
        vec![wire::Media {
            name: "malformed.png".to_owned(),
            content_type: "image/png".to_owned(),
            data_url: "not-a-data-url".to_owned(),
        }],
        vec![wire::Media {
            name: "malformed-base64.png".to_owned(),
            content_type: "image/png".to_owned(),
            data_url: "data:image/png;base64,%%%".to_owned(),
        }],
        vec![wire::Media {
            name: "unsupported.txt".to_owned(),
            content_type: "text/plain".to_owned(),
            data_url: "data:text/plain;base64,aGVsbG8=".to_owned(),
        }],
        vec![wire::Media {
            name: "mismatch.jpg".to_owned(),
            content_type: "image/jpeg".to_owned(),
            data_url: "data:image/jpeg;base64,iVBORw0KGgo=".to_owned(),
        }],
        vec![wire::Media {
            name: "oversized.webp".to_owned(),
            content_type: "image/webp".to_owned(),
            data_url: format!("data:image/webp;base64,{oversized_payload}"),
        }],
        vec![valid; wire::media::MAX_MEDIA_ITEMS + 1],
    ];

    for (index, attachments) in invalid_sets.into_iter().enumerate() {
        let request = wire::intent::SteerSend {
            fanout: test_uuid(u8::try_from(90 + index).expect("fixture fanout id fits")),
            lane,
            message: "inspect this capture".to_owned(),
            attachments,
        };
        let id = u8::try_from(30 + index).expect("fixture command id fits");
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
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
        let before = bus.canvas_snapshot().expect("journal reads before refusal");
        let before_cursor = before.cursor();
        let before_facts = before.facts().facts().len();

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

        let after = bus.canvas_snapshot().expect("journal reads after refusal");
        assert_eq!(after.cursor(), before_cursor);
        assert_eq!(after.facts().facts().len(), before_facts);
    }

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
    assert!(rendered.contains(&media.markdown().expect("fixture media renders")));
    assert_eq!(
        rendered.matches(&media.data_url).count(),
        1,
        "typed direct-send media reaches the lane exactly once",
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

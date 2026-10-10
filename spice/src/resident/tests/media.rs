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

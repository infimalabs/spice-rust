//! Each lane's inbox keeps its recent retirements beside its live direction.

use wire::Disposition;

use super::*;

const OTHER: u8 = 2;
const RECIPIENT: u8 = 4;

struct Lanes {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
}

impl Lanes {
    /// Two occupied lanes.
    fn new(tag: &str) -> Self {
        let (root, paths) = resident_fixture(tag);
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let worktree = repository.worktree_root().to_string_lossy().into_owned();
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let bus =
            Bus::start(journal::Journal::open(&paths.journal).expect("fixture journal opens"))
                .expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        for lane in [OTHER, RECIPIENT] {
            seed_vendor_lane_in_worktree(
                &bus,
                epoch,
                test_uuid(lane),
                test_uuid(lane + 1),
                &worktree,
                "omp",
            );
        }
        Self {
            root,
            paths,
            bus,
            epoch,
        }
    }

    fn admit<B: serde::Serialize>(
        &self,
        kind: CommandKind,
        id: u8,
        witness: Witness,
        body: &B,
    ) -> wire::AdmittedCommand {
        Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind,
            expected: vec![witness],
            body: serde_json::to_value(body).expect("request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
            release: "1.2.3".parse().expect("fixture release"),
        })
        .expect("operator command admits")
    }

    /// Direct one lane as the operator.
    fn send(&self, id: u8, to: u8, fanout: u8, message: &str) -> wire::intent::DirectiveAnswer {
        let send = wire::intent::SteerSend {
            fanout: test_uuid(fanout),
            lane: test_uuid(to),
            message: message.to_owned(),
            attachments: Vec::new(),
        };
        let witness = Witness::Residency {
            lane: test_uuid(to),
            occupant: Some(test_uuid(to + 1)),
            epoch: self.epoch,
        };
        let command = self.admit(CommandKind::SteerSend, id, witness, &send);
        let outcome = steering::send(
            &self.bus,
            self.epoch,
            &steer::Config::default(),
            &command,
            &send,
            &mut CommandEffects::default(),
        );
        let Outcome::Reply(Reply {
            body: CommandAnswer::Directive(sent),
            ..
        }) = outcome
        else {
            panic!("operator direction is durable: {outcome:?}");
        };
        sent
    }

    fn cancel(&self, id: u8, directive: CanonicalUuid) -> Outcome<CommandAnswer> {
        let cancel = wire::intent::DirectiveRef { directive };
        let witness = Witness::Directive {
            steer: directive,
            disposition: Disposition::Live,
        };
        let command = self.admit(CommandKind::SteerCancel, id, witness, &cancel);
        steering::cancel(
            &self.bus,
            self.epoch,
            &command,
            &cancel,
            &mut CommandEffects::default(),
        )
    }

    fn inbox(&self, id: u8, actor: Actor, retired: bool) -> wire::intent::InboxAnswer {
        let request = wire::intent::SteerInbox {
            lane: None,
            retired,
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::SteerInbox,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("inbox request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor,
            epoch: self.epoch,
            release: "1.2.3".parse().expect("fixture release"),
        })
        .expect("inbox admits");
        let Outcome::Reply(Reply {
            body: CommandAnswer::Inbox(inbox),
            ..
        }) = steering::inbox(&self.bus, &self.paths, self.epoch, &command, &request)
        else {
            panic!("inbox reads durable authority");
        };
        inbox
    }

    fn finish(self) {
        self.bus.verify().expect("the authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

#[test]
fn a_lane_inbox_keeps_its_retired_direction_with_fanout_beside_live_direction() {
    let lanes = Lanes::new("steer-inbox-retired");
    let recipient = test_uuid(RECIPIENT);
    let live = lanes.send(60, RECIPIENT, 59, "the parser moved");
    let sent = lanes.send(
        62,
        RECIPIENT,
        61,
        "> quoted context\nkeep the parser where it is",
    );
    let canceled = lanes.cancel(63, sent.directive);
    assert!(matches!(canceled, Outcome::Reply(_)), "{canceled:?}");

    let inbox = lanes.inbox(64, Actor::Lane(recipient), true);
    let [row] = inbox.lanes.as_slice() else {
        panic!("a lane principal reads only its own lane: {inbox:?}")
    };
    assert_eq!(row.lane, recipient);
    let [pending] = row.pending.as_slice() else {
        panic!("the other direction is still live: {row:?}")
    };
    assert_eq!(
        (pending.key.as_str(), pending.fanout),
        (live.key.as_str(), Some(test_uuid(59)))
    );
    let [retired] = row.retired.as_slice() else {
        panic!("the canceled direction stays readable: {row:?}")
    };
    assert_eq!(retired.directive, sent.directive);
    assert_eq!(retired.key, sent.key);
    assert_eq!(retired.fanout, Some(test_uuid(61)));
    assert_eq!(retired.from, None);
    assert_eq!(retired.disposition, Disposition::Canceled);
    assert_eq!(retired.authored, sent.authored);
    assert_eq!(
        retired.text,
        "> quoted context\nkeep the parser where it is"
    );
    assert!(retired.attachments.is_empty());
    assert_eq!(
        retired.preview.as_deref(),
        Some("keep the parser where it is")
    );
    assert!(retired.retired >= retired.authored);

    let operator = lanes.inbox(65, Actor::Operator, false);
    assert!(
        operator.lanes.iter().all(|lane| lane.retired.is_empty()),
        "retirements are read only on request"
    );
    let operator = lanes.inbox(66, Actor::Operator, true);
    let others = operator
        .lanes
        .iter()
        .filter(|lane| lane.lane != recipient)
        .flat_map(|lane| &lane.retired)
        .count();
    assert_eq!(others, 0, "a retirement belongs only to its own lane");

    lanes.finish();
}

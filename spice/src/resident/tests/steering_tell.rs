//! A seated lane tells another seated lane something, as itself, and each
//! lane's inbox keeps its recent retirements beside its live direction.

use std::collections::BTreeMap;

use wire::Disposition;

use super::*;

const SENDER: u8 = 2;
const RECIPIENT: u8 = 4;
const VACANT: u8 = 6;

struct Peers {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
}

impl Peers {
    /// Two occupied lanes and one lane nobody sits in.
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
        seed_vendor_lane_in_worktree(
            &bus,
            epoch,
            test_uuid(SENDER),
            test_uuid(SENDER + 1),
            &worktree,
            "omp",
        );
        seed_vendor_lane_in_worktree(
            &bus,
            epoch,
            test_uuid(RECIPIENT),
            test_uuid(RECIPIENT + 1),
            &worktree,
            "omp",
        );
        seed_vacant_lane_in_worktree(&bus, epoch, test_uuid(VACANT), &worktree);
        Self {
            root,
            paths,
            bus,
            epoch,
        }
    }

    fn seat(&self, lane: u8) -> Witness {
        let lane = test_uuid(lane);
        let occupant = current_residency(&self.bus, self.epoch)
            .expect("residency reads")
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == lane)
            .and_then(|row| row.occupant);
        Witness::Residency {
            lane,
            occupant,
            epoch: self.epoch,
        }
    }

    fn admit<B: serde::Serialize>(
        &self,
        kind: CommandKind,
        id: u8,
        actor: Actor,
        witness: Witness,
        body: &B,
    ) -> Result<wire::AdmittedCommand, wire::CommandRejection> {
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
            actor,
            epoch: self.epoch,
        })
    }

    fn tell(&self, id: u8, to: u8, message: &str) -> (Outcome<CommandAnswer>, CommandEffects) {
        let tell = wire::intent::SteerTell {
            lane: test_uuid(to),
            message: message.to_owned(),
        };
        let command = self
            .admit(
                CommandKind::SteerTell,
                id,
                Actor::Lane(test_uuid(SENDER)),
                self.seat(to),
                &tell,
            )
            .expect("a lane's tell admits");
        let mut effects = CommandEffects::default();
        let outcome = steering::tell(
            &self.bus,
            self.epoch,
            &steer::Config::default(),
            &command,
            &tell,
            &mut effects,
        );
        (outcome, effects)
    }

    /// Direct one lane as the operator.
    fn send(&self, id: u8, to: u8, fanout: u8, message: &str) -> wire::intent::DirectiveAnswer {
        let send = wire::intent::SteerSend {
            fanout: test_uuid(fanout),
            lane: test_uuid(to),
            message: message.to_owned(),
            attachments: Vec::new(),
        };
        let command = self
            .admit(
                CommandKind::SteerSend,
                id,
                Actor::Operator,
                self.seat(to),
                &send,
            )
            .expect("operator send admits");
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

    /// Land one assistant message on a lane's tape and hear its headers,
    /// returning what the hearing wrote down.
    fn answer(&self, lane: u8, keys: &[&str], prose: &str) -> String {
        let lane = test_uuid(lane);
        let tape = tape::Tape::open(self.paths.tape.join(lane.to_string()).join("omp"))
            .expect("the lane tape opens");
        let message = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": prose}],
                "stopReason": "stop",
            },
        });
        tape.tee(format!("{message}\n").as_bytes())
            .expect("the answer is durable");
        let keys = keys
            .iter()
            .map(|key| key.parse().expect("fixture key"))
            .collect();
        let vendor: wire::Name = "omp".parse().expect("fixture vendor");
        let mut diagnostics = Vec::new();
        steering::dispose_heard(
            &self.bus,
            &self.paths,
            self.epoch,
            lane,
            &vendor,
            &keys,
            &mut diagnostics,
        );
        String::from_utf8(diagnostics).expect("diagnostics are UTF-8")
    }

    fn pending(&self, lane: u8) -> Vec<wire::intent::PendingDirective> {
        let inbox = self.inbox(90, Actor::Lane(test_uuid(lane)), false);
        let [row] = inbox.lanes.as_slice() else {
            panic!("a lane principal reads only its own lane: {inbox:?}")
        };
        row.pending.clone()
    }

    fn cancel(&self, id: u8, directive: CanonicalUuid) -> Outcome<CommandAnswer> {
        let cancel = wire::intent::DirectiveRef { directive };
        let witness = Witness::Directive {
            steer: directive,
            disposition: Disposition::Live,
        };
        let command = self
            .admit(
                CommandKind::SteerCancel,
                id,
                Actor::Operator,
                witness,
                &cancel,
            )
            .expect("operator cancel admits");
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
fn a_lane_tells_a_seated_peer_as_itself_and_the_delivery_names_the_sender() {
    let peers = Peers::new("steer-tell");
    let sender = test_uuid(SENDER);
    let recipient = test_uuid(RECIPIENT);

    let (outcome, effects) = peers.tell(40, RECIPIENT, "the parser moved to listen/src/omp.rs");
    let Outcome::Reply(Reply {
        body: CommandAnswer::Directive(told),
        ..
    }) = outcome
    else {
        panic!("a peer message is durable: {outcome:?}");
    };
    assert_eq!(effects.inbox_delivery, Some(recipient));

    let sent = peers.send(42, RECIPIENT, 41, "keep the parser where it is");

    let snapshot = peers.bus.fact_snapshot().expect("authority reads");
    let creators = snapshot
        .facts()
        .iter()
        .filter_map(|envelope| {
            let body = envelope.fact.read::<wire::body::SteerSent>()?;
            Some((body.directive, (envelope.actor.clone(), body.from)))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        creators[&told.directive],
        (Actor::Lane(sender), Some(sender)),
        "the peer message is journaled as the sending lane"
    );
    assert_eq!(creators[&sent.directive], (Actor::Operator, None));

    let request = wire::intent::SteerInbox {
        lane: Some(recipient),
        retired: false,
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(43),
        epoch: peers.epoch,
        kind: CommandKind::SteerInbox,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("inbox request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: peers.epoch,
    })
    .expect("operator inbox admits");
    let Outcome::Reply(Reply {
        body: CommandAnswer::Inbox(inbox),
        ..
    }) = steering::inbox(&peers.bus, &peers.paths, peers.epoch, &command, &request)
    else {
        panic!("inbox reads durable authority");
    };
    let [row] = inbox.lanes.as_slice() else {
        panic!("one recipient row: {inbox:?}")
    };
    let seal: steer::Key = "1kQnrzNZ".parse().expect("fixture seal");
    let render = |key: &str| {
        let pending = row
            .pending
            .iter()
            .filter(|directive| directive.key == key)
            .cloned()
            .collect::<Vec<_>>();
        steer::inbox::render_pending(&seal, &pending, |_, _| None)
    };
    let peer = render(&told.key);
    assert!(
        peer.contains(&format!("from=lane:{sender}\n[PEER lane {sender}]")),
        "the recipient hears who told it: {peer}"
    );
    let operator = render(&sent.key);
    assert!(
        !operator.contains("from=") && !operator.contains("[PEER"),
        "operator direction names no sender: {operator}"
    );

    peers.finish();
}

#[test]
fn an_answered_peer_message_replies_once_to_its_seated_sender_and_the_reply_ends_there() {
    let peers = Peers::new("steer-tell-reply");
    let recipient = test_uuid(RECIPIENT);
    let (outcome, _) = peers.tell(70, RECIPIENT, "the parser moved");
    let Outcome::Reply(Reply {
        body: CommandAnswer::Directive(told),
        ..
    }) = outcome
    else {
        panic!("a peer message is durable: {outcome:?}");
    };
    let sent = peers.send(71, RECIPIENT, 72, "keep the parser where it is");

    let heard = peers.answer(
        RECIPIENT,
        &[&told.key, &sent.key],
        &format!(
            "ACK {} {}: Moved the import; tests pass.",
            told.key, sent.key
        ),
    );
    assert!(
        peers.pending(RECIPIENT).is_empty(),
        "both keys retire: {heard}"
    );
    let [reply] = peers
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("only the peer message earns one reply: {pending:?}"));
    assert_eq!(reply.from, Some(recipient));
    assert_eq!(
        reply.control.as_deref(),
        Some(wire::peer_reply_control(&told.key, wire::PeerAnswer::Ack).as_str())
    );
    assert_eq!(
        reply.text,
        format!("ACK {}: Moved the import; tests pass.", told.key)
    );
    let seal: steer::Key = "1kQnrzNZ".parse().expect("fixture seal");
    let rendered = steer::inbox::render_pending(&seal, &[reply.clone()], |_, _| None);
    let replied = rendered
        .lines()
        .find(|line| line.starts_with(&format!("[PEER REPLY lane {recipient}]")));
    assert!(
        replied.is_some_and(|line| line.contains(told.key.as_str())),
        "the sender hears a reply naming the key it asked under: {rendered}"
    );

    let mut diagnostics = Vec::new();
    steering::dispose_at_startup(&peers.bus, &peers.paths, peers.epoch, &mut diagnostics)
        .expect("the restart sweep runs");
    assert_eq!(peers.pending(SENDER).len(), 1, "a restart repeats no reply");

    peers.answer(
        SENDER,
        &[&reply.key],
        &format!("ACK {}: Thanks.", reply.key),
    );
    assert!(peers.pending(SENDER).is_empty(), "the reply retires");
    assert!(
        peers.pending(RECIPIENT).is_empty(),
        "acknowledging a reply sends nothing back"
    );
    let snapshot = peers.bus.fact_snapshot().expect("authority reads");
    let replies = snapshot
        .facts()
        .iter()
        .filter_map(|envelope| {
            let reply = envelope.fact.read::<wire::body::SteerReplied>()?;
            Some((envelope.actor.clone(), reply.answered))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replies,
        [(Actor::Lane(recipient), told.key.clone())],
        "one reply, journaled as the answering lane, for the peer message alone"
    );

    peers.finish();
}

#[test]
fn a_lane_inbox_keeps_its_retired_direction_with_fanout_beside_live_direction() {
    let peers = Peers::new("steer-inbox-retired");
    let recipient = test_uuid(RECIPIENT);
    let (outcome, _) = peers.tell(60, RECIPIENT, "the parser moved");
    let Outcome::Reply(Reply {
        body: CommandAnswer::Directive(told),
        ..
    }) = outcome
    else {
        panic!("a peer message is durable: {outcome:?}");
    };
    let sent = peers.send(
        62,
        RECIPIENT,
        61,
        "> quoted context\nkeep the parser where it is",
    );
    let canceled = peers.cancel(63, sent.directive);
    assert!(matches!(canceled, Outcome::Reply(_)), "{canceled:?}");

    let inbox = peers.inbox(64, Actor::Lane(recipient), true);
    let [row] = inbox.lanes.as_slice() else {
        panic!("a lane principal reads only its own lane: {inbox:?}")
    };
    assert_eq!(row.lane, recipient);
    let [live] = row.pending.as_slice() else {
        panic!("the peer message is still live: {row:?}")
    };
    assert_eq!((live.key.as_str(), live.fanout), (told.key.as_str(), None));
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
        retired.preview.as_deref(),
        Some("keep the parser where it is")
    );
    assert!(retired.retired >= retired.authored);

    let operator = peers.inbox(65, Actor::Operator, false);
    assert!(
        operator.lanes.iter().all(|lane| lane.retired.is_empty()),
        "retirements are read only on request"
    );
    let operator = peers.inbox(66, Actor::Operator, true);
    let others = operator
        .lanes
        .iter()
        .filter(|lane| lane.lane != recipient)
        .flat_map(|lane| &lane.retired)
        .count();
    assert_eq!(others, 0, "a retirement belongs only to its own lane");

    peers.finish();
}

#[test]
fn a_tell_is_refused_from_the_operator_to_the_sender_and_to_an_unseated_lane() {
    let peers = Peers::new("steer-tell-refused");
    let tell = wire::intent::SteerTell {
        lane: test_uuid(RECIPIENT),
        message: "a peer message".to_owned(),
    };
    let Err(wire::CommandRejection::Refusal(refusal)) = peers.admit(
        CommandKind::SteerTell,
        50,
        Actor::Operator,
        peers.seat(RECIPIENT),
        &tell,
    ) else {
        panic!("the operator's voice is steer, never tell");
    };
    assert_eq!(refusal.reason(), "command::authority");

    for (id, to, reason) in [
        (51, SENDER, "steer::tell-self"),
        (52, VACANT, "steer::lane-unseated"),
        (53, 99, "steer::lane-unseated"),
    ] {
        let (outcome, effects) = peers.tell(id, to, "a peer message");
        assert!(
            matches!(&outcome, Outcome::Refusal(refusal) if refusal.reason() == reason),
            "{reason}: {outcome:?}"
        );
        if reason == "steer::lane-unseated" {
            let Outcome::Refusal(wire::Refusal::Repairable { command, .. }) = &outcome else {
                panic!("an unseated tell must point to lane discovery: {outcome:?}");
            };
            assert_eq!(command.arguments(), ["lane", "list"]);
        }
        assert!(
            effects.inbox_delivery.is_none(),
            "{reason} delivers nothing"
        );
    }
    let snapshot = peers.bus.fact_snapshot().expect("authority reads");
    assert!(
        snapshot
            .facts()
            .iter()
            .all(|envelope| envelope.fact.read::<wire::body::SteerSent>().is_none()),
        "a refused tell journals nothing"
    );

    peers.finish();
}

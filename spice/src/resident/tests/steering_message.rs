//! An occupant's authored MSG packet publishes one peer message to each named
//! seated occupant exactly once per source packet, and names each address it
//! did not reach.

use std::collections::BTreeSet;

use wire::OccupantAddress;

use super::*;

const SENDER: u8 = 2;
const FIRST: u8 = 4;
const SECOND: u8 = 6;
const THIRD: u8 = 8;

struct Lanes {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
    worktree: String,
}

impl Lanes {
    /// Three seated lanes, each occupant born on its own lane.
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
        let lanes = Self {
            root,
            paths,
            bus,
            epoch,
            worktree,
        };
        for lane in [SENDER, FIRST, SECOND] {
            lanes.seat(lane);
        }
        lanes
    }

    /// Seat one more lane whose occupant is born on it now.
    fn seat(&self, lane: u8) {
        seed_vendor_lane_in_worktree(
            &self.bus,
            self.epoch,
            test_uuid(lane),
            test_uuid(lane + 1),
            &self.worktree,
            "omp",
        );
    }

    fn cursor(&self) -> u64 {
        self.bus
            .task_snapshot()
            .expect("snapshot reads")
            .cursor()
            .expect("the fixture journal has facts")
            .get()
    }

    fn address(&self, lane: u8) -> OccupantAddress {
        self.bus
            .task_snapshot()
            .expect("snapshot reads")
            .facts()
            .occupant_address(test_uuid(lane), test_uuid(lane + 1))
            .expect("the fixture occupant has one birth")
    }

    fn tape(&self, lane: u8) -> tape::Tape {
        tape::Tape::open(
            self.paths
                .tape
                .join(test_uuid(lane).to_string())
                .join("omp"),
        )
        .expect("the lane tape opens")
    }

    /// Land one complete streamed text block, and its settled twin, on a
    /// lane's tape, returning the streamed record's locus.
    fn author(&self, lane: u8, text: &str) -> listen::Locus {
        let tape = self.tape(lane);
        let streamed = serde_json::json!({
            "type": "message_update",
            "message": {"role": "assistant"},
            "assistantMessageEvent": {"type": "text_end", "contentIndex": 0, "content": text},
        });
        let locus = tape
            .tee(format!("{streamed}\n").as_bytes())
            .expect("the streamed block is durable");
        let settled = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stopReason": "stop",
            },
        });
        tape.tee(format!("{settled}\n").as_bytes())
            .expect("the settled message is durable");
        locus
    }

    /// Read every record on a lane's tape, as a sweep would.
    fn records(&self, lane: u8) -> (CanonicalUuid, Vec<listen::Record>) {
        let root = self
            .paths
            .tape
            .join(test_uuid(lane).to_string())
            .join("omp");
        let scan = listen::scan(&root, listen::Dialect::Omp, None).expect("the tape scans");
        let source = self
            .tape(lane)
            .source_id()
            .expect("the tape has a generation")
            .as_str()
            .parse()
            .expect("the generation is a UUID");
        (source, scan.records)
    }

    /// Hear every record on the lane's tape as authored by `author`.
    fn hear(
        &self,
        lane: u8,
        author: CanonicalUuid,
    ) -> (
        Vec<Result<steering::Published, steering::Unpublished>>,
        String,
    ) {
        let (source, records) = self.records(lane);
        let mut outcomes = Vec::new();
        let mut diagnostics = Vec::new();
        for record in records {
            let listen::RecordPayload::Events(events) = record.payload else {
                continue;
            };
            let events = events
                .into_iter()
                .map(|sourced| sourced.event)
                .collect::<Vec<_>>();
            outcomes.extend(steering::publish_authored(
                &self.bus,
                &self.paths,
                self.epoch,
                &steer::Config::default(),
                &steering::AuthoredRecord {
                    lane: test_uuid(lane),
                    author,
                    source,
                    locus: record.locus,
                    events: &events,
                },
                steering::Refusals::Record,
                &mut diagnostics,
            ));
        }
        (
            outcomes,
            String::from_utf8(diagnostics).expect("diagnostics are UTF-8"),
        )
    }

    fn messages(&self) -> Vec<(wire::Envelope<wire::AnyFact>, wire::body::SteerMessaged)> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| {
                let body = envelope.fact.read::<wire::body::SteerMessaged>()?;
                Some((envelope.clone(), body))
            })
            .collect()
    }

    fn pending(&self, lane: u8) -> Vec<wire::intent::PendingDirective> {
        let request = wire::intent::SteerInbox {
            lane: None,
            retired: false,
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(90),
            epoch: self.epoch,
            kind: CommandKind::SteerInbox,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("inbox request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Lane(test_uuid(lane)),
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
        let [row] = inbox.lanes.as_slice() else {
            panic!("a lane principal reads only its own lane: {inbox:?}")
        };
        row.pending.clone()
    }

    fn vacate(&self, lane: u8) {
        let lane = test_uuid(lane);
        let observed = current_residency(&self.bus, self.epoch).expect("residency reads");
        let mut vacant = observed.body().clone();
        let row = vacant
            .lanes
            .iter_mut()
            .find(|row| row.lane == lane)
            .expect("fixture lane exists");
        let occupant = row.occupant.take().expect("the lane is seated");
        row.disposition = "vacant".to_owned();
        let mut ending = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("ending write starts");
        crate::resident::shutdown::guard_residency(&mut ending, &observed);
        let exited = ending
            .fact(
                wire::body::LaneExited {
                    lane,
                    occupant,
                    code: 0,
                },
                FactCause::Existing(observed.position()),
            )
            .expect("exit fact is valid");
        ending.residency_head(exited, move |_| vacant);
        commit(&self.bus, ending).expect("vacancy commits");
    }

    /// Start another occupant on a vacated lane, born there now.
    fn renew(&self, lane: u8, occupant: CanonicalUuid) {
        let lane = test_uuid(lane);
        let observed = current_residency(&self.bus, self.epoch).expect("residency reads");
        let mut renewed = observed.body().clone();
        let row = renewed
            .lanes
            .iter_mut()
            .find(|row| row.lane == lane)
            .expect("fixture lane exists");
        assert!(row.occupant.is_none(), "the lane is vacant");
        row.occupant = Some(occupant);
        row.lineage.push(occupant);
        row.disposition = "occupied".to_owned();
        let mut starting = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("starting write starts");
        crate::resident::shutdown::guard_residency(&mut starting, &observed);
        let started = starting
            .fact(
                wire::body::LaneStarted {
                    lane,
                    occupant,
                    epoch: self.epoch,
                    group: 123,
                    fresh_session: None,
                },
                FactCause::Existing(observed.position()),
            )
            .expect("start fact is valid");
        starting.residency_head(started, move |_| renewed);
        commit(&self.bus, starting).expect("renewal commits");
    }

    /// Declare the lane's seated occupant's session at the tape's current end.
    fn declare(&self, lane: u8) -> listen::Locus {
        let declaration = self
            .tape(lane)
            .tee(br#"{"type":"session","id":"session-1","version":3,"timestamp":"2030-04-12T18:44:09.271Z","cwd":"/"}
"#)
            .expect("the declaration is durable");
        let observed = current_residency(&self.bus, self.epoch).expect("residency reads");
        let mut declared = observed.body().clone();
        let row = declared
            .lanes
            .iter_mut()
            .find(|row| row.lane == test_uuid(lane))
            .expect("declared lane exists");
        let session = wire::LaneSession {
            lane: row.lane,
            occupant: row.occupant.expect("the declaring lane is seated"),
            worktree: row.worktree.clone(),
            vendor: row.vendor.clone(),
            launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
            epoch: self.epoch,
            stream: "stdout".to_owned(),
            identity: "session-1".to_owned(),
            locus: wire::TapeLocus {
                member: declaration.member,
                offset: declaration.offset,
                length: declaration.length,
            },
        };
        row.session = Some(session.clone());
        let mut write = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("declaration write starts");
        crate::resident::shutdown::guard_residency(&mut write, &observed);
        let (source, _) = self.records(lane);
        let fact = write
            .fact(
                crate::resident::lane_surface::session_fact(&session, source),
                FactCause::Existing(observed.position()),
            )
            .expect("declaration fact is valid");
        write.residency_head(fact, move |_| declared);
        commit(&self.bus, write).expect("declaration commits");
        declaration
    }

    fn recover(&self) {
        steering::publish_at_startup(
            &self.bus,
            &self.paths,
            self.epoch,
            &steer::Config::default(),
            &mut Vec::new(),
        )
        .expect("recovery reads");
    }

    /// Replay startup's transcript recovery, as a restarted resident does.
    fn restart(&self) {
        crate::resident::ingest_surface::recover_transcripts(
            &self.bus,
            &self.paths,
            self.epoch,
            &steer::Config::default(),
            &mut Vec::new(),
        )
        .expect("startup recovery replays");
    }

    /// Land one assistant message on a lane's tape and hear its answers.
    fn answer(&self, lane: u8, keys: &[&str], prose: &str) {
        self.land(
            lane,
            keys,
            serde_json::json!([{"type": "text", "text": prose}]),
            "stop",
        );
    }

    /// Land one assistant message whose tool calls declare `intents` and
    /// hear its answers.
    fn intend(&self, lane: u8, keys: &[&str], intents: &[&str]) {
        let calls = intents
            .iter()
            .enumerate()
            .map(|(index, intent)| {
                serde_json::json!({
                    "type": "toolCall",
                    "id": format!("call-{index}"),
                    "name": "read",
                    "arguments": {},
                    "intent": intent,
                })
            })
            .collect();
        self.land(lane, keys, serde_json::Value::Array(calls), "toolUse");
    }

    fn land(&self, lane: u8, keys: &[&str], content: serde_json::Value, stop: &str) {
        let message = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": content,
                "stopReason": stop,
            },
        });
        self.tape(lane)
            .tee(format!("{message}\n").as_bytes())
            .expect("the answer is durable");
        let keys = keys
            .iter()
            .map(|key| key.parse().expect("fixture key"))
            .collect();
        let vendor: wire::Name = "omp".parse().expect("fixture vendor");
        steering::dispose_heard(
            &self.bus,
            &self.paths,
            self.epoch,
            test_uuid(lane),
            &vendor,
            &keys,
            &mut Vec::new(),
        );
    }

    fn dead_letter_removed_lanes(&self) {
        steering::dead_letter_removed_lanes(&self.bus, &mut Vec::new())
            .expect("removed lanes read");
    }

    /// Remove one vacant lane from the residency.
    fn remove(&self, lane: u8) {
        let lane = test_uuid(lane);
        let observed = current_residency(&self.bus, self.epoch).expect("residency reads");
        let mut remaining = observed.body().clone();
        let row = remaining
            .lanes
            .iter()
            .find(|row| row.lane == lane)
            .cloned()
            .expect("fixture lane exists");
        assert!(row.occupant.is_none(), "only a vacant lane is removed");
        remaining.lanes.retain(|row| row.lane != lane);
        let mut removal = write(Actor::Operator, self.epoch, Producer::ResidentRecognition)
            .expect("removal write starts");
        crate::resident::shutdown::guard_residency(&mut removal, &observed);
        let removed = removal
            .fact(
                wire::body::LaneRemoved {
                    lane,
                    team: row.team,
                    worktree: row.worktree,
                },
                FactCause::Existing(observed.position()),
            )
            .expect("removal fact is valid");
        removal.residency_head(removed, move |_| remaining);
        commit(&self.bus, removal).expect("removal commits");
    }

    /// Ask, as the operator, to recover one parked directive.
    fn recover_directive(&self, directive: CanonicalUuid) -> Outcome<CommandAnswer> {
        let recover = wire::intent::DirectiveRef { directive };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(91),
            epoch: self.epoch,
            kind: CommandKind::SteerRecover,
            expected: vec![Witness::Recoverable { steer: directive }],
            body: serde_json::to_value(&recover).expect("recovery encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator recovery admits");
        steering::recover(
            &self.bus,
            self.epoch,
            &steer::Config::default(),
            &command,
            &recover,
            &mut CommandEffects::default(),
        )
    }

    /// Judge the lane's newest streamed record live, as its seated author's.
    fn hear_newest(
        &self,
        lane: u8,
    ) -> (
        listen::Locus,
        Vec<Result<steering::Published, steering::Unpublished>>,
    ) {
        let (source, records) = self.records(lane);
        let newest = &records[records.len() - 2];
        let listen::RecordPayload::Events(events) = &newest.payload else {
            panic!("the streamed record decodes");
        };
        let events = events
            .iter()
            .map(|sourced| sourced.event.clone())
            .collect::<Vec<_>>();
        let outcomes = steering::publish_authored(
            &self.bus,
            &self.paths,
            self.epoch,
            &steer::Config::default(),
            &steering::AuthoredRecord {
                lane: test_uuid(lane),
                author: test_uuid(lane + 1),
                source,
                locus: newest.locus,
                events: &events,
            },
            steering::Refusals::Record,
            &mut Vec::new(),
        );
        (newest.locus, outcomes)
    }

    fn finish(self) {
        self.bus.verify().expect("the authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

/// The streamed block is the packet's one source: its settled twin, a reread
/// of the same record, and an identical body at another locus each behave
/// as what they are.
#[test]
fn a_packet_publishes_its_exact_body_once_to_every_named_seated_occupant() {
    let lanes = Lanes::new("msg-publish");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    let packet = format!("Status first.\nMSG {first} {second}: rebase onto main\nthen rerun.");
    let authored = lanes.author(SENDER, &packet);

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        outcomes,
        [Ok(sent(&[FIRST, SECOND], &[]))],
        "only the streamed block is a source: {diagnostics}"
    );
    let messages = lanes.messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].0.seq.get() + 1,
        messages[1].0.seq.get(),
        "both messages commit in one write"
    );
    for ((envelope, body), (lane, occupant)) in messages
        .iter()
        .zip([(FIRST, FIRST + 1), (SECOND, SECOND + 1)])
    {
        assert_eq!(envelope.actor, Actor::Lane(test_uuid(SENDER)));
        assert_eq!(body.lane, test_uuid(lane));
        assert_eq!(body.occupant, test_uuid(occupant));
        assert_eq!(body.from, test_uuid(SENDER));
        assert_eq!(body.sender, test_uuid(SENDER + 1));
        assert_eq!(body.text, "rebase onto main\nthen rerun.");
        assert_eq!(
            (body.locus.member, body.locus.offset, body.locus.line),
            (authored.member, authored.offset, 2)
        );
        let pending = lanes.pending(lane);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].text, "rebase onto main\nthen rerun.");
        assert_eq!(pending[0].from, Some(test_uuid(SENDER)));
    }

    let (reread, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(reread, [Ok(steering::Published::Already)]);
    assert_eq!(lanes.messages().len(), 2, "a reread sends nothing");

    lanes.author(SENDER, &packet);
    let (outcomes, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        outcomes,
        [
            Ok(steering::Published::Already),
            Ok(sent(&[FIRST, SECOND], &[])),
        ],
        "the same body authored again is another message"
    );
    assert_eq!(lanes.pending(FIRST).len(), 2);
    lanes.finish();
}

/// Each seated recipient hears the packet and each unavailable address is
/// named with why; a packet no recipient can hear, or one naming its own
/// author or an address twice, publishes nothing.
#[test]
fn a_packet_reaches_each_seated_recipient_and_names_each_unavailable_address() {
    let lanes = Lanes::new("msg-refuse");
    let (sender, first, second) = (
        lanes.address(SENDER),
        lanes.address(FIRST),
        lanes.address(SECOND),
    );
    lanes.vacate(SECOND);
    let unrecorded = OccupantAddress::from(wire::Seq::new(9_999_999).expect("nonzero position"));
    lanes.author(SENDER, &format!("MSG {first} {second}: departed"));
    lanes.author(SENDER, &format!("MSG {second} {unrecorded}: nobody"));
    lanes.author(SENDER, &format!("MSG {first} {sender}: myself"));
    lanes.author(SENDER, &format!("MSG {first} {first}: twice"));

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    let refused = |reason: &str, detail: &str| {
        Err(steering::Unpublished::Refused {
            reason: reason.to_owned(),
            detail: detail.to_owned(),
        })
    };
    assert_eq!(
        outcomes[..2],
        [
            Ok(sent(
                &[FIRST],
                &[&format!("{second} address::expired (its lane is vacant)")]
            )),
            refused(
                "msg::recipient-unavailable",
                &format!(
                    "{second} address::expired (its lane is vacant); {unrecorded} address::unrecorded"
                ),
            ),
        ],
        "{diagnostics}"
    );
    let reasons = outcomes[2..]
        .iter()
        .map(|outcome| match outcome {
            Err(steering::Unpublished::Refused { reason, .. }) => reason.as_str(),
            other => panic!("the packet refuses whole: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(reasons, ["msg::recipient-self", "msg::recipient-repeated"]);
    assert_eq!(delivered(&lanes, FIRST).text, "departed");
    assert_eq!(lanes.messages().len(), 1);

    let (reread, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(
        reread[0],
        Ok(steering::Published::Already),
        "a partly delivered packet is published"
    );
    assert_eq!(lanes.messages().len(), 1, "a reread sends nothing");
    lanes.finish();
}

/// The commit re-proves the residency the packet resolved against, so a seat
/// change in between refuses everything; the next resolution judges the
/// residency as it now stands.
#[test]
fn a_residency_move_between_resolution_and_commit_publishes_to_nobody() {
    let lanes = Lanes::new("msg-race");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    let stale = lanes.bus.task_snapshot().expect("snapshot reads");
    lanes.vacate(SECOND);
    let packet = steering::Packet {
        lane: test_uuid(SENDER),
        author: test_uuid(SENDER + 1),
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 0,
            length: 10,
            event: 0,
            line: 1,
            column: 1,
        },
        recipients: vec![first, second],
        body: "raced",
    };
    let config = steer::Config::default();

    let raced = steering::publish_packet(&lanes.bus, lanes.epoch, &config, &packet, &stale);
    assert!(
        matches!(
            &raced,
            Err(steering::Unpublished::Refused { reason, .. }) if reason == "steer::residency-moved"
        ),
        "{raced:?}"
    );
    assert!(lanes.messages().is_empty());

    let current = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(&lanes.bus, lanes.epoch, &config, &packet, &current);
    assert_eq!(
        resolved,
        Ok(sent(
            &[FIRST],
            &[&format!("{second} address::expired (its lane is vacant)")]
        ))
    );
    assert_eq!(lanes.messages().len(), 1);
    lanes.finish();
}

/// An expired address receives nothing, and its author learns what its birth
/// lane seats in the view the packet resolved against: the renewed occupant's
/// own address, or a vacancy.
#[test]
fn an_expired_address_names_what_its_lane_seats_now() {
    let lanes = Lanes::new("msg-renewed");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.vacate(SECOND);
    lanes.vacate(FIRST);
    let renewed = test_uuid(40);
    lanes.renew(SECOND, renewed);
    let current = lanes
        .bus
        .task_snapshot()
        .expect("snapshot reads")
        .facts()
        .occupant_address(test_uuid(SECOND), renewed)
        .expect("the renewed occupant has one birth");
    assert_ne!(current, second);
    let packet = steering::Packet {
        lane: test_uuid(SENDER),
        author: test_uuid(SENDER + 1),
        locus: wire::body::TranscriptLocus {
            source: Some(test_uuid(30)),
            member: 1,
            offset: 0,
            length: 10,
            event: 0,
            line: 1,
            column: 1,
        },
        recipients: vec![first, second, current],
        body: "renewed",
    };
    let view = lanes.bus.task_snapshot().expect("snapshot reads");
    let resolved = steering::publish_packet(
        &lanes.bus,
        lanes.epoch,
        &steer::Config::default(),
        &packet,
        &view,
    );

    assert_eq!(
        resolved,
        Ok(sent(
            &[SECOND],
            &[
                &format!("{first} address::expired (its lane is vacant)"),
                &format!("{second} address::expired (its lane seats {current})"),
            ]
        ))
    );
    let messages = lanes.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].1.occupant, renewed,
        "only the address naming the renewed occupant reaches it"
    );
    lanes.finish();
}

/// Sender identity is the record's capture binding, and a packet answers
/// nothing: the author's own pending direction stays live.
#[test]
fn a_packet_is_attributed_to_its_author_and_retires_nothing() {
    let lanes = Lanes::new("msg-author");
    let first = lanes.address(FIRST);
    let direction = steering::send(
        &lanes.bus,
        lanes.epoch,
        &steer::Config::default(),
        &Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(40),
            epoch: lanes.epoch,
            kind: CommandKind::SteerSend,
            expected: vec![Witness::Residency {
                lane: test_uuid(SENDER),
                occupant: Some(test_uuid(SENDER + 1)),
                epoch: lanes.epoch,
            }],
            body: serde_json::json!({
                "fanout": test_uuid(41),
                "lane": test_uuid(SENDER),
                "message": "keep going",
                "attachments": [],
            }),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: lanes.epoch,
        })
        .expect("operator send admits"),
        &wire::intent::SteerSend {
            fanout: test_uuid(41),
            lane: test_uuid(SENDER),
            message: "keep going".to_owned(),
            attachments: Vec::new(),
        },
        &mut CommandEffects::default(),
    );
    let Outcome::Reply(Reply {
        body: CommandAnswer::Directive(direction),
        ..
    }) = direction
    else {
        panic!("operator direction is durable: {direction:?}");
    };
    lanes.author(SENDER, &format!("MSG {first}: about {}", direction.key));

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(outcomes, [Ok(sent(&[FIRST], &[]))], "{diagnostics}");
    let keys = lanes
        .pending(SENDER)
        .into_iter()
        .map(|pending| pending.key)
        .collect::<BTreeSet<_>>();
    assert_eq!(keys, BTreeSet::from([direction.key.clone()]));

    // A record heard under an occupant with no birth on the lane is no one's.
    lanes.author(SENDER, &format!("MSG {first}: impostor"));
    let (outcomes, _) = lanes.hear(SENDER, test_uuid(FIRST + 1));
    assert!(
        matches!(
            outcomes.last(),
            Some(Err(steering::Unpublished::Refused { reason, .. })) if reason == "msg::sender-unproved"
        ),
        "{outcomes:?}"
    );
    assert_eq!(lanes.messages().len(), 1);
    lanes.finish();
}

/// Startup rereads what the lane's declared occupant wrote after its session
/// declaration, publishing what a crash left behind and nothing twice.
#[test]
fn startup_recovery_publishes_only_unpublished_packets_after_the_declaration() {
    let lanes = Lanes::new("msg-recover");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: before the session"));
    let declaration = lanes.declare(SENDER);

    lanes.author(SENDER, &format!("MSG {first}: heard live"));
    let (live, _) = lanes.hear_newest(SENDER);
    assert!(live.offset > declaration.offset);
    lanes.author(SENDER, &format!("MSG {first}: lost to a crash"));

    lanes.recover();
    lanes.recover();
    let texts = lanes
        .messages()
        .into_iter()
        .map(|(_, body)| (body.text, body.sender))
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            ("heard live".to_owned(), test_uuid(SENDER + 1)),
            ("lost to a crash".to_owned(), test_uuid(SENDER + 1)),
        ],
    );
    lanes.finish();
}

/// A refusal is the packet's settled judgment: once the residency refused it,
/// recovery publishes nothing even after the refused address names a seated
/// occupant.
#[test]
fn a_refused_packet_stays_unpublished_after_its_refusal_stops_holding() {
    let lanes = Lanes::new("msg-withheld");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    // Each seating writes the same facts, so the next birth lands as far past
    // the cursor the seating starts from as the last one did.
    let stride = second.birth().get() - first.birth().get();
    let lag = lanes.cursor() - second.birth().get();
    lanes.declare(SENDER);
    // Judging the packet journals its one refusal before the next seating.
    let unborn = OccupantAddress::from(
        wire::Seq::new(lanes.cursor() + 1 + stride - lag).expect("nonzero position"),
    );
    lanes.author(SENDER, &format!("MSG {unborn}: not yet born"));

    let (_, outcomes) = lanes.hear_newest(SENDER);
    assert!(
        matches!(
            outcomes.as_slice(),
            [Err(steering::Unpublished::Refused { reason, .. })] if reason == "msg::recipient-unavailable"
        ),
        "{outcomes:?}"
    );
    lanes.seat(THIRD);
    assert_eq!(
        lanes.address(THIRD),
        unborn,
        "the refused address now seats"
    );

    lanes.recover();
    lanes.recover();
    assert!(lanes.messages().is_empty());
    assert!(lanes.pending(THIRD).is_empty());
    lanes.finish();
}

fn sent(lanes: &[u8], unreached: &[&str]) -> steering::Published {
    steering::Published::Sent {
        lanes: lanes.iter().map(|lane| test_uuid(*lane)).collect(),
        unreached: unreached.iter().map(|entry| (*entry).to_owned()).collect(),
    }
}

/// The delivery a recipient hears names the exact body, the sending
/// occupant's address, and its own key, and never wakes the lane.
fn delivered(lanes: &Lanes, lane: u8) -> wire::intent::PendingDirective {
    let [message] = lanes
        .pending(lane)
        .try_into()
        .unwrap_or_else(|pending| panic!("one MSG is pending: {pending:?}"));
    assert!(!message.wakes, "a peer message never wakes its lane");
    assert_eq!(
        message.sender,
        Some(wire::intent::PeerSender {
            occupant: test_uuid(SENDER + 1),
            address: Some(lanes.address(SENDER)),
        })
    );
    let seal: steer::Key = "1kQnrzNZ".parse().expect("fixture seal");
    let rendered = steer::inbox::render_pending(&seal, &[message.clone()], |_, _| None);
    for part in [
        message.key.as_str(),
        message.text.as_str(),
        &lanes.address(SENDER).to_string(),
    ] {
        assert!(rendered.contains(part), "{part} is delivered: {rendered}");
    }
    message
}

/// Every fanout recipient owns its key: each answer retires only its own
/// directive and relays its clause once to the sending lane, whose answer to
/// that reply sends nothing back.
#[test]
fn each_recipient_answer_retires_its_own_msg_and_replies_once_to_the_sending_lane() {
    let lanes = Lanes::new("msg-answer");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.author(SENDER, &format!("MSG {first} {second}: rebase onto main"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let (to_first, to_second) = (delivered(&lanes, FIRST), delivered(&lanes, SECOND));
    assert_eq!(to_first.text, "rebase onto main");
    assert_ne!(to_first.key, to_second.key);
    lanes.declare(FIRST);
    lanes.declare(SECOND);

    lanes.answer(
        FIRST,
        &[&to_first.key],
        &format!("ACK {}: Rebased.", to_first.key),
    );
    assert!(lanes.pending(FIRST).is_empty());
    assert_eq!(
        lanes.pending(SECOND),
        [to_second.clone()],
        "a sibling recipient's key stays live"
    );
    let [ack] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("one reply reaches the sender: {pending:?}"));
    assert_eq!(ack.from, Some(test_uuid(FIRST)));
    assert_eq!(ack.text, "Rebased.");
    assert_eq!(
        ack.control.as_deref(),
        Some(wire::peer_reply_control(&to_first.key, wire::PeerAnswer::Ack).as_str())
    );
    assert!(!ack.wakes, "a peer reply never wakes its lane");
    assert_eq!(
        ack.sender,
        Some(wire::intent::PeerSender {
            occupant: test_uuid(FIRST + 1),
            address: Some(first),
        }),
        "a reply names the exact occupant that answered"
    );

    lanes.answer(
        SECOND,
        &[&to_second.key],
        &format!("NAK {}: Main is red.", to_second.key),
    );
    assert!(lanes.pending(SECOND).is_empty());
    let replies = lanes.pending(SENDER);
    let nak = replies
        .iter()
        .find(|reply| reply.from == Some(test_uuid(SECOND)))
        .unwrap_or_else(|| panic!("the refusal replies too: {replies:?}"));
    assert_eq!(nak.text, "Main is red.");
    assert_eq!(
        nak.sender.map(|sender| sender.address),
        Some(Some(second)),
        "each reply names its own answerer"
    );
    assert_eq!(
        nak.control.as_deref(),
        Some(wire::peer_reply_control(&to_second.key, wire::PeerAnswer::Nak).as_str())
    );

    let keys = replies
        .iter()
        .map(|reply| reply.key.as_str())
        .collect::<Vec<_>>();
    lanes.answer(SENDER, &keys, &format!("ACK {}: Thanks.", keys.join(" ")));
    assert!(lanes.pending(SENDER).is_empty());
    assert!(lanes.pending(FIRST).is_empty() && lanes.pending(SECOND).is_empty());
    let replied = lanes
        .bus
        .fact_snapshot()
        .expect("authority reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::body::SteerReplied>())
        .map(|reply| reply.answered)
        .collect::<Vec<_>>();
    assert_eq!(
        replied,
        [to_first.key, to_second.key],
        "one reply per answered MSG, none for an answered reply"
    );
    lanes.finish();
}

/// A reply follows lane continuity: the lane of a sender that has left hears
/// the answer at its next seat, and a restart neither drops nor repeats it.
#[test]
fn a_reply_to_a_departed_sender_waits_on_its_lane_across_a_restart() {
    let lanes = Lanes::new("msg-reply-vacant");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: is the parser settled?"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    lanes.vacate(SENDER);
    let message = delivered(&lanes, FIRST);

    lanes.answer(FIRST, &[&message.key], &format!("ACK {}", message.key));
    assert!(lanes.pending(FIRST).is_empty(), "the MSG retires");
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("the vacant lane holds one reply: {pending:?}"));
    assert_eq!(
        reply.control.as_deref(),
        Some(wire::peer_reply_control(&message.key, wire::PeerAnswer::Ack).as_str())
    );
    assert!(!reply.wakes, "a peer reply never wakes its vacant lane");
    assert_eq!(reply.text, "", "a bare header relays no clause");

    let mut diagnostics = Vec::new();
    steering::dispose_at_startup(&lanes.bus, &lanes.paths, lanes.epoch, &mut diagnostics)
        .expect("the restart sweep runs");
    assert_eq!(
        lanes.pending(SENDER),
        [reply],
        "a restart neither drops nor repeats the waiting reply"
    );
    lanes.finish();
}

/// An MSG is bound to its recipient's logical lane: it keeps its one key,
/// body, and deadline through that lane's vacancy and renewal, the successor
/// answers it, the reply names the occupant that answered, and an answered
/// MSG never returns at a later renewal. Its quote still names the occupant
/// that wrote it after that sender leaves.
#[test]
fn an_msg_follows_its_lane_to_a_renewed_occupant_with_one_key_and_deadline() {
    let lanes = Lanes::new("msg-lane-bound");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    lanes.vacate(SENDER);
    let message = delivered(&lanes, FIRST);
    let resolver_source = lanes.bus.reader();
    let resolver = spice::quote::Resolver::from_tape(&lanes.paths.tape, &resolver_source);
    let target = wire::QuoteTarget::Steering {
        key: message.key.clone(),
        digest: wire::QuoteTarget::text_digest(&message.text),
    };
    assert!(
        matches!(
            resolver.resolve(&target.to_string(), 0),
            wire::quote::QuoteResolution::Resolved(wire::quote::ResolvedQuote {
                author: wire::QuoteAuthor::Agent { lane, occupant },
                ..
            }) if lane == test_uuid(SENDER) && occupant == test_uuid(SENDER + 1)
        ),
        "a quote names the occupant that wrote the MSG"
    );
    drop(resolver);
    drop(resolver_source);

    lanes.vacate(FIRST);
    assert_eq!(
        lanes.pending(FIRST),
        [message.clone()],
        "a vacant lane keeps its MSG"
    );
    let successor = test_uuid(41);
    lanes.renew(FIRST, successor);
    let before = lanes.cursor();
    lanes.dead_letter_removed_lanes();
    assert_eq!(
        lanes.cursor(),
        before,
        "a lane still resident parks nothing"
    );
    lanes.restart();
    lanes.restart();
    assert_eq!(
        lanes.pending(FIRST),
        [message.clone()],
        "the successor inherits the same key, body, and deadline through every replay"
    );
    assert_eq!(lanes.messages().len(), 1, "replay publishes nothing again");

    lanes.declare(FIRST);
    lanes.answer(
        FIRST,
        &[&message.key],
        &format!("ACK {}: Holding.", message.key),
    );
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the successor's answer retires the inherited key"
    );
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("one reply reaches the sending lane: {pending:?}"));
    let answerer = lanes
        .bus
        .task_snapshot()
        .expect("snapshot reads")
        .facts()
        .occupant_address(test_uuid(FIRST), successor)
        .expect("the successor has one birth");
    assert_ne!(answerer, first);
    assert_eq!(
        reply.sender,
        Some(wire::intent::PeerSender {
            occupant: successor,
            address: Some(answerer),
        }),
        "the reply names the occupant that answered, not the one first addressed"
    );

    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(43));
    assert!(
        lanes.pending(FIRST).is_empty(),
        "an answered MSG never returns to the lane"
    );
    lanes.finish();
}

/// Tool-call intent is answer evidence, never authorship: a successor's
/// intent header retires the MSG its lane inherited, while an MSG header in
/// an intent publishes nothing.
#[test]
fn a_tool_intent_retires_an_inherited_msg_but_never_publishes_one() {
    let lanes = Lanes::new("msg-tool-intent");
    let first = lanes.address(FIRST);
    let sender = lanes.address(SENDER);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);
    lanes.vacate(FIRST);
    let successor = test_uuid(41);
    lanes.renew(FIRST, successor);
    lanes.declare(FIRST);

    lanes.intend(
        FIRST,
        &[&message.key],
        &[
            &format!("MSG {sender}: a caption is not a packet"),
            &format!("ACK {}: Holding.", message.key),
        ],
    );
    let (published, _) = lanes.hear(FIRST, successor);

    assert!(
        published.is_empty(),
        "an intent publishes nothing: {published:?}"
    );
    assert_eq!(lanes.messages().len(), 1, "only the authored MSG exists");
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the intent's header retires the inherited key"
    );
    let [reply] = lanes
        .pending(SENDER)
        .try_into()
        .unwrap_or_else(|pending| panic!("only the reply reaches the sender: {pending:?}"));
    assert_eq!(reply.sender.map(|sender| sender.occupant), Some(successor));
    lanes.finish();
}

/// A predecessor transcript that cannot be read at exit settles nothing and
/// says so; once readable, the answer it holds retires the key exactly once.
#[test]
fn an_unreadable_predecessor_transcript_settles_nothing_at_exit() {
    let lanes = Lanes::new("msg-unreadable-predecessor");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);
    lanes.declare(FIRST);
    lanes.answer(FIRST, &[], &format!("ACK {}: Holding.", message.key));
    lanes.vacate(FIRST);
    let tape = lanes
        .paths
        .tape
        .join(test_uuid(FIRST).to_string())
        .join("omp");
    let hidden = tape.with_extension("hidden");
    fs::rename(&tape, &hidden).expect("the tape moves aside");
    fs::write(&tape, b"not a tape").expect("an unreadable tape stands in");
    let vendor: wire::Name = "omp".parse().expect("fixture vendor");
    let exit = |diagnostics: &mut Vec<u8>| {
        steering::dispose_at_exit(
            &lanes.bus,
            &lanes.paths,
            lanes.epoch,
            test_uuid(FIRST),
            &vendor,
            diagnostics,
        );
    };

    let mut diagnostics = Vec::new();
    exit(&mut diagnostics);

    assert_eq!(
        lanes.pending(FIRST),
        [message.clone()],
        "an unreadable predecessor settles nothing"
    );
    assert!(lanes.pending(SENDER).is_empty(), "no reply is owed yet");
    assert!(
        String::from_utf8_lossy(&diagnostics).contains(&test_uuid(FIRST).to_string()),
        "the failure is visible"
    );

    fs::remove_file(&tape).expect("the stand-in leaves");
    fs::rename(&hidden, &tape).expect("the tape returns");
    exit(&mut Vec::new());
    exit(&mut Vec::new());
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the readable answer retires"
    );
    assert_eq!(
        lanes.pending(SENDER).len(),
        1,
        "one reply reaches the sender"
    );
    lanes.finish();
}

/// Removing a lane ends its peer duties' destination: each live one parks
/// once with the lane-removed reason, a lane that takes its place inherits
/// nothing, and recovery refuses to revive the duty.
#[test]
fn a_removed_lane_parks_its_msg_and_a_replacement_lane_inherits_nothing() {
    let lanes = Lanes::new("msg-lane-removed");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}: hold the merge"));
    lanes.hear(SENDER, test_uuid(SENDER + 1));
    let message = delivered(&lanes, FIRST);

    lanes.vacate(FIRST);
    lanes.remove(FIRST);
    lanes.seat(THIRD);
    let before = lanes.cursor();
    lanes.dead_letter_removed_lanes();
    let head = lanes
        .bus
        .read(move |read| read.steering_head(message.directive))
        .expect("steering reads")
        .expect("the MSG stays durable");
    assert_eq!(head.body().disposition, wire::Disposition::DeadLettered);
    assert_eq!(head.body().key, message.key, "parking never re-keys");
    let reasons = lanes
        .bus
        .fact_snapshot()
        .expect("authority reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::steering::SteerDeadLettered>())
        .map(|parked| (parked.directive, parked.reason))
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [(message.directive, steering::LANE_REMOVED.to_owned())]
    );
    let parked = lanes.cursor();
    assert!(parked > before);
    lanes.dead_letter_removed_lanes();
    assert_eq!(lanes.cursor(), parked, "a parked MSG parks once");
    assert!(
        lanes.pending(THIRD).is_empty(),
        "a replacement lane inherits nothing"
    );

    let refused = lanes.recover_directive(message.directive);
    assert!(
        matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == steering::LANE_REMOVED),
        "{refused:?}"
    );
    lanes.finish();
}

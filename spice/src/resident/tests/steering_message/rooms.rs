//! A packet's `#TASK` contexts record its exact source bytes once in each
//! named room, and a directed route freezes its room on the delivery.

use super::*;

impl Lanes {
    /// Seed one task an MSG context can name as its room.
    pub(super) fn room(&self, stamp: &str) -> wire::TaskHandle {
        let task: wire::TaskStamp = stamp.parse().expect("fixture stamp");
        let phase: wire::Phase = "todo".parse().expect("fixture phase");
        let creation = wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        );
        let mut seed =
            write(Actor::Spice, self.epoch, Producer::CommandDoor).expect("task write starts");
        let added = seed
            .fact(
                wire::body::TaskAdded {
                    task: task.clone(),
                    title: "Room".to_owned(),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    project: "steer.rooms".to_owned(),
                    tags: Vec::new(),
                    depends: Vec::new(),
                    intent: None,
                    preparation: None,
                    flow: vec![phase.clone()],
                    phase: phase.clone(),
                    revision: 1,
                    creation: creation.clone(),
                },
                FactCause::Root,
            )
            .expect("task fact is valid");
        let born = task.clone();
        seed.task_head(added, move |position| wire::TaskHead {
            preparation: None,
            intent: None,
            authorization: None,
            born,
            queue_origin: position,
            title: "Room".to_owned(),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "steer.rooms".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: "resident:018f82d4-6f59-7d06-92e3-000000000001"
                .parse()
                .expect("fixture origin"),
            creation,
            revision: position,
        });
        seed.workflow_head(task.clone(), added, move |_| wire::WorkflowHead {
            flow: vec![phase.clone()],
            phase,
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
            generation: 0,
        });
        commit(&self.bus, seed).expect("task commits");
        format!("ROOMS-{task}").parse().expect("fixture handle")
    }

    /// Account every record now on a lane's tape, as its sweep would.
    pub(super) fn account(&self, lane: u8) {
        let (source, records) = self.records(lane);
        let last = records.last().expect("the tape has records").locus;
        let head = wire::TranscriptSourceHead {
            lane: test_uuid(lane),
            vendor: "omp".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source,
            next: wire::body::TranscriptPosition {
                member: last.member,
                offset: last.offset + last.length,
            },
        };
        let mut accounting = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("accounting write starts");
        let ingested = accounting
            .fact(
                wire::body::JournalIngested {
                    lane: head.lane,
                    vendor: head.vendor.clone(),
                    stream: head.stream.clone(),
                    source,
                    supersedes: None,
                    from: wire::body::TranscriptPosition {
                        member: 0,
                        offset: 0,
                    },
                    to: head.next,
                    usage: Vec::new(),
                    non_usage_records: 0,
                    residue_records: 0,
                },
                FactCause::Root,
            )
            .expect("accounted batch is valid");
        accounting.transcript_source_head(ingested, move |_| head);
        commit(&self.bus, accounting).expect("accounting commits");
    }

    fn rooms(&self) -> Vec<wire::rooms::RoomMessaged> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| envelope.fact.read::<wire::rooms::RoomMessaged>())
            .collect()
    }
}

/// `#R` records the packet's exact bytes in R and tells only the author;
/// `A#R` also delivers to A with R frozen; a vacant directed route stays
/// named as unreached, and a packet no route reaches still records in R;
/// equal text at another locus records again; a reread writes nothing.
#[test]
fn room_routes_record_each_source_once_and_deliver_directed_routes() {
    let lanes = Lanes::new("msg-room");
    let room = lanes.room("dVrD244Jd");
    let born = room.born().expect("fixture handle carries its birth");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.vacate(SECOND);
    let block =
        format!("MSG #{room}: for the room\n\nMSG {first}#{room} {second}#{room}: for first");
    lanes.author(SENDER, &block);
    lanes.author(SENDER, &format!("MSG #{room}: for the room"));
    lanes.author(SENDER, &format!("MSG {second}#{room}: for no one seated"));
    lanes.account(SENDER);

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    let recorded = |lanes: &[u8], unreached: &[String]| {
        Ok(steering::Published::Sent {
            lanes: lanes.iter().map(|lane| test_uuid(*lane)).collect(),
            unreached: unreached.to_vec(),
            rooms: vec![room.clone()],
        })
    };
    assert_eq!(
        outcomes,
        [
            recorded(&[], &[]),
            recorded(
                &[FIRST],
                &[format!("{second} address::expired (its lane is vacant)")]
            ),
            recorded(&[], &[]),
            recorded(
                &[],
                &[format!("{second} address::expired (its lane is vacant)")]
            ),
        ],
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains(&format!("recorded in {room}")),
        "{diagnostics}"
    );
    let messages = lanes.messages();
    let [(_, message)] = messages.as_slice() else {
        panic!("only the seated directed route delivers: {messages:?}");
    };
    assert_eq!(message.lane, test_uuid(FIRST));
    assert_eq!(message.room, Some(born.clone()));
    assert_eq!(lanes.pending(FIRST).len(), 1);
    let records = lanes.rooms();
    assert_eq!(records.len(), 4, "each packet records once: {records:?}");
    assert!(records.iter().all(|record| record.room == born));
    let bodies = records[..2]
        .iter()
        .map(|record| &block[record.hunk.text.start as usize..record.hunk.text.end as usize])
        .collect::<Vec<_>>();
    assert_eq!(
        bodies,
        ["for the room", "for first"],
        "each hunk names its body"
    );
    assert_eq!(records[0].hunk.text, records[2].hunk.text);
    assert!(
        !records[0].hunk.same_bytes(&records[2].hunk),
        "equal text at another locus is another publication"
    );

    let (reread, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(reread, [const { Ok(steering::Published::Already) }; 4]);
    assert_eq!(lanes.rooms().len(), 4, "a reread records nothing");
    assert_eq!(lanes.messages().len(), 1, "a reread sends nothing");
    lanes.finish();
}

/// A context naming no task refuses the whole packet: no route delivers and
/// no room records.
#[test]
fn an_unknown_context_refuses_every_route() {
    let lanes = Lanes::new("msg-room-unknown");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);
    lanes.author(
        SENDER,
        &format!("MSG {first} #{room} #ROOMS-dVs4wZ9Q3: lost"),
    );
    lanes.account(SENDER);

    let (outcomes, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert!(
        matches!(
            outcomes.as_slice(),
            [Err(steering::Unpublished::Refused { reason, .. })] if reason == "msg::context-unknown"
        ),
        "{outcomes:?}"
    );
    assert!(lanes.messages().is_empty());
    assert!(lanes.rooms().is_empty());
    assert!(lanes.pending(FIRST).is_empty());
    lanes.finish();
}

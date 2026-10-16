//! A packet's `#TASK` contexts record its exact source bytes once in each
//! named room, and a directed route freezes its room on the delivery.

use super::*;

impl Lanes {
    /// Seed one task an MSG context can name as its room.
    pub(super) fn room(&self, stamp: &str) -> wire::TaskHandle {
        self.seed_task(stamp, "Room", None, None, None)
    }

    /// Seed one room task `holder`'s lane holds an active claim on.
    fn held_room(&self, stamp: &str, holder: u8) -> wire::TaskHandle {
        self.seed_task(stamp, "Room", None, None, Some(test_uuid(holder)))
    }

    /// Seed one pending task with this statement, held by `holder` when named.
    pub(super) fn seed_task(
        &self,
        stamp: &str,
        title: &str,
        description: Option<&str>,
        intent: Option<wire::TaskIntent>,
        holder: Option<CanonicalUuid>,
    ) -> wire::TaskHandle {
        let task: wire::TaskStamp = stamp.parse().expect("fixture stamp");
        let phase: wire::Phase = if intent == Some(wire::TaskIntent::Design) {
            "design"
        } else {
            "todo"
        }
        .parse()
        .expect("fixture phase");
        let creation = wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        );
        let mut seed =
            write(Actor::Spice, self.epoch, Producer::CommandDoor).expect("task write starts");
        let added = seed
            .fact(
                wire::body::TaskAdded {
                    task: task.clone(),
                    title: title.to_owned(),
                    description: description.map(str::to_owned),
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    project: "serve.rooms".to_owned(),
                    filing: wire::TaskFiling::Filed,
                    tags: Vec::new(),
                    priority: None,
                    taste: None,
                    depends: Vec::new(),
                    intent,
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
        let title = title.to_owned();
        let description = description.map(str::to_owned);
        seed.task_head(added, move |position| wire::TaskHead {
            preparation: None,
            intent,
            authorization: None,
            evidence: Vec::new(),
            born,
            queue_origin: position,
            title,
            description,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "serve.rooms".to_owned(),
            filing: wire::TaskFiling::Filed,
            tags: Vec::new(),
            priority: None,
            taste: None,
            dependencies: Vec::new(),
            provenance: wire::TaskProvenance::Parentless,
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
        if let Some(lane) = holder {
            self.claim(&mut seed, &task, added, lane);
        }
        commit(&self.bus, seed).expect("task commits");
        format!("ROOMS-{task}").parse().expect("fixture handle")
    }

    /// Close `room`'s anchor on the operator's word, settling it as
    /// `spice task close` does.
    pub(super) fn close_room(&self, room: &wire::TaskHandle) {
        let task = room.born().expect("fixture handle carries its stamp");
        let phase: wire::Phase = "todo".parse().expect("fixture phase");
        let mut close =
            write(Actor::Operator, self.epoch, Producer::CommandDoor).expect("close write starts");
        let closed = close
            .fact(
                wire::body::TaskClosed {
                    task: task.clone(),
                    validation: "settled on the operator's word".to_owned(),
                    phase: phase.clone(),
                    claimed: None,
                },
                FactCause::Root,
            )
            .expect("closure fact is valid");
        let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
        close.workflow_head(task, closed, move |_| wire::WorkflowHead {
            flow: vec![phase.clone()],
            phase,
            status: "completed".to_owned(),
            review: "closed".to_owned(),
            assignment: None,
            deferral: None,
            completion: Some(at),
            generation: 0,
        });
        commit(&self.bus, close).expect("closure commits");
    }

    /// Stage `lane`'s active claim, with its lease, on the task `added` adds.
    fn claim(
        &self,
        seed: &mut JournalWrite,
        task: &wire::TaskStamp,
        added: journal::PendingFact,
        lane: CanonicalUuid,
    ) {
        let cell = wire::ClaimCell {
            project: "serve.rooms".to_owned(),
            phase: "todo".parse().expect("fixture phase"),
        };
        let until: wire::Instant = "2030-08-01T07:00:00.000Z"
            .parse()
            .expect("fixture claim expiry");
        let won = seed
            .fact(
                wire::body::ClaimWon {
                    task: task.clone(),
                    actor: Actor::Lane(lane),
                    lane,
                    occupant: lane,
                    epoch: self.epoch,
                    cell: cell.clone(),
                    until,
                    git: None,
                    recovery: None,
                    source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
                },
                FactCause::Pending(added),
            )
            .expect("claim fact is valid");
        let epoch = self.epoch;
        seed.claim_head(task.clone(), won, move |fact| wire::ClaimHead {
            actor: Actor::Lane(lane),
            fact,
            claimed: fact,
            cell,
            lane,
            occupant: lane,
            epoch,
            recovery: None,
            disposition: wire::ClaimDisposition::Active,
            acknowledged: 0,
        });
        seed.lease_head(task.clone(), won, move |_| wire::LeaseHead {
            start: until,
            renewal: until,
            expiry: until,
            maximum: until,
            epoch,
        });
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

    pub(super) fn rooms(&self) -> Vec<wire::rooms::RoomMessaged> {
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
/// `A#R` also opens a room-bound delivery for A, queued while A's proved lane
/// is vacant; a packet with no seated route still records in R; equal text at
/// another locus records again; a reread writes nothing.
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
    let recorded = |lanes: &[u8], queued: &[wire::OccupantAddress], unreached: &[String]| {
        Ok(steering::Published::Sent {
            lanes: lanes.iter().map(|lane| test_uuid(*lane)).collect(),
            queued: queued.to_vec(),
            unreached: unreached.to_vec(),
            rooms: vec![room.clone()],
            conversed: None,
            held_room: false,
        })
    };
    assert_eq!(
        outcomes,
        [
            recorded(&[], &[], &[]),
            recorded(&[FIRST], &[second], &[]),
            recorded(&[], &[], &[]),
            recorded(&[], &[second], &[]),
        ],
        "{diagnostics}"
    );
    let messages = lanes.messages();
    let [(_, message)] = messages.as_slice() else {
        panic!("only the seated directed route delivers: {messages:?}");
    };
    assert_eq!(message.lane, test_uuid(FIRST));
    assert_eq!(message.room, Some(born.clone()));
    let queued = lanes.queued_messages();
    assert_eq!(queued.len(), 2);
    assert!(
        queued
            .iter()
            .all(|(_, message)| message.room == Some(born.clone()))
    );
    let [pending] = lanes
        .pending(FIRST)
        .try_into()
        .expect("one message is pending");
    assert_eq!(pending.room.as_ref(), Some(&room));
    let seal: steer::Key = "1kQnrzNZ".parse().expect("fixture seal");
    let rendered = steer::inbox::render_pending(&seal, &[pending], |_, _| None);
    assert!(
        rendered.contains(&format!(
            "[PEER sender {} lane {} room {room}]",
            lanes.address(SENDER),
            test_uuid(SENDER)
        )),
        "the delivery header names the frozen room: {rendered}"
    );
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

/// More retained room publications than several catch-up pages hold.
const RETAINED_BACKLOG: usize = 40;

/// Each listener starts at a horizon the resident records once, so a large
/// retained backlog is never owed; both current listeners then receive the
/// same bounded source-backed page of what followed, and delivery never
/// becomes lifecycle authority.
#[test]
fn resident_catch_up_pages_two_listeners_from_their_horizons_without_waking_them() {
    let lanes = Lanes::new("room-catch-up");
    let room = lanes.room("dVrD245Kc");
    let (first_lane, second_lane) = (test_uuid(FIRST), test_uuid(SECOND));
    for (lane, body) in [(FIRST, "first joined"), (SECOND, "second joined")] {
        lanes.author(lane, &format!("MSG #{room}: {body}"));
        lanes.account(lane);
        let _ = lanes.hear(lane, test_uuid(lane + 1));
    }
    for item in 0..RETAINED_BACKLOG {
        lanes.author(SENDER, &format!("MSG #{room}: retained {item}"));
    }
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));

    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    let startup = load_startup_surface(&repository).expect("fixture surface loads");
    let mut seats = Seats::new();
    for lane in [FIRST, SECOND] {
        seats
            .seat_detached(
                test_uuid(lane),
                test_uuid(lane + 1),
                Box::new(DetachedFixture {
                    dropped: Arc::new(AtomicU64::new(0)),
                }),
            )
            .expect("listener seats");
    }
    let notice = |lane| room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, lane));
    let appended = |after: u64| {
        lanes
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter(|envelope| envelope.seq.get() > after)
            .map(|envelope| {
                envelope
                    .fact
                    .read::<wire::rooms::RoomHorizon>()
                    .map(|body| body.lane)
            })
            .collect::<Vec<_>>()
    };
    let mut diagnostics = Vec::new();
    let before = lanes.cursor();
    for _ in 0..2 {
        room_catch_up::reconcile(
            &lanes.bus,
            &lanes.paths,
            &startup.config,
            &seats,
            &mut diagnostics,
        )
        .expect("catch-up reconciles");
    }
    let mut started = appended(before);
    started.sort();
    assert_eq!(
        started,
        [Some(first_lane), Some(second_lane)],
        "a retried reconciliation records one horizon per listener and nothing else"
    );
    assert!(
        notice(first_lane).is_none() && notice(second_lane).is_none(),
        "retained history before the horizons is never owed: {}",
        String::from_utf8_lossy(&diagnostics)
    );

    for item in 0..18 {
        lanes.author(SENDER, &format!("MSG #{room}: publication {item}"));
    }
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));
    let before = lanes.cursor();
    room_catch_up::reconcile(
        &lanes.bus,
        &lanes.paths,
        &startup.config,
        &seats,
        &mut diagnostics,
    )
    .expect("catch-up reconciles");
    assert_eq!(
        lanes.cursor(),
        before,
        "room delivery never writes lifecycle authority or wakes a lane"
    );

    let first = notice(first_lane).unwrap_or_else(|| {
        panic!(
            "first listener has catch-up: {}",
            String::from_utf8_lossy(&diagnostics)
        )
    });
    let second = notice(second_lane).expect("second listener has catch-up");
    for notice in [&first, &second] {
        assert!(notice.text.contains("publication 0"));
        assert!(notice.text.contains("continue: spice://task/"));
        assert!(!notice.text.contains("retained") && !notice.text.contains("joined"));
    }
    land_delivery(&lanes, &room, &startup.config, &mut seats, &first);
    seats.release_all();
    lanes.finish();
}

/// Commit-driven catch-up ignores authority-neutral task records and scopes a
/// publication transaction to the room indexed by that transaction.
#[test]
fn resident_catch_up_reconciles_only_commit_changed_authority() {
    let lanes = Lanes::new("room-catch-up-commit-scope");
    let room = lanes.held_room("dVrD245Kc", FIRST);
    let first_lane = test_uuid(FIRST);
    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    let startup = load_startup_surface(&repository).expect("fixture surface loads");
    let mut seats = Seats::new();
    seats
        .seat_detached(
            first_lane,
            test_uuid(FIRST + 1),
            Box::new(DetachedFixture {
                dropped: Arc::new(AtomicU64::new(0)),
            }),
        )
        .expect("listener seats");
    let mut diagnostics = Vec::new();
    room_catch_up::reconcile(
        &lanes.bus,
        &lanes.paths,
        &startup.config,
        &seats,
        &mut diagnostics,
    )
    .expect("startup catch-up establishes the horizon");

    let task = room.born().expect("fixture room has a birth");
    let mut note =
        write(Actor::Operator, lanes.epoch, Producer::CommandDoor).expect("note write starts");
    note.fact(
        wire::body::TaskNoted {
            task: task.clone(),
            note: "authority-neutral note".to_owned(),
        },
        FactCause::Root,
    )
    .expect("note fact is valid");
    let noted = commit(&lanes.bus, note).expect("note commits");
    assert_eq!(
        room_catch_up::changes(&lanes.bus, noted).expect("note changes classify"),
        room_catch_up::Changes::default(),
        "a task note schedules no room projection"
    );
    room_catch_up::reconcile_commit(
        &lanes.bus,
        &lanes.paths,
        &startup.config,
        &seats,
        &room_catch_up::changes(&lanes.bus, noted).expect("note changes classify"),
        &mut diagnostics,
    )
    .expect("irrelevant commit is ignored");

    let published = append_room_publication(&lanes, &room);
    let mut expected_rooms = BTreeSet::new();
    expected_rooms.insert(task);
    assert_eq!(
        room_catch_up::changes(&lanes.bus, published).expect("publication changes classify"),
        room_catch_up::Changes {
            authority: false,
            lanes: BTreeSet::new(),
            rooms: expected_rooms,
        },
        "a publication schedules only its indexed room"
    );
    room_catch_up::reconcile_commit(
        &lanes.bus,
        &lanes.paths,
        &startup.config,
        &seats,
        &room_catch_up::changes(&lanes.bus, published).expect("publication changes classify"),
        &mut diagnostics,
    )
    .expect("changed room reconciles");
    let notice = room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane))
        .expect("current listener receives the changed room");
    assert!(notice.text.contains("appended before accounting"));
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    seats.release_all();
    lanes.finish();
}

/// A room-bound delivery's terminal steering fact publishes no room row, but
/// commit classification still names the exact room whose attention changed.
#[test]
fn room_bound_steering_disposition_names_its_room_change() {
    let lanes = Lanes::new("room-catch-up-steering-disposition");
    let room = lanes.room("dVrD245Kc");
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}#{room}: settle this delivery"));
    lanes.account(SENDER);
    let (_, published) = lanes.hear_newest(SENDER);
    assert!(matches!(published.as_slice(), [Ok(_)]));
    let key = lanes.messages().pop().expect("room message records").1.key;

    let commits = lanes.bus.subscribe().expect("commit subscription opens");
    lanes.answer(FIRST, &[&key], &format!("ACK {key}: received"));
    let acknowledged = commits.recv().expect("acknowledgment commit publishes");
    let mut rooms = BTreeSet::new();
    rooms.insert(room.born().expect("room stamp"));
    assert_eq!(
        room_catch_up::changes(&lanes.bus, acknowledged).expect("acknowledgment changes classify"),
        room_catch_up::Changes {
            authority: false,
            lanes: BTreeSet::new(),
            rooms,
        }
    );
    lanes.finish();
}

/// `MSG ADDRESS#ROOM` invites the addressed lane: it hears later room posts
/// through ordinary catch-up without posting or holding the room's work, while
/// a seated lane nobody addressed still hears nothing.
#[test]
fn a_lane_addressed_into_a_room_hears_its_later_publications() {
    let lanes = Lanes::new("room-invitation");
    let room = lanes.room("dVrD245Kc");
    let (first_lane, second_lane) = (test_uuid(FIRST), test_uuid(SECOND));
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}#{room}: join us here"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));

    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    let startup = load_startup_surface(&repository).expect("fixture surface loads");
    let mut seats = Seats::new();
    for lane in [FIRST, SECOND] {
        seats
            .seat_detached(
                test_uuid(lane),
                test_uuid(lane + 1),
                Box::new(DetachedFixture {
                    dropped: Arc::new(AtomicU64::new(0)),
                }),
            )
            .expect("listener seats");
    }
    let mut diagnostics = Vec::new();
    let mut reconcile = || {
        room_catch_up::reconcile(
            &lanes.bus,
            &lanes.paths,
            &startup.config,
            &seats,
            &mut diagnostics,
        )
        .expect("catch-up reconciles");
    };
    reconcile();
    lanes.author(SENDER, &format!("MSG #{room}: after the invitation"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));
    reconcile();

    let notice = |lane| room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, lane));
    let invited = notice(first_lane).unwrap_or_else(|| {
        panic!(
            "the addressed lane hears the room: {}",
            String::from_utf8_lossy(&diagnostics)
        )
    });
    assert!(invited.text.contains("after the invitation"));
    assert!(
        notice(second_lane).is_none(),
        "an unaddressed lane is not invited"
    );
    seats.release_all();
    lanes.finish();
}

fn land_delivery(
    lanes: &Lanes,
    room: &wire::TaskHandle,
    config: &SurfaceConfig,
    seats: &mut Seats,
    notice: &room_catch_up::Notice,
) {
    let source = message_source(lanes.bus.reader(), lanes.epoch);
    let publisher = source.message_publisher();
    let generation = publisher.batch().expect("message generation begins");
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&lanes.paths, &tastes, &steering, &generation);
    let message_state = std::sync::Mutex::new(MessageState::default());
    let mut diagnostics = Vec::new();
    let interrupted = lanes.operator(FIRST, "interrupted delivery");
    accept_capture_record(
        &lanes.bus,
        lanes.epoch,
        seats,
        &surfaces,
        None,
        &interrupted,
        CaptureViews::new(&message_state, &mut diagnostics),
    )
    .expect("interrupted delivery record is accepted");
    let first_lane = test_uuid(FIRST);
    assert!(
        lanes
            .bus
            .read(move |read| read.room_cursor(first_lane))
            .expect("cursor reads")
            .is_none(),
        "a different transcript record cannot advance the cursor"
    );

    let landed = lanes.operator(FIRST, &notice.text);
    accept_capture_record(
        &lanes.bus,
        lanes.epoch,
        seats,
        &surfaces,
        None,
        &landed,
        CaptureViews::new(&message_state, &mut diagnostics),
    )
    .expect("exact delivery record is accepted");
    assert!(
        lanes
            .bus
            .read(move |read| read.room_cursor(first_lane))
            .expect("cursor still reads")
            .is_none(),
        "live transcript coalescing leaves an unaccounted delivery pending"
    );
    let landed_path = room_catch_up::landed_notice_path(&lanes.paths, first_lane);
    fs::create_dir_all(landed_path.parent().expect("landed notice has a directory"))
        .expect("landed notice directory is created");
    room_catch_up::publish_notice(&landed_path, notice)
        .expect("the supervisor records exact landed evidence");
    append_room_publication(lanes, room);
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, config, seats, &mut diagnostics)
        .expect("intervening commit reconciliation succeeds");
    assert_eq!(
        room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane)).as_ref(),
        Some(notice),
        "an observed but unaccounted page remains the current delivery"
    );
    assert_eq!(
        room_catch_up::read_notice(&landed_path).as_ref(),
        Some(notice),
        "landed evidence stays exact until durable cursor progress"
    );
    let source_id = lanes
        .tape(FIRST)
        .source_id()
        .expect("tape source reads")
        .as_str()
        .parse()
        .expect("tape source is a UUID");
    let _ = ingest_transcripts(
        &lanes.bus,
        &lanes.paths,
        Some(first_lane),
        lane::ingest::Bound::IMMEDIATE,
        lane::ingest::Coalescing::default(),
        &mut diagnostics,
    )
    .expect("safe-boundary sweep accounts the delivery");
    recognize_delivery(
        lanes,
        config,
        seats,
        &landed,
        source_id,
        notice,
        &mut diagnostics,
    );
    let continuation =
        room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane))
            .expect("the bounded continuation becomes current");
    assert_ne!(continuation.text, notice.text);
    assert!(continuation.through > notice.through);
    assert!(continuation.text.contains("appended before accounting"));
    assert!(
        room_catch_up::read_notice(&landed_path).is_none(),
        "cursor progress releases landed evidence"
    );
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    generation.commit().expect("message generation commits");
}

fn recognize_delivery(
    lanes: &Lanes,
    config: &SurfaceConfig,
    seats: &mut Seats,
    landed: &lane::capture::CaptureRecord,
    source: CanonicalUuid,
    notice: &room_catch_up::Notice,
    diagnostics: &mut Vec<u8>,
) {
    let first_lane = test_uuid(FIRST);
    let delivered = room_catch_up::recognize(&lanes.bus, &lanes.paths, landed, source)
        .expect("accounted delivery is recognized")
        .expect("exact delivery commits its cursor");
    let mut expected_lanes = BTreeSet::new();
    expected_lanes.insert(first_lane);
    assert_eq!(
        room_catch_up::changes(&lanes.bus, delivered).expect("delivery changes classify"),
        room_catch_up::Changes {
            authority: false,
            lanes: expected_lanes,
            rooms: BTreeSet::new(),
        },
        "cursor progress schedules only its lane"
    );
    let cursor = lanes
        .bus
        .read(move |read| read.room_cursor(first_lane))
        .expect("cursor rereads")
        .expect("exact delivery advances the cursor");
    assert_eq!(cursor.body().through.get(), notice.through.get());
    room_catch_up::reconcile_commit(
        &lanes.bus,
        &lanes.paths,
        config,
        seats,
        &room_catch_up::changes(&lanes.bus, delivered).expect("delivery changes classify"),
        diagnostics,
    )
    .expect("recognized cursor reconciles");
}

fn append_room_publication(lanes: &Lanes, room: &wire::TaskHandle) -> bus::JournalCommit {
    let publication = wire::rooms::RoomAuthorityStamp::for_micros(1_790_000_000_000_000)
        .expect("fixture publication stamp");
    let mut append =
        write(Actor::Operator, lanes.epoch, Producer::CommandDoor).expect("post write starts");
    append
        .fact(
            wire::rooms::RoomPublished {
                room: room.born().expect("fixture room has a birth"),
                publication,
                content: wire::rooms::RoomPublicationContent::Authored {
                    text: "appended before accounting".to_owned(),
                },
            },
            FactCause::Root,
        )
        .expect("intervening publication is valid");
    commit(&lanes.bus, append).expect("intervening publication commits")
}

/// A context naming no task refuses the whole packet: no route delivers and
/// no room records.
#[test]
fn an_unknown_context_refuses_every_route() {
    let lanes = Lanes::new("msg-room-unknown");
    let room = lanes.room("dVrD244Jd");
    let (first, second) = (lanes.address(FIRST), lanes.address(SECOND));
    lanes.author(
        SENDER,
        &format!("MSG {first}#{room} {second}#ROOMS-dVs4wZ9Q3: lost"),
    );
    lanes.account(SENDER);

    let (outcomes, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert!(
        matches!(
            outcomes.as_slice(),
            [Err(steering::Unpublished::AuthorDirected { reason, .. })]
                if reason == "msg::context-unknown"
        ),
        "{outcomes:?}"
    );
    assert!(lanes.messages().is_empty());
    assert!(lanes.rooms().is_empty());
    assert!(lanes.pending(FIRST).is_empty());
    assert!(lanes.pending(SECOND).is_empty());
    lanes.finish();
}

/// A packet pointing at the task its author's lane holds, or qualifying an
/// address with it, is marked for the no-header guidance and otherwise
/// publishes normally; another task or no room keeps the plain notice.
#[test]
fn a_packet_naming_the_authors_held_task_earns_the_no_header_guidance() {
    let lanes = Lanes::new("msg-room-held");
    let held = lanes.held_room("dVrD244Jd", SENDER);
    let other = lanes.room("dVrD245Kc");
    let first = lanes.address(FIRST);
    for packet in [
        format!("MSG #{held}: status"),
        format!("MSG #{other}: status"),
        format!("MSG {first}#{held}: status"),
        format!("MSG #{held} #{other}: status"),
        format!("MSG {first}: status"),
    ] {
        lanes.author(SENDER, &packet);
    }
    lanes.account(SENDER);

    let (outcomes, diagnostics) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    let marked = outcomes
        .iter()
        .map(|outcome| match outcome {
            Ok(steering::Published::Sent { held_room, .. }) => *held_room,
            refused => panic!("every packet publishes: {refused:?}\n{diagnostics}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(marked, [true, false, true, true, false], "{diagnostics}");
    assert_eq!(
        lanes.rooms().len(),
        5,
        "each named room records its packet regardless of guidance"
    );
    assert_eq!(
        lanes.pending(FIRST).len(),
        2,
        "addressed routes still deliver"
    );

    let notice = |held_room| steering::MessageRecordedNotice {
        occupant: test_uuid(SENDER + 1),
        packet: "1:0:0:1:1".to_owned(),
        header: format!("MSG #{held}:"),
        line: 1,
        column: 1,
        rooms: vec![held.to_string()],
        queued: Vec::new(),
        operator: false,
        asked: None,
        answered: None,
        reached: 0,
        held_room,
    };
    assert!(notice(true).text().contains("`MSG ADDRESS:`"));
    assert!(!notice(false).text().contains("`MSG ADDRESS:`"));
    lanes.finish();
}

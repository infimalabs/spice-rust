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
        self.seed_task_with_acceptance(stamp, title, description, intent, holder, &[])
    }

    /// Seed one pending task with explicit statement acceptance.
    pub(super) fn seed_task_with_acceptance(
        &self,
        stamp: &str,
        title: &str,
        description: Option<&str>,
        intent: Option<wire::TaskIntent>,
        holder: Option<CanonicalUuid>,
        acceptance: &[&str],
    ) -> wire::TaskHandle {
        let task: wire::TaskStamp = stamp.parse().expect("fixture stamp");
        let phase: wire::Phase = if intent.is_some_and(wire::TaskIntent::is_design) {
            "design"
        } else {
            "todo"
        }
        .parse()
        .expect("fixture phase");
        let creation = wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        );
        let acceptance = acceptance
            .iter()
            .map(|item| (*item).to_owned())
            .collect::<Vec<_>>();
        let mut seed =
            write(Actor::Spice, self.epoch, Producer::CommandDoor).expect("task write starts");
        let added = seed
            .fact(
                wire::body::TaskAdded {
                    task: task.clone(),
                    title: title.to_owned(),
                    pitch: "Rooms need work to talk about.".to_owned(),
                    description: description.map(str::to_owned),
                    acceptance: acceptance.clone(),
                    requirements: wire::EnvironmentRequirements::default(),
                    project: "serve.rooms".to_owned(),
                    filing: wire::TaskFiling::Filed,
                    priority: None,
                    taste: None,
                    document_slug: None,
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
            born,
            queue_origin: position,
            title,
            pitch: "Rooms need work to talk about.".to_owned(),
            description,
            acceptance,
            requirements: wire::EnvironmentRequirements::default(),
            resources: wire::ResourceRequirements::default(),
            project: "serve.rooms".to_owned(),
            filing: wire::TaskFiling::Filed,
            priority: None,
            taste: None,
            document_slug: None,
            dependencies: Vec::new(),
            provenance: wire::TaskProvenance::Parentless,
            creation,
            revision: position,
        });
        seed.workflow_head(task.clone(), added, move |_| wire::WorkflowHead {
            flow: vec![phase.clone()],
            phase,
            status: wire::WorkflowStatus::pending(),
            deferral: None,
            generation: 0,
        });
        if let Some(lane) = holder {
            self.claim(&mut seed, &task, FactCause::Pending(added), lane);
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
            status: wire::WorkflowStatus::closed(at),
            deferral: None,
            generation: 0,
        });
        commit(&self.bus, close).expect("closure commits");
    }

    /// Stage `lane`'s active claim, with its lease, on `task`.
    fn claim(
        &self,
        seed: &mut JournalWrite,
        task: &wire::TaskStamp,
        cause: FactCause,
        lane: CanonicalUuid,
    ) {
        let occupant = current_residency(&self.bus, self.epoch)
            .expect("residency reads")
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == lane)
            .and_then(|row| row.occupant)
            .expect("claimed lane is occupied");
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
                    occupant,
                    epoch: self.epoch,
                    cell: cell.clone(),
                    until,
                    git: None,
                    recovery: None,
                    source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
                },
                cause,
            )
            .expect("claim fact is valid");
        let epoch = self.epoch;
        seed.claim_head(task.clone(), won, move |fact| wire::ClaimHead {
            actor: Actor::Lane(lane),
            fact,
            claimed: fact,
            cell,
            lane,
            occupant,
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

    /// Commit `lane`'s win of `task`'s claim on its own.
    pub(super) fn win_claim(&self, task: &wire::TaskHandle, lane: u8) {
        let task = task.born().expect("fixture handle carries its stamp");
        let lane = test_uuid(lane);
        let mut win =
            write(Actor::Lane(lane), self.epoch, Producer::CommandDoor).expect("win write starts");
        self.claim(&mut win, &task, FactCause::Root, lane);
        commit(&self.bus, win).expect("claim win commits");
    }

    /// Release `task`'s current claim as its holder.
    pub(super) fn free_claim(&self, task: &wire::TaskHandle) {
        let task = task.born().expect("fixture handle carries its stamp");
        let claim = {
            let task = task.clone();
            self.bus
                .read(move |read| read.claim_head(&task))
                .expect("claim head reads")
                .expect("the task is claimed")
                .into_body()
        };
        let mut free =
            write(claim.actor.clone(), self.epoch, Producer::CommandDoor).expect("free starts");
        let freed = free
            .fact(
                wire::body::ClaimFreed {
                    task: task.clone(),
                    actor: claim.actor.clone(),
                    parked: None,
                },
                FactCause::Existing(claim.claimed),
            )
            .expect("claim release fact is valid");
        free.claim_head(task, freed, move |fact| wire::ClaimHead {
            fact,
            disposition: wire::ClaimDisposition::Freed,
            ..claim
        });
        commit(&self.bus, free).expect("claim release commits");
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

    /// Replace one lane's accounted transcript source without changing its
    /// vendor session.
    pub(super) fn replace_transcript_source(&self, lane: u8) -> CanonicalUuid {
        let mut head = self
            .bus
            .read(move |read| read.transcript_source_heads(&test_uuid(lane)))
            .expect("transcript sources read")
            .into_iter()
            .next()
            .expect("declared lane has a transcript source")
            .into_body();
        let superseded = head.source;
        let source = test_uuid(99);
        head.source = source;
        let mut accounting = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("replacement accounting write starts");
        let ingested = accounting
            .fact(
                wire::body::JournalIngested {
                    lane: head.lane,
                    vendor: head.vendor.clone(),
                    stream: head.stream.clone(),
                    source,
                    supersedes: Some(superseded),
                    from: wire::body::TranscriptPosition {
                        member: head.next.member,
                        offset: 0,
                    },
                    to: head.next,
                    usage: Vec::new(),
                    non_usage_records: 0,
                    residue_records: 0,
                },
                FactCause::Root,
            )
            .expect("replacement batch is valid");
        accounting.transcript_source_head(ingested, move |_| head);
        commit(&self.bus, accounting).expect("replacement accounting commits");
        source
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
            redirected: Vec::new(),
            unreached: unreached.to_vec(),
            rooms: vec![room.clone()],
            operator: false,
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
    let now = pending.authored;
    let rendered = steer::inbox::render_pending(&seal, &[pending], now, |_, _| None);
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

/// More room publications than one catch-up page holds.
const RETAINED_BACKLOG: usize = 18;

/// A current claim admits publications from its own durable authority even
/// when the supervisor seats later. An addressed lane without native
/// authority receives no historical echo, and delivery never becomes
/// lifecycle authority.
#[test]
fn resident_catch_up_starts_at_native_authority_and_skips_an_addressed_lane() {
    let lanes = Lanes::new("room-catch-up");
    let room = lanes.held_room("dVrD245Kc", FIRST);
    let (first_lane, second_lane) = (test_uuid(FIRST), test_uuid(SECOND));
    let address = lanes.address(SECOND);
    lanes.author(SENDER, &format!("MSG {address}#{room}: second addressed"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));
    for item in 0..RETAINED_BACKLOG {
        lanes.author(SENDER, &format!("MSG #{room}: retained {item}"));
    }
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));

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
    let mut diagnostics = Vec::new();
    let before = lanes.cursor();
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, &seats, &mut diagnostics)
        .expect("catch-up reconciles");
    assert_eq!(
        lanes.cursor(),
        before,
        "room delivery never writes lifecycle authority or wakes a lane"
    );

    let first = notice(first_lane).unwrap_or_else(|| {
        panic!(
            "the working lane catches up from its claim: {}",
            String::from_utf8_lossy(&diagnostics)
        )
    });
    assert!(first.text.contains("second addressed"), "{first:?}");
    assert!(first.text.contains("retained 0"), "{first:?}");
    assert!(first.text.contains("continue: spice://task/"));
    assert!(
        notice(second_lane).is_none(),
        "a room-scoped MSG route reaches its lane by steering, never later catch-up"
    );
    land_delivery(&lanes, &room, &mut seats, &first);
    seats.release_all();
    lanes.finish();
}

/// Commit-driven catch-up scopes a publication transaction to the room indexed
/// by that transaction.
#[test]
fn resident_catch_up_reconciles_only_commit_changed_authority() {
    let lanes = Lanes::new("room-catch-up-commit-scope");
    let room = lanes.held_room("dVrD245Kc", FIRST);
    let first_lane = test_uuid(FIRST);
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
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, &seats, &mut diagnostics)
        .expect("startup catch-up establishes the horizon");

    let task = room.born().expect("fixture room has a birth");
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
        &seats,
        &room_catch_up::changes(&lanes.bus, published).expect("publication changes classify"),
        &mut diagnostics,
    )
    .expect("changed room reconciles");
    let notice = room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane))
        .expect("current listener receives the changed room");
    assert!(notice.text.contains("appended before accounting"));
    assert!(
        !notice.text.contains("spice room leave"),
        "native work owes no Design request, so no leave is offered"
    );
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    seats.release_all();
    lanes.finish();
}

/// Recording a Design request changes room attention at its own authority:
/// prior unread context stays excluded, while a later room post is delivered.
#[test]
fn a_design_request_commit_reconciles_its_room_for_the_asked_lane() {
    let lanes = Lanes::new("room-catch-up-design-request");
    let prior = lanes.held_room("dVrD245Kc", FIRST);
    let design = lanes.seed_task(
        "dVrD245Kd",
        "Design",
        Some("Decide how room context should reach an asked lane."),
        Some(wire::TaskIntent::Design),
        None,
    );
    let first_lane = test_uuid(FIRST);
    let mut seats = Seats::new();
    seats
        .seat_detached(
            first_lane,
            test_uuid(FIRST + 1),
            Box::new(DetachedFixture {
                dropped: Arc::new(AtomicU64::new(0)),
            }),
        )
        .expect("listener seat");
    let mut diagnostics = Vec::new();
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, &seats, &mut diagnostics)
        .expect("startup catch-up reconciles");

    append_room_publication(&lanes, &design);
    let design_stamp = design.born().expect("fixture Design has a birth");
    let noticed = record_design_request(&lanes, &design_stamp, &prior, first_lane);
    let mut expected_rooms = BTreeSet::new();
    expected_rooms.insert(design_stamp);
    let changes =
        room_catch_up::changes(&lanes.bus, noticed).expect("Design request changes classify");
    assert_eq!(
        changes,
        room_catch_up::Changes {
            authority: false,
            lanes: BTreeSet::new(),
            rooms: expected_rooms,
        }
    );
    room_catch_up::reconcile_commit(&lanes.bus, &lanes.paths, &seats, &changes, &mut diagnostics)
        .expect("Design request room reconciles");

    assert!(
        room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane)).is_none(),
        "the request cannot retroactively admit room context"
    );
    let published = append_room_publication(&lanes, &design);
    let changes =
        room_catch_up::changes(&lanes.bus, published).expect("later publication changes classify");
    room_catch_up::reconcile_commit(&lanes.bus, &lanes.paths, &seats, &changes, &mut diagnostics)
        .expect("later room publication reconciles");
    let notice = room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, first_lane))
        .expect("asked lane receives later room context");
    assert!(notice.text.contains("appended before accounting"));
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    seats.release_all();
    lanes.finish();
}

/// The admission current selection records, as a fixture notice carries it.
fn fixture_admission() -> wire::rooms::DesignAdmission {
    wire::rooms::DesignAdmission::Scored(
        wire::rooms::DesignNoticeMatch::new(wire::rooms::DesignReach::Wide, 0.2, 0.1, [1; 32])
            .expect("fixture match clears its floor"),
    )
}

fn record_design_request(
    lanes: &Lanes,
    design: &wire::TaskStamp,
    prior: &wire::TaskHandle,
    lane: CanonicalUuid,
) -> bus::JournalCommit {
    let statement = lanes
        .bus
        .read({
            let design = design.clone();
            move |read| read.task_head(&design)
        })
        .expect("Design statement reads")
        .expect("Design statement exists")
        .body()
        .revision;
    let prior = prior.born().expect("fixture prior task has a birth");
    let mut request = write(Actor::Spice, lanes.epoch, Producer::ResidentRecognition)
        .expect("request write starts");
    request
        .fact(
            wire::rooms::DesignNoticed {
                design: design.clone(),
                statement,
                lane,
                occupant: test_uuid(FIRST + 1),
                session: wire::rooms::DesignSession {
                    vendor: "omp".parse().expect("vendor"),
                    identity: "fixture-session".to_owned(),
                },
                compaction: None,
                opportunity: wire::rooms::DesignNoticeOpportunity::TaskCompletion {
                    task: prior.clone(),
                    completed: statement,
                },
                evidence: wire::rooms::DesignNoticeEvidence::Task {
                    task: prior,
                    statement,
                    completed: statement,
                    support: wire::rooms::DesignSupport::Implemented,
                },
                excerpt: "context room".to_owned(),
                terms: vec!["context".to_owned(), "room".to_owned()],
                admission: fixture_admission(),
                asks: Vec::new(),
            },
            FactCause::Root,
        )
        .expect("Design request is valid");
    commit(&lanes.bus, request).expect("Design request commits")
}

fn land_delivery(
    lanes: &Lanes,
    room: &wire::TaskHandle,
    seats: &mut Seats,
    notice: &room_catch_up::Notice,
) {
    let source = message_source(lanes.bus.reader(), lanes.epoch);
    let publisher = source.message_publisher();
    let generation = publisher.batch().expect("message generation begins");
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let ambient = spice::ambient::capture();
    let surfaces = CaptureSurfaces::new(&lanes.paths, &tastes, &steering, &generation, &ambient);
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
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, seats, &mut diagnostics)
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
    recognize_delivery(lanes, seats, &landed, source_id, notice, &mut diagnostics);
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
        seats,
        &room_catch_up::changes(&lanes.bus, delivered).expect("delivery changes classify"),
        diagnostics,
    )
    .expect("recognized cursor reconciles");
}

fn append_room_publication(lanes: &Lanes, room: &wire::TaskHandle) -> bus::JournalCommit {
    let publication = wire::rooms::RoomAuthorityStamp::for_micros(
        1_790_000_000_000_000
            + i64::try_from(lanes.cursor()).expect("fixture cursor fits a timestamp"),
    )
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

fn read_awareness(lanes: &Lanes, design: &wire::TaskHandle, words: &[&str]) -> serde_json::Value {
    lanes.room_read(
        &["awareness", design.as_str()]
            .into_iter()
            .chain(words.iter().copied())
            .collect::<Vec<_>>(),
    )
}

fn awareness_rows(page: &serde_json::Value) -> Vec<serde_json::Value> {
    page["page"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["seq"].clone())
        .collect()
}

/// Awareness pages a Design's notices over their exact population, though
/// every record's envelope outweighs the row it renders: each first page
/// reports the whole population at any `--limit`, admits whole rows, and its
/// continuations visit every notice once in journal order.
#[test]
fn design_awareness_pages_heavy_notices_over_their_exact_population() {
    const NOTICES: usize = 34;
    let lanes = Lanes::new("design-awareness-paging");
    let prior = lanes.held_room("dVrD245Kc", FIRST);
    let design = lanes.seed_task(
        "dVrD245Kd",
        "Design",
        Some("Decide how awareness pages."),
        Some(wire::TaskIntent::Design),
        None,
    );
    let design_stamp = design.born().expect("fixture Design has a birth");
    let statement = lanes
        .bus
        .read({
            let design = design_stamp.clone();
            move |read| read.task_head(&design)
        })
        .expect("Design statement reads")
        .expect("Design statement exists")
        .body()
        .revision;
    // Rows show six distinct terms; the six long ones the record also keeps
    // make each envelope several times its row.
    let terms = ('a'..='f')
        .map(|letter| format!("a{letter}"))
        .chain(('a'..='f').map(|letter| format!("b{letter}{}", "x".repeat(400))))
        .collect::<Vec<_>>();
    let prior = prior.born().expect("fixture prior task has a birth");
    let mut noticed = Vec::with_capacity(NOTICES);
    for _ in 1..=NOTICES {
        let mut request = write(Actor::Spice, lanes.epoch, Producer::ResidentRecognition)
            .expect("notice write starts");
        request
            .fact(
                wire::rooms::DesignNoticed {
                    design: design_stamp.clone(),
                    statement,
                    lane: test_uuid(FIRST),
                    occupant: test_uuid(FIRST + 1),
                    session: wire::rooms::DesignSession {
                        vendor: "omp".parse().expect("vendor"),
                        identity: "fixture-session".to_owned(),
                    },
                    compaction: None,
                    opportunity: wire::rooms::DesignNoticeOpportunity::TaskCompletion {
                        task: prior.clone(),
                        completed: statement,
                    },
                    evidence: wire::rooms::DesignNoticeEvidence::Task {
                        task: prior.clone(),
                        statement,
                        completed: statement,
                        support: wire::rooms::DesignSupport::Implemented,
                    },
                    excerpt: "fixture notice".to_owned(),
                    terms: terms.clone(),
                    admission: fixture_admission(),
                    asks: Vec::new(),
                },
                FactCause::Root,
            )
            .expect("notice is valid");
        noticed.push(serde_json::json!(
            commit(&lanes.bus, request).expect("notice commits").first()
        ));
    }
    let read = |words: &[&str]| read_awareness(&lanes, &design, words);

    for limit in [
        &[][..],
        &["--limit", "1"],
        &["--limit", "5"],
        &["--limit", "50"],
    ] {
        let first = read(limit);
        let shown = awareness_rows(&first);
        assert!(!shown.is_empty(), "{limit:?} admits whole rows");
        assert_eq!(shown[..], noticed[..shown.len()], "{limit:?}");
        let omission = &first["page"]["omission"];
        if omission.is_null() {
            assert_eq!(shown.len(), NOTICES, "{limit:?} shows every notice");
        } else {
            assert_eq!(omission["population"], NOTICES, "{limit:?}");
            assert_eq!(omission["withheld"], NOTICES - shown.len(), "{limit:?}");
        }
    }

    let mut page = read(&["--limit", "5"]);
    let mut seen = awareness_rows(&page);
    while !page["page"]["omission"].is_null() {
        let resume =
            serde_json::from_value::<Vec<String>>(page["page"]["omission"]["continuation"].clone())
                .expect("a withheld notice names its continuation")
                .split_off(2);
        page = lanes.room_read(
            &["awareness"]
                .into_iter()
                .chain(resume.iter().map(String::as_str))
                .collect::<Vec<_>>(),
        );
        let next = awareness_rows(&page);
        assert!(!next.is_empty(), "each continuation admits a whole row");
        assert_eq!(
            page["page"]["omission"]["population"]
                .as_u64()
                .unwrap_or(next.len() as u64),
            (NOTICES - seen.len()) as u64,
            "a continuation counts exactly the notices from its cursor"
        );
        seen.extend(next);
    }
    assert_eq!(
        seen, noticed,
        "continuations visit every notice once in order"
    );
    lanes.finish();
}

/// The one-read projection a catch-up plan uses yields, for every room it
/// keeps, exactly what the per-room projection yields over the lane's latest
/// publication; the rooms it drops are those the lane has no way to listen
/// to: no work of its own there, no Design request about it, no live stop.
#[test]
fn batched_room_projection_equals_the_per_room_projection() {
    let lanes = Lanes::new("room-catch-up-batched-projection");
    let first_lane = test_uuid(FIRST);
    let own = lanes.held_room("dVrD245Kc", FIRST);
    lanes.held_room("dVrD245Kd", SECOND);
    let idle = lanes.room("dVrD245Kg");
    let design = lanes.seed_task(
        "dVrD245Km",
        "Design",
        Some("Decide how a batched projection is proven."),
        Some(wire::TaskIntent::Design),
        None,
    );
    let design_room = design.born().expect("fixture Design has a birth");
    record_design_request(&lanes, &design_room, &own, first_lane);
    lanes.author(SENDER, &format!("MSG #{idle}: nobody listens here"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));

    let planning = room_catch_up::Planning::load(&lanes.bus).expect("planning loads");
    let batched = room_catch_up::project_rooms(
        &lanes.bus,
        &planning,
        first_lane,
        planning.entries.iter().map(board::Entry::born),
        None,
    )
    .expect("batched projection");

    assert_eq!(
        batched
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [own.born().expect("fixture task has a birth"), design_room]
            .into_iter()
            .collect(),
        "only rooms with the lane's work or a Design request are projected"
    );
    for entry in &planning.entries {
        let room = entry.born().clone();
        let latest = lanes
            .bus
            .read({
                let room = room.clone();
                move |read| read.latest_room_publication(&room, &Actor::Lane(first_lane))
            })
            .expect("latest publication reads");
        let single = crate::board_command::project_attention(
            &lanes.bus,
            &planning.entries,
            &planning.residency,
            &room,
            latest.as_slice(),
        )
        .expect("per-room projection");
        match batched.get(&room) {
            Some(attention) => assert_eq!(*attention, single, "{room}"),
            None => assert!(
                single
                    .lanes
                    .iter()
                    .find(|row| row.lane == first_lane)
                    .and_then(spice::rooms::RoomLaneAttention::catch_up_after)
                    .is_none(),
                "a dropped room admits no catch-up for the lane: {room} {single:?}"
            ),
        }
    }
    lanes.finish();
}

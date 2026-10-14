use super::*;

use serde_json::Value;
use wire::intent::{
    RoomCoordinate, RoomCoordinateAction, RoomCoordinationRead, RoomCoordinationView, RoomLeave,
    RoomLeaveAnswer, RoomListeningDuty, RoomPutAside,
};
use wire::rooms::{
    CoordinationControl, OopsDisposition, OopsGrooming, RoomAuthorityStamp,
    RoomCoordinationStarted, RoomListenerAvailability, RoomListenerDelivery, RoomMessaged,
    RoomPost, RoomPostOutcome, RoomPublicationContent, RoomPublished, TranscriptHunk,
};

mod automation;
mod design_standing;

fn room_projects() -> ProjectPolicy {
    ProjectPolicy::new(
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("room fixture project policy")
}

fn prove_seat_addresses(bus: &Bus, epoch: CanonicalUuid, seats: &[Seat]) {
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    for (group, seat) in seats.iter().enumerate() {
        write
            .fact(
                wire::body::LaneStarted {
                    lane: seat.lane,
                    occupant: seat.occupant,
                    epoch,
                    group: group as u32 + 1,
                    fresh_session: Some(true),
                },
                FactCause::Root,
            )
            .expect("fixture occupant birth builds");
    }
    assert!(matches!(
        bus.commit(write).expect("fixture births reach writer"),
        CommitOutcome::Committed(_)
    ));
}

fn coordinate_command(
    bus: &Bus,
    epoch: CanonicalUuid,
    room: &TaskHandle,
    seat: &Seat,
    action: RoomCoordinateAction,
) -> wire::AdmittedCommand {
    ruled_coordinate_command(bus, epoch, room, seat, action, None)
}

fn ruled_coordinate_command(
    bus: &Bus,
    epoch: CanonicalUuid,
    room: &TaskHandle,
    seat: &Seat,
    action: RoomCoordinateAction,
    auto_resume: Option<u8>,
) -> wire::AdmittedCommand {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, room);
    let target = current
        .addresses
        .get(&seat.lane)
        .and_then(wire::SeatAddress::address)
        .expect("fixture seat has a proved base-52 address");
    fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomCoordinate,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            residency_witness(&current, seat.lane, seat.occupant),
        ],
        &RoomCoordinate {
            room: room.clone(),
            target,
            action,
            criterion: "Choose the exact retained candidate.".to_owned(),
            contribution: "Inspect and capture only this anchor.".to_owned(),
            stop: "Stop after recording the native disposition.".to_owned(),
            auto_resume,
        },
    )
}

fn coordinate(bus: &Bus, command: &wire::AdmittedCommand) -> Outcome<CommandAnswer> {
    let wire::intent::Intent::RoomCoordinate(request) = command.body() else {
        panic!("fixture command carries room coordination");
    };
    room_coordinate(
        bus,
        command,
        request,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
    )
}

fn coordinate_after_validation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    before_commit: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let wire::intent::Intent::RoomCoordinate(request) = command.body() else {
        panic!("fixture command carries room coordination");
    };
    room_coordinate_with(
        bus,
        command,
        request,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
        before_commit,
    )
}

fn put_aside_command(bus: &Bus, epoch: CanonicalUuid, room: &TaskHandle) -> wire::AdmittedCommand {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, room);
    fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPutAside,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &RoomPutAside { room: room.clone() },
    )
}

fn put_aside(bus: &Bus, command: &wire::AdmittedCommand) -> Outcome<CommandAnswer> {
    let wire::intent::Intent::RoomPutAside(request) = command.body() else {
        panic!("fixture command carries room put-aside");
    };
    room_put_aside(
        bus,
        command,
        request,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
    )
}

fn leave(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    room: &TaskHandle,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::RoomLeave,
        Vec::new(),
        &RoomLeave {
            room: room.clone(),
            clause: "my part is done".to_owned(),
        },
    );
    let wire::intent::Intent::RoomLeave(request) = command.body() else {
        panic!("fixture command carries room leave");
    };
    room_leave(
        bus,
        &command,
        request,
        &room_projects(),
        &board::NextConfig::default(),
    )
}

fn leave_answer(outcome: Outcome<CommandAnswer>) -> (bool, RoomLeaveAnswer) {
    match outcome {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::RoomLeave(answer),
        }) => (matches!(position, ReplyPosition::Committed(_)), answer),
        other => panic!("room leave returned another outcome: {other:?}"),
    }
}

fn post_listeners(bus: &Bus, epoch: CanonicalUuid, room: &TaskHandle) -> Vec<CanonicalUuid> {
    let answer = assert_committed(
        dispatch(bus, &post_command(bus, epoch, room, "Recorded only."))
            .expect("room command dispatches"),
    );
    let CommandAnswer::RoomPost(answer) = answer else {
        panic!("room post returned another answer shape: {answer:?}");
    };
    match answer.outcome {
        RoomPostOutcome::RecordedWithListeners { listeners } => {
            listeners.iter().map(|listener| listener.lane).collect()
        }
        _ => Vec::new(),
    }
}

fn coordination_status(bus: &Bus, room: &TaskHandle) -> RoomCoordinationView {
    let outcome = room_coordination_read(
        bus,
        &RoomCoordinationRead { room: room.clone() },
        &room_projects(),
        &board::NextConfig::default(),
    );
    let Outcome::Reply(Reply {
        body: CommandAnswer::RoomCoordination(view),
        ..
    }) = outcome
    else {
        panic!("room coordination status returned another outcome: {outcome:?}");
    };
    view
}

fn publication_page(bus: &Bus, room: &TaskHandle) -> journal::SelectedFactPage {
    let room = room.born().expect("fixture room has a birth stamp");
    bus.read(move |read| {
        read.room_publications(
            &room,
            None,
            journal::FactReadBudget::new(32, journal::MAX_RETAINED_FACT_PAGE_BYTES)
                .expect("fixture room page budget"),
        )
    })
    .expect("room publication page reads")
}

fn publish_listener(bus: &Bus, room: &TaskHandle, lane: CanonicalUuid, micros: i64) {
    let room = room.born().expect("fixture room has a birth stamp");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let origin = format!("task:{room}").parse().expect("fixture task origin");
    let mut write = JournalWrite::new(Actor::Lane(lane), origin, at, Producer::CommandDoor);
    write
        .fact(
            RoomPublished {
                room,
                publication: RoomAuthorityStamp::for_micros(micros)
                    .expect("fixture publication identity"),
                content: RoomPublicationContent::Authored {
                    text: "substantive listener evidence".to_owned(),
                },
            },
            FactCause::Root,
        )
        .expect("fixture room publication builds");
    assert!(matches!(
        bus.commit(write)
            .expect("fixture publication reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn vacate(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) {
    let observed = bus
        .task_snapshot()
        .expect("fixture residency snapshot reads")
        .residency()
        .expect("fixture residency exists")
        .clone();
    let mut vacant = observed.body().clone();
    let row = vacant
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane exists");
    row.occupant = None;
    row.disposition = "vacant".to_owned();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    let exited = write
        .fact(
            wire::body::LaneExited {
                lane: seat.lane,
                occupant: seat.occupant,
                code: 0,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("fixture exit builds");
    write.residency_head(exited, move |_| vacant);
    assert!(matches!(
        bus.commit(write).expect("fixture exit reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn post_command(
    bus: &Bus,
    epoch: CanonicalUuid,
    room: &TaskHandle,
    text: &str,
) -> wire::AdmittedCommand {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, room);
    fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![statement_witness(heads)],
        &RoomPost {
            room: room.clone(),
            text: text.to_owned(),
            grooming: None,
        },
    )
}

fn publish_packet(bus: &Bus, room: &TaskHandle, seat: &Seat) -> RoomAuthorityStamp {
    let vendor: wire::Name = "omp".parse().expect("fixture vendor");
    let source = uuid(90_001);
    let next = wire::body::TranscriptPosition {
        member: 1,
        offset: 64,
    };
    let head = wire::TranscriptSourceHead {
        lane: seat.lane,
        vendor: vendor.clone(),
        stream: "session".to_owned(),
        source,
        next,
    };
    let origin = format!("lane:{}", seat.lane)
        .parse()
        .expect("fixture lane origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut ingest = JournalWrite::new(
        Actor::Spice,
        origin,
        at.clone(),
        Producer::ResidentRecognition,
    );
    let fact = ingest
        .fact(
            wire::body::JournalIngested {
                lane: seat.lane,
                vendor,
                stream: "session".to_owned(),
                source,
                supersedes: None,
                from: wire::body::TranscriptPosition {
                    member: 1,
                    offset: 0,
                },
                to: next,
                usage: Vec::new(),
                non_usage_records: 1,
                residue_records: 0,
            },
            FactCause::Root,
        )
        .expect("fixture transcript ingestion builds");
    ingest.transcript_source_head(fact, {
        let head = head.clone();
        move |_| head
    });
    assert!(matches!(
        bus.commit(ingest)
            .expect("fixture ingestion reaches writer"),
        CommitOutcome::Committed(_)
    ));

    let publication =
        RoomAuthorityStamp::for_micros(1_790_000_000_000_001).expect("fixture packet identity");
    let mut packet = JournalWrite::new(
        Actor::Lane(seat.lane),
        "task:room-history".parse().expect("fixture packet origin"),
        at,
        Producer::AckScanner,
    );
    packet
        .fact(
            RoomMessaged {
                room: room.born().expect("fixture room has a birth"),
                publication: publication.clone(),
                hunk: TranscriptHunk {
                    head,
                    locus: wire::MessageLocus {
                        member: 1,
                        offset: 0,
                        length: 64,
                        event: 0,
                    },
                    text: wire::rooms::TextRange { start: 0, end: 6 },
                },
                author: seat.occupant,
                directed: Vec::new(),
            },
            FactCause::Root,
        )
        .expect("fixture room packet builds");
    assert!(matches!(
        bus.commit(packet).expect("fixture packet reaches writer"),
        CommitOutcome::Committed(_)
    ));
    publication
}

fn history_page(root: &FixtureRoot, words: &[&str]) -> Value {
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "room", "history"].iter().chain(words))
        .expect("room history parses");
    let contract = crate::cli::selected_contract(&matches).expect("room history is registered");
    let journal = journal::JournalReader::open(root.as_ref().join("authority.sqlite3"))
        .expect("fixture journal opens to read");
    let outcome = crate::room_command::answer(
        &journal,
        &root.as_ref().join("tape"),
        &room_projects(),
        crate::cli::selected_matches(&matches),
        contract.path,
    );
    let Outcome::Reply(reply) = outcome else {
        panic!("room history returned another outcome: {outcome:?}");
    };
    serde_json::to_value(reply.body).expect("room history encodes")
}

#[test]
fn room_post_records_once_and_retains_its_exact_retry_answer() {
    let (_root, bus, epoch, _seats) = fixture("room-post-retry", 0);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-retry",
        "Room",
        None,
    );
    let command = post_command(&bus, epoch, &room, "Complete composer text.");
    let first = dispatch(&bus, &command).expect("room command dispatches");
    let answer = assert_committed(first.clone());
    let CommandAnswer::RoomPost(answer) = answer else {
        panic!("room post returned another answer shape: {answer:?}");
    };
    assert!(matches!(
        answer.outcome,
        RoomPostOutcome::RecordedNoCurrentListener { .. }
    ));

    let replay_command = command.clone();
    let replay = bus
        .read(move |read| read.command_replay(&replay_command))
        .expect("command retry receipt reads");
    assert_eq!(replay, Some(journal::CommandReplay::Exact(first)));

    let page = publication_page(&bus, &room);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.continuation, None);
    assert_eq!(page.rows[0].actor, Actor::Operator);
    let recorded = page.rows[0]
        .fact
        .read::<RoomPublished>()
        .expect("indexed publication has its typed body");
    assert_eq!(recorded.publication, answer.publication);
    assert_eq!(
        recorded.content,
        RoomPublicationContent::Authored {
            text: "Complete composer text.".to_owned()
        }
    );
}

#[test]
fn oops_grooming_records_source_preserving_negative_triage_without_mutating_reports() {
    let (_root, bus, epoch, _seats) = fixture("room-oops-grooming", 0);
    let batch_stamp = stamp("room-oops-batch");
    let batch = TaskHandle::for_project("board.test", &batch_stamp);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: batch_stamp }],
        &TaskAdd {
            preparation: None,
            title: "Bounded OOPS batch".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["Each selected source receives a typed disposition".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:oops-grooming".parse().expect("fixture origin")),
            flow: Some("design".to_owned()),
            depends: Vec::new(),
            intent: Some(wire::TaskIntent::Design),
            claim: false,
            draft: false,
        },
    ));
    let report_stamp = stamp("room-oops-report");
    let report = TaskHandle::for_filing("board.test", &report_stamp, wire::TaskFiling::Oops);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence {
            task: report_stamp.clone(),
        }],
        &TaskOopsAdd {
            title: "Observed friction".to_owned(),
            description: "Evidence retained by the report".to_owned(),
            origin: Some("fixture:oops-source".parse().expect("fixture origin")),
            project: Some("board.test".to_owned()),
        },
    ));
    let before = board(&bus, epoch, Actor::Operator);
    let batch_heads = at(&before, &batch);
    let report_heads = at(&before, &report);
    let statement = report_heads.statement.clone();
    let workflow = report_heads.workflow.clone();
    let oversized = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![
            statement_witness(batch_heads),
            statement_witness(report_heads),
        ],
        &RoomPost {
            room: batch.clone(),
            text: String::new(),
            grooming: Some(OopsGrooming {
                reports: vec![report_stamp.clone(); wire::rooms::OOPS_GROOMING_REPORT_LIMIT + 1],
                disposition: OopsDisposition::NoWork,
                cause: "One unbounded cluster.".to_owned(),
                exceptions: String::new(),
                uncertainty: String::new(),
            }),
        },
    );
    assert!(matches!(
        oversized,
        Outcome::Refusal(refusal) if refusal.reason() == "room::oops-batch-too-large"
    ));
    let grooming = OopsGrooming {
        reports: vec![report_stamp],
        disposition: OopsDisposition::NoWork,
        cause: "The report names expected bounded behavior.".to_owned(),
        exceptions: "A changed invariant requires a new batch.".to_owned(),
        uncertainty: "No production counterexample is retained.".to_owned(),
    };
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![
            statement_witness(batch_heads),
            statement_witness(report_heads),
        ],
        &RoomPost {
            room: batch.clone(),
            text: String::new(),
            grooming: Some(grooming.clone()),
        },
    ));

    let page = publication_page(&bus, &batch);
    let recorded = page.rows[0]
        .fact
        .read::<RoomPublished>()
        .expect("grooming publication has a typed body");
    assert_eq!(
        recorded.content,
        RoomPublicationContent::OopsGroomed { grooming }
    );
    let after = board(&bus, epoch, Actor::Operator);
    let retained = at(&after, &report);
    assert_eq!(retained.statement, statement);
    assert_eq!(retained.workflow, workflow);
    assert_eq!(retained.visibility, wire::intent::ProjectVisibility::Public);
    assert!(retained.statement.body.filing.is_oops());
}

#[test]
fn room_post_reports_occupied_vacant_and_unknown_listeners_without_waking() {
    let (_root, bus, epoch, seats) = fixture("room-post-listeners", 2);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-listeners",
        "Room",
        None,
    );
    let micros = jiff::Timestamp::now().as_microsecond();
    publish_listener(&bus, &room, seats[0].lane, micros);
    publish_listener(&bus, &room, seats[1].lane, micros + 1);
    let unknown = uuid(900_000);
    publish_listener(&bus, &room, unknown, micros + 2);
    vacate(&bus, epoch, &seats[1]);

    let answer = assert_committed(
        dispatch(&bus, &post_command(&bus, epoch, &room, "Recorded only."))
            .expect("room command dispatches"),
    );
    let CommandAnswer::RoomPost(answer) = answer else {
        panic!("room post returned another answer shape: {answer:?}");
    };
    let RoomPostOutcome::RecordedWithListeners { listeners } = answer.outcome else {
        panic!("current listeners were not reported")
    };
    assert_eq!(listeners.len(), 3);
    let occupied = listeners
        .iter()
        .find(|listener| listener.lane == seats[0].lane)
        .expect("occupied listener is present");
    assert_eq!(occupied.availability, RoomListenerAvailability::Occupied);
    assert_eq!(occupied.delivery, RoomListenerDelivery::AtSafeBoundary);
    let vacant = listeners
        .iter()
        .find(|listener| listener.lane == seats[1].lane)
        .expect("vacant listener is present");
    assert_eq!(vacant.availability, RoomListenerAvailability::Vacant);
    assert_eq!(vacant.delivery, RoomListenerDelivery::WaitsForNextStart);
    let unknown = listeners
        .iter()
        .find(|listener| listener.lane == unknown)
        .expect("unknown listener is present");
    assert_eq!(unknown.availability, RoomListenerAvailability::Unknown);
    assert_eq!(unknown.delivery, RoomListenerDelivery::PendingAvailability);
    assert!(
        bus.task_snapshot()
            .expect("post-residency reads")
            .residency()
            .expect("post-residency exists")
            .body()
            .lanes
            .iter()
            .any(|row| row.lane == seats[1].lane && row.occupant.is_none())
    );
}

#[test]
fn a_left_lane_stops_listening_until_it_contributes_again() {
    let (_root, bus, epoch, seats) = fixture("room-leave-listening", 1);
    let lane = seats[0].lane;
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-leave-listen",
        "Room",
        None,
    );
    let micros = jiff::Timestamp::now().as_microsecond();
    publish_listener(&bus, &room, lane, micros);
    assert_eq!(post_listeners(&bus, epoch, &room), vec![lane]);

    let (committed, first) = leave_answer(leave(&bus, epoch, Actor::Lane(lane), &room));
    assert!(committed && first.recorded, "{first:?}");
    assert!(first.kept.is_empty(), "{first:?}");
    assert!(post_listeners(&bus, epoch, &room).is_empty());

    let (committed, repeated) = leave_answer(leave(&bus, epoch, Actor::Lane(lane), &room));
    assert!(!committed && !repeated.recorded, "{repeated:?}");
    assert_eq!(repeated.publication, first.publication);

    publish_listener(&bus, &room, lane, micros + 1_000_000_000);
    assert_eq!(post_listeners(&bus, epoch, &room), vec![lane]);
}

#[test]
fn leaving_names_the_claim_that_keeps_a_lane_listening() {
    let (_root, bus, epoch, seats) = fixture("room-leave-native", 1);
    let lane = seats[0].lane;
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-leave-native",
        "Room",
        None,
    );
    super::resolution::claim_task(&bus, epoch, &seats[0], &Actor::Lane(lane), &room);

    let (committed, answer) = leave_answer(leave(&bus, epoch, Actor::Lane(lane), &room));
    assert!(committed && answer.recorded, "{answer:?}");
    assert_eq!(
        answer.kept,
        vec![RoomListeningDuty::Work { task: room.clone() }]
    );
    assert_eq!(post_listeners(&bus, epoch, &room), vec![lane]);
}

#[test]
fn only_a_lane_leaves_a_room() {
    let (_root, bus, epoch, _seats) = fixture("room-leave-operator", 0);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-leave-operatr",
        "Room",
        None,
    );
    assert!(matches!(
        &leave(&bus, epoch, Actor::Operator, &room),
        Outcome::Refusal(refusal) if refusal.reason() == "room::leave-lane-only"
    ));
}

#[test]
fn room_post_refuses_unknown_anchor_without_recording() {
    let (_root, bus, epoch, _seats) = fixture("room-post-unknown", 0);
    let unknown_stamp = stamp("room-post-unknown-anchor");
    let unknown = TaskHandle::for_project("board.test", &unknown_stamp);
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::RoomPost,
        vec![Witness::Task {
            task: unknown_stamp,
            head: Seq::new(1).expect("fixture position"),
        }],
        &RoomPost {
            room: unknown.clone(),
            text: "Do not record.".to_owned(),
            grooming: None,
        },
    );
    let outcome = dispatch(&bus, &command).expect("room command dispatches");
    assert!(matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "room::anchor-unknown"
    ));
    assert!(publication_page(&bus, &unknown).rows.is_empty());
}

/// Every way an anchor settles closes its room to new posts: the refusal
/// names the standing, records nothing, and the room's history keeps what it
/// held before settlement.
#[test]
fn room_post_refuses_every_settled_anchor_without_recording() {
    let (_root, bus, epoch, seats) = fixture("room-post-settled", 1);
    let closed = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-closed",
        "Closed",
        None,
    );
    let deleted = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-deleted",
        "Deleted",
        None,
    );
    let completed = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-post-completed",
        "Completed",
        Some("todo"),
    );
    for room in [&closed, &deleted, &completed] {
        assert_committed(
            dispatch(&bus, &post_command(&bus, epoch, room, "While open."))
                .expect("room command dispatches"),
        );
    }

    let heads = board(&bus, epoch, Actor::Operator);
    let witnesses = |task: &TaskHandle| {
        let heads = at(&heads, task);
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ]
    };
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskClose,
        witnesses(&closed),
        &TaskClose {
            task: closed.clone(),
            validation: "settled on the operator's word".to_owned(),
            force_claimed: false,
        },
    ));
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDelete,
        witnesses(&deleted),
        &TaskDelete {
            task: deleted.clone(),
            reason: "withdrawn".to_owned(),
            force_claimed: false,
        },
    ));
    complete_through_claim(&bus, epoch, &seats[0], &completed);

    for (room, standing) in [
        (&closed, "closed"),
        (&deleted, "deleted"),
        (&completed, "completed"),
    ] {
        let outcome = dispatch(&bus, &post_command(&bus, epoch, room, "After settlement."))
            .expect("room command dispatches");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("a post to a {standing} anchor recorded: {outcome:?}");
        };
        assert_eq!(refusal.reason(), "room::anchor-settled");
        assert_eq!(
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed),
            Some(format!("{room} {standing}").as_str())
        );
        let page = publication_page(&bus, room);
        assert_eq!(page.rows.len(), 1, "{standing} room history changed");
        assert_eq!(
            page.rows[0]
                .fact
                .read::<RoomPublished>()
                .map(|row| row.content),
            Some(RoomPublicationContent::Authored {
                text: "While open.".to_owned()
            })
        );
    }
}

/// Claim `task` from the allocator under `seat`'s lane and complete it.
fn complete_through_claim(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let held = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
            .expect("held task has completion witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "done".to_owned(),
            operation: Some(completion_operation()),
        },
    ));
}

#[test]
fn room_history_pages_posts_packets_and_coordination_in_one_order() {
    let (root, bus, epoch, seats) = fixture("room-history-coordination", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-history-coordination",
        "Room history",
        None,
    );
    assert_committed(
        dispatch(&bus, &post_command(&bus, epoch, &room, "before")).expect("room post dispatches"),
    );
    publish_packet(&bus, &room, &seats[0]);
    assert_committed(coordinate(
        &bus,
        &coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Start),
    ));
    assert_committed(
        dispatch(&bus, &post_command(&bus, epoch, &room, "after")).expect("room post dispatches"),
    );
    assert_committed(put_aside(&bus, &put_aside_command(&bus, epoch, &room)));

    let first = history_page(&root, &[room.as_str(), "--limit", "3"]);
    assert_eq!(first["page"]["omission"]["population"], 5);
    assert_eq!(first["page"]["omission"]["shown"], 3);
    assert_eq!(first["page"]["omission"]["withheld"], 2);
    let first_rows = first["page"]["rows"].as_array().expect("first page rows");
    assert_eq!(
        first_rows
            .iter()
            .map(|row| row["content"]["state"].as_str())
            .collect::<Vec<_>>(),
        [Some("text"), Some("unavailable"), Some("linked")]
    );
    let continuation: Vec<String> =
        serde_json::from_value(first["page"]["omission"]["continuation"].clone())
            .expect("continuation is one command");
    let words = continuation[2..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let second = history_page(&root, &words);
    assert_eq!(second["page"]["rows"].as_array().map(Vec::len), Some(2));
    assert!(second["page"]["omission"].is_null());
    let second_rows = second["page"]["rows"].as_array().expect("second page rows");
    assert_eq!(
        second_rows
            .iter()
            .map(|row| row["content"]["state"].as_str())
            .collect::<Vec<_>>(),
        [Some("text"), Some("linked")]
    );

    let shown = first_rows
        .iter()
        .chain(second_rows)
        .map(|row| row["seq"].clone())
        .collect::<Vec<_>>();
    let indexed = publication_page(&bus, &room)
        .rows
        .into_iter()
        .map(|row| serde_json::to_value(row.seq).expect("sequence encodes"))
        .collect::<Vec<_>>();
    assert_eq!(shown, indexed, "history and publication index order agree");
    for row in first_rows.iter().chain(second_rows) {
        if row["content"]["state"] == "linked" {
            assert_eq!(row["content"]["source"]["kind"], "journal");
            let fact = row["content"]["source"]["fact"]
                .as_str()
                .expect("journal fact is base-52")
                .parse::<wire::SequenceCode>()
                .expect("journal fact decodes");
            assert_eq!(fact.get(), row["seq"].as_u64().expect("row sequence"));
        }
    }
}

#[test]
fn room_coordination_waits_for_native_capture_and_stop_clears_on_native_free() {
    let (root, bus, epoch, seats) = fixture("room-coordinate-native", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-native",
        "Room controller",
        None,
    );
    let start = coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Start);
    let first = coordinate(&bus, &start);
    let started = assert_committed(first.clone());
    let replay_command = start.clone();
    assert_eq!(
        bus.read(move |read| read.command_replay(&replay_command))
            .expect("coordination retry receipt reads"),
        Some(journal::CommandReplay::Exact(first)),
    );
    let CommandAnswer::RoomCoordination(started) = started else {
        panic!("start returned another answer shape: {started:?}");
    };
    assert_eq!(started.requests.len(), 1);
    assert_eq!(
        coordination_lane(&bus, started.requests[0].target),
        Some(seats[0].lane),
    );
    assert!(!started.hide);
    let key = started.requests[0].key.clone();
    let sent = bus
        .fact_snapshot()
        .expect("coordination facts read")
        .facts()
        .iter()
        .find_map(|envelope| {
            envelope
                .fact
                .read::<wire::body::SteerSent>()
                .filter(|sent| sent.key == key)
        })
        .expect("coordination direction is durable");
    let named = sent
        .text
        .split_once("`spice task capture ")
        .and_then(|(_, rest)| rest.split_once('`'))
        .map(|(handle, _)| handle)
        .expect("direction names a native capture command")
        .parse::<TaskHandle>()
        .expect("direction names a valid task handle");
    assert_eq!(named, room);
    let duplicate = coordinate(
        &bus,
        &coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Resume),
    );
    assert!(
        matches!(duplicate, Outcome::Refusal(_)),
        "an unanswered scoped request is not duplicated: {duplicate:?}",
    );

    let lane = Actor::Lane(seats[0].lane);
    let available = board(&bus, epoch, lane.clone());
    let rule: wire::Name = "capture".parse().expect("named capture rule");
    let heads = at(&available, &named);
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &wire::intent::ClaimWin {
            rule,
            recovery: None,
        },
    ));
    let aside = assert_committed(put_aside(&bus, &put_aside_command(&bus, epoch, &room)));
    let CommandAnswer::RoomCoordination(aside) = aside else {
        panic!("put-aside returned another answer shape: {aside:?}");
    };
    let stop = aside
        .stop
        .expect("active native holder receives a stop request");
    assert!(aside.hide);
    assert_eq!(coordination_lane(&bus, stop.target), Some(seats[0].lane),);
    let visible = coordination_status(&bus, &room);
    assert!(visible.requests.is_empty());
    let publications = publication_page(&bus, &room).rows.len();
    let repeated = put_aside(&bus, &put_aside_command(&bus, epoch, &room));
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(_),
        body: CommandAnswer::RoomCoordination(repeated),
    }) = repeated
    else {
        panic!("unchanged put-aside returned another outcome: {repeated:?}");
    };
    assert!(repeated.hide);
    assert_eq!(repeated.stop, visible.stop);
    assert_eq!(publication_page(&bus, &room).rows.len(), publications);
    assert_eq!(visible.stop, Some(stop));

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        claim_free_witnesses(&bus, epoch, &lane, &seats[0], &room),
        &TaskRef { task: room.clone() },
    ));
    assert_eq!(coordination_status(&bus, &room).stop, None);
    clean(&root, bus);
}

#[test]
fn room_coordination_resolves_only_an_occupied_seat_address() {
    let (root, bus, epoch, seats) = fixture("room-coordinate-seat-state", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let mut current = board(&bus, epoch, Actor::Operator);
    let target = current
        .addresses
        .get(&seats[0].lane)
        .and_then(wire::SeatAddress::address)
        .expect("fixture seat has a proved address");
    assert_eq!(
        coordinate_target(&current, target).expect("occupied seat resolves"),
        (seats[0].lane, seats[0].occupant)
    );

    current.residency.body.lanes[0].disposition = crate::resident::SIMULATED_DISPOSITION.to_owned();
    let Err(Outcome::Refusal(refusal)) = coordinate_target(&current, target) else {
        panic!("a simulated seat address must refuse before command submission");
    };
    assert_eq!(refusal.reason(), "room::address-unoccupied");
    assert!(
        refusal.explanation().contains(&target.to_string())
            && refusal
                .explanation()
                .contains(crate::resident::SIMULATED_DISPOSITION),
        "the refusal names both the submitted address and seat disposition: {refusal:?}"
    );
    clean(&root, bus);
}

#[test]
fn room_coordination_vacancy_race_records_nothing() {
    let (root, bus, epoch, seats) = fixture("room-coordinate-vacancy", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-vacancy",
        "Room controller",
        None,
    );
    let command = coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Start);
    vacate(&bus, epoch, &seats[0]);
    assert!(
        !matches!(coordinate(&bus, &command), Outcome::Reply(_)),
        "a stale occupied-lane witness refuses the whole coordination write",
    );
    assert!(publication_page(&bus, &room).rows.is_empty());
    clean(&root, bus);
}

#[test]
fn room_coordination_refuses_a_lane_that_won_other_work() {
    let (root, bus, epoch, seats) = fixture("room-coordinate-busy", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let other = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-other",
        "Other work",
        None,
    );
    let lane = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &lane, &other);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-busy",
        "Room controller",
        None,
    );

    let outcome = coordinate(
        &bus,
        &coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Start),
    );
    assert!(
        matches!(outcome, Outcome::Refusal(_)),
        "a lane cannot acquire a second native claim: {outcome:?}",
    );
    assert!(publication_page(&bus, &room).rows.is_empty());
    clean(&root, bus);
}

#[test]
fn room_coordination_refuses_a_claim_won_after_validation_without_recording() {
    let (root, bus, epoch, seats) = fixture("room-coordinate-claim-race", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let room = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-race-room",
        "Room controller",
        None,
    );
    let other = add(
        &bus,
        epoch,
        Actor::Operator,
        "room-coordinate-race-other",
        "Other work",
        None,
    );
    let command = coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Start);
    let lane = Actor::Lane(seats[0].lane);
    let outcome = coordinate_after_validation(&bus, &command, || {
        claim_task(&bus, epoch, &seats[0], &lane, &other);
    });
    assert!(
        matches!(&outcome, Outcome::Refusal(refusal) if refusal.reason() == "room::lane-busy"),
        "a claim won after validation must refuse the coordination write: {outcome:?}",
    );

    let room_stamp = room.born().expect("fixture room has a birth stamp");
    let control = CoordinationControl::Coordinate(room_stamp.clone()).to_string();
    let facts = bus.fact_snapshot().expect("coordination race facts read");
    assert!(
        !facts.facts().iter().any(|envelope| {
            envelope
                .fact
                .read::<wire::body::SteerSent>()
                .is_some_and(|sent| sent.control.as_deref() == Some(&control))
        }),
        "refused coordination published a steering direction",
    );
    assert!(
        !facts.facts().iter().any(|envelope| {
            envelope
                .fact
                .read::<RoomCoordinationStarted>()
                .is_some_and(|started| started.room == room_stamp)
        }),
        "refused coordination published a room record",
    );

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        claim_free_witnesses(&bus, epoch, &lane, &seats[0], &other),
        &TaskRef {
            task: other.clone(),
        },
    ));
    claim_task(&bus, epoch, &seats[0], &lane, &room);
    assert_committed(coordinate(
        &bus,
        &coordinate_command(&bus, epoch, &room, &seats[0], RoomCoordinateAction::Resume),
    ));
    clean(&root, bus);
}

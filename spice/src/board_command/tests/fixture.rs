use super::*;

pub(super) static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
pub(super) static NEXT_COMMAND: AtomicU64 = AtomicU64::new(100);

#[derive(Clone, Debug)]
pub(super) struct Seat {
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
}

pub(super) fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID is canonical")
}

pub(super) fn stamp(text: &str) -> wire::TaskStamp {
    let value = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    wire::TaskStamp::for_millis(value).expect("fixture name fits the task stamp codec")
}

pub(super) fn fixture(name: &str, lane_count: usize) -> (PathBuf, Bus, CanonicalUuid, Vec<Seat>) {
    let number = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "spice-board-command-{name}-{}-{number}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("fixture directory exists");
    let path = root.join("authority.sqlite3");
    let journal = journal::Journal::open(&path).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = uuid(number + 1);
    let seats = (0..lane_count)
        .map(|index| Seat {
            lane: uuid(number * 100 + index as u64 + 10),
            occupant: uuid(number * 100 + index as u64 + 30),
        })
        .collect::<Vec<_>>();
    let lanes = seats
        .iter()
        .enumerate()
        .map(|(index, seat)| wire::LaneResidency {
            lane: seat.lane,
            lineage_id: seat.lane,
            name: None,
            worktree: "/fixture".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(seat.occupant),
            lineage: vec![seat.occupant],
            capabilities: vec![uuid(number * 100 + index as u64 + 50)],
            session: None,
            disposition: "occupied".to_owned(),
        })
        .collect::<Vec<_>>();
    let subscriptions = lanes
        .iter()
        .map(|lane| wire::LineageSubscription {
            lineage_id: lane.lineage_id,
            policy: wire::intent::LineageLife::Drain,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        })
        .collect::<Vec<_>>();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("resident seating fact builds");
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        lanes,
        subscriptions,
    });
    match bus.commit(write).expect("fixture seating reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("fixture seating did not commit: {outcome:?}"),
    }
    (root, bus, epoch, seats)
}

pub(super) fn issue<B: Serialize>(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, kind, expected, body);
    dispatch(bus, &command).expect("board command owns fixture kind")
}

pub(super) fn issue_with_allocation<B: Serialize>(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, kind, expected, body);
    let projects = ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("board command test project policy");
    dispatch_with_repository(bus, &command, None, &projects, allocation)
        .expect("board command owns fixture kind")
}

pub(super) fn fixture_command<B: Serialize>(
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> wire::AdmittedCommand {
    Command {
        v: WireVersion::CURRENT,
        id: uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed)),
        epoch,
        kind,
        expected,
        body: serde_json::to_value(body).expect("fixture command body encodes"),
    }
    .admit(&Accepted {
        wire: WireVersion::CURRENT,
        actor,
        epoch,
    })
    .expect("fixture command admits")
}

pub(super) fn assert_committed(outcome: Outcome<CommandAnswer>) -> CommandAnswer {
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(_),
        body,
    }) = outcome
    else {
        panic!("command did not commit: {outcome:?}");
    };
    body
}

pub(super) fn board(bus: &Bus, epoch: CanonicalUuid, actor: Actor) -> BoardAnswer {
    let outcome = issue(
        bus,
        epoch,
        actor,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    );
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::Board(answer),
    }) = outcome
    else {
        panic!("board read did not return a snapshot: {outcome:?}");
    };
    assert_eq!(position, answer.cursor);
    *answer
}

pub(super) fn board_with_allocation(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    allocation: &board::NextConfig,
) -> BoardAnswer {
    let outcome = issue_with_allocation(
        bus,
        epoch,
        actor,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
        allocation,
    );
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::Board(answer),
    }) = outcome
    else {
        panic!("configured board read did not return a snapshot: {outcome:?}");
    };
    assert_eq!(position, answer.cursor);
    *answer
}

pub(super) fn add(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &str,
    title: &str,
    flow: Option<&str>,
) -> TaskHandle {
    add_project(bus, epoch, actor, task, title, "board.test", flow)
}

pub(super) fn add_project(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &str,
    title: &str,
    project: &str,
    flow: Option<&str>,
) -> TaskHandle {
    add_project_with_acceptance(bus, epoch, actor, task, title, project, flow, &[])
}

#[allow(
    clippy::too_many_arguments,
    reason = "task fixtures retain the command's authored fields"
)]
pub(super) fn add_project_with_acceptance(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &str,
    title: &str,
    project: &str,
    flow: Option<&str>,
    acceptance: &[&str],
) -> TaskHandle {
    let incepted = stamp(task);
    let task = TaskHandle::for_project(project, &incepted);
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: incepted }],
        &TaskAdd {
            title: title.to_owned(),
            project: Some(project.to_owned()),
            description: None,
            acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
            origin: None,
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
        },
    ));
    task
}

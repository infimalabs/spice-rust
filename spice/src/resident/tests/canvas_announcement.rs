//! Canvas announcement, opening, and captured-activity regressions.
//!
//! The `pub(super)` helpers below are the deliberate seam shared with the
//! resident lifecycle cases that remain in the parent test module.

use super::*;

pub(super) fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
    let root = fixture(name);
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.data).expect("data directory");
    (root, paths)
}

pub(super) fn process_spec(command: &str, runtime: Duration) -> Spec {
    Spec {
        argv: Argv::new("/bin/sh", ["-c", command]).expect("fixture argv"),
        cwd: PathBuf::from("/"),
        environment: Environment::new([("PATH", "/usr/bin:/bin")]).expect("fixture environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
            .expect("fixture deadlines"),
        wait: WaitPolicy::new(
            "resident fixture",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .expect("fixture wait policy"),
        output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
        redaction: Redaction::None,
        grace: Duration::from_millis(100),
    }
}

pub(super) fn seed_lane(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    seed_lane_in_lineage(bus, epoch, lane, lane, occupant);
}

pub(super) fn seed_lane_in_lineage(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    lineage_id: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    let current = current_residency(bus, epoch).expect("current residency");
    let mut head = current.body().clone();
    head.topology_revision = head
        .topology_revision
        .checked_add(1)
        .expect("fixture topology revision advances");
    head.lanes.push(wire::LaneResidency {
        lane,
        lineage_id,
        name: None,
        worktree: "/fixture".to_owned(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![test_uuid(9)],
        session: None,
        disposition: "occupied".to_owned(),
    });
    if !head
        .subscriptions
        .iter()
        .any(|subscription| subscription.lineage_id == lineage_id)
    {
        head.subscriptions.push(wire::LineageSubscription {
            lineage_id,
            policy: wire::intent::LineageLife::Drive,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        });
        head.subscriptions
            .sort_by_key(|subscription| subscription.lineage_id);
    }
    head.lanes.sort_by_key(|lane| lane.lane);
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id,
                capability: test_uuid(9),
                name: None,
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane seats");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 123,
            },
            FactCause::Pending(seated),
        )
        .expect("occupant starts");
    write.residency_head(started, move |_| head);
    commit(bus, write).expect("fixture lane commits");
}

pub(super) fn fact_u64(snapshot: &journal::FactSnapshot, kind: wire::FactKind, field: &str) -> u64 {
    snapshot
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == kind)
        .and_then(|fact| fact.fact.body().get(field))
        .and_then(|value| value.as_u64())
        .expect("fact carries numeric fixture field")
}

pub(super) fn test_uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}

/// One published discovery, at a path the caller owns for the test.
///
/// The returned possession is the publisher, holding the runtime lock and
/// naming the generation it published. A resident announces its own discovery
/// while it is running, and a client admits that record only from a holder
/// that claims it, so a fixture missing either half would be announcing a
/// record a client is required to refuse — the caller has to keep the
/// possession alive for the announcement.
fn published_discovery(
    name: &str,
    port: u16,
) -> (PathBuf, PathBuf, canvas::Discovery, sill::Possession) {
    let root = fixture(name);
    fs::create_dir_all(&root).expect("announcement fixture root");
    let path = root.join("canvas");
    let discovery = canvas::Discovery {
        endpoint: format!("http://127.0.0.1:{port}"),
        capability: test_uuid(1),
        epoch: test_uuid(2),
    };
    discovery
        .publish(&path)
        .expect("fixture discovery publishes");
    let lock = sill::LockFile::new(
        sill::LockName::new("resident").expect("resident is a non-empty lock name"),
        root.join("resident"),
    );
    let sill::Acquisition::Acquired(publisher) = lock.try_acquire().expect("fixture resident lock")
    else {
        panic!("an announcement fixture nothing else has seen cannot be contended");
    };
    publisher
        .rewrite_record(holder_record(std::process::id(), Some(discovery.epoch)).as_bytes())
        .expect("fixture holder record writes");
    (root, path, discovery, publisher)
}

/// The announced endpoint is one that passed admission, and nothing dials it.
///
/// This is the whole reason `up` reads its own discovery back rather than
/// formatting the address it bound: the operator is handed a URL only after
/// the same ownership, loopback-origin, and generation checks a client makes.
/// A plain start must not seize the operator's browser, so the launcher stays
/// untouched until `--open` asks for it.
#[test]
fn announcing_the_canvas_names_the_admitted_endpoint_without_dialling_it() {
    let (root, path, discovery, _publisher) = published_discovery("announce-endpoint", 48231);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        None,
        false,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {}\n", discovery.endpoint)
    );
    assert!(
        diagnostics.is_empty(),
        "an admitted endpoint is not a diagnostic: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    assert_eq!(dialled, None, "a plain start must not open a browser");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// `--open` hands the launcher the admitted endpoint and no other.
#[test]
fn opening_at_start_dials_exactly_the_endpoint_that_was_announced() {
    let (root, path, discovery, _publisher) = published_discovery("announce-open", 48232);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        None,
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(dialled.as_deref(), Some(discovery.endpoint.as_str()));
    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {}\n", discovery.endpoint)
    );
    assert!(
        diagnostics.is_empty(),
        "an opened endpoint is not a diagnostic: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// An authenticated opening keeps its bearer outside the HTTP origin.
#[test]
fn authenticated_opening_at_start_carries_the_token_as_a_fragment() {
    const TOKEN: &str = "correct-horse-battery-staple";

    let (root, path, discovery, _publisher) = published_discovery("announce-authenticated", 48234);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        Some(TOKEN),
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    let opening = format!("{}#{TOKEN}", discovery.endpoint);
    assert_eq!(dialled.as_deref(), Some(opening.as_str()));
    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {opening}\n")
    );
    assert_eq!(
        opening.split_once('#').expect("opening has fragment").0,
        discovery.endpoint,
        "the token must not become part of the HTTP origin"
    );
    assert!(diagnostics.is_empty());
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// A generation replaced under the announcement is refused, not announced.
///
/// Folding the opener into `up` only preserves the admission it replaced if
/// the resident's own read is subject to it. Publishing a new generation
/// between the read and the identity recheck is the exact race that check
/// exists for, and it must reach the announcement as a refusal rather than as
/// a URL belonging to the generation that just lost.
///
/// It must also reach a different stream. Since the endpoint moved to stdout so
/// a shell can capture it, anything else written there is read as an address,
/// and this is the one case that produces something else.
#[test]
fn announcing_the_canvas_refuses_a_generation_that_was_replaced() {
    let (root, path, discovery, _publisher) = published_discovery("announce-stale", 48233);
    let replacement = canvas::Discovery {
        capability: test_uuid(3),
        epoch: test_uuid(4),
        ..discovery.clone()
    };
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        None,
        true,
        || {
            replacement
                .publish(&path)
                .expect("replacement publishes before the identity recheck");
        },
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(
        String::from_utf8(diagnostics).expect("diagnostic is text"),
        "canvas unavailable: canvas::discovery-replaced\n"
    );
    assert!(
        announced.is_empty(),
        "a refusal reason is not an endpoint, and a shell reading the endpoint \
         stream would dial it: {}",
        String::from_utf8_lossy(&announced)
    );
    assert_eq!(dialled, None, "a stale generation must not be dialled");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

#[test]
fn durable_capture_projects_current_occupant_tool_and_prose_with_event_identity() {
    let (root, paths) = resident_fixture("capture-rendering");
    let bus = Bus::start(journal::Journal::in_memory().expect("message authority opens"))
        .expect("message bus starts");
    let source = canvas::stream::Source::new(bus.reader(), test_uuid(9), 8);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let locus = listen::Locus::new(4, 120, 48);
    let record = lane::capture::CaptureRecord {
        lane: test_uuid(5),
        occupant: test_uuid(6),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events: vec![
                listen::Event::Reasoning(listen::Reasoning {
                    content: listen::ReasoningContent::Opaque,
                    locus,
                }),
                listen::Event::Tool(listen::Tool {
                    call_id: "tool-1".to_owned(),
                    moment: listen::ToolMoment::Call {
                        name: "Bash".to_owned(),
                        arguments:
                            r#"{"command":"git status --short","description":"read the tree"}"#
                                .to_owned(),
                    },
                    locus,
                }),
                listen::Event::Prose(listen::Prose {
                    role: listen::ProseRole::Assistant,
                    text: "agent-authored answer".to_owned(),
                    phase: None,
                    locus,
                }),
            ],
        },
    };
    let mut seats = Seats::new();
    seats
        .seat(
            record.lane,
            record.occupant,
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("capture fixture occupant starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher);

    accept_capture_record(
        &bus,
        test_uuid(4),
        &mut seats,
        &surfaces,
        &record,
        &mut previews,
        &mut diagnostics,
    )
    .expect("current capture projects");
    assert_tool_and_prose_activity(&mut updates, &record);
    assert_stale_capture_ignored(
        &bus,
        &mut seats,
        &surfaces,
        &mut updates,
        &record,
        &mut previews,
        &mut diagnostics,
    );
    seats
        .vacate(record.lane)
        .expect("replacement occupant remains supervised")
        .wait()
        .expect("replacement occupant stops");
    drop(source);
    bus.shutdown().expect("message bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn the_admitted_capture_path_commits_taste_before_projecting_activity() {
    let (root, paths) = resident_fixture("capture-taste");
    let repository = Repository::discover(&root).expect("fixture repository");
    let surface = crate::trust_command::Surface::load(&repository).expect("trust surface");
    let bus = Bus::start(journal::Journal::in_memory().expect("message authority opens"))
        .expect("message bus starts");
    let epoch = test_uuid(4);
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    seed_lane(&bus, epoch, lane, occupant);
    let source = canvas::stream::Source::new(bus.reader(), test_uuid(9), 8);
    let publisher = source.message_publisher();
    let locus = listen::Locus::new(4, 120, 48);
    let record = lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: "Polling is not a completion event.".to_owned(),
                phase: None,
                locus,
            })],
        },
    };
    let bag = taste::Bag::new(vec![
        taste::Taste::new(
            "polling".parse().expect("taste name"),
            ["poll*"],
            "Wait for a terminal event.",
        )
        .expect("taste row"),
    ])
    .expect("taste bag");
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("capture fixture occupant starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &bag, &steering, &publisher);

    for _ in 0..2 {
        accept_capture_record(
            &bus,
            epoch,
            &mut seats,
            &surfaces,
            &record,
            &mut previews,
            &mut diagnostics,
        )
        .expect("current capture admits");
    }

    let snapshot = bus.fact_snapshot().expect("capture facts read");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::TasteReminded)
            .count(),
        1,
        "the duplicate durable vendor record is suppressed in the same epoch"
    );
    assert_eq!(
        steer::Ledger::from_snapshot(&snapshot)
            .expect("reminder replay")
            .pending(lane)
            .len(),
        1
    );
    assert!(
        fs::read_to_string(steer::inbox::path(&paths.inbox, lane))
            .expect("inbox rendering")
            .contains("[TASTE] polling: Wait for a terminal event.")
    );
    assert!(diagnostics.is_empty());
    seats
        .vacate(lane)
        .expect("occupant remains supervised")
        .wait()
        .expect("occupant stops");
    drop(source);
    bus.shutdown().expect("message bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn assert_tool_and_prose_activity(
    updates: &mut tokio::sync::broadcast::Receiver<wire::AgentMessage>,
    record: &lane::capture::CaptureRecord,
) {
    let tool = updates.try_recv().expect("tool activity publishes once");
    assert_eq!(tool.lane, record.lane);
    assert_eq!(tool.author, record.occupant);
    assert_eq!(tool.activity, wire::AgentActivityKind::Tool);
    assert_eq!(tool.text, "Bash: git status --short");
    assert_eq!(
        tool.locus,
        wire::MessageLocus {
            member: 4,
            offset: 120,
            length: 48,
            event: 1,
        }
    );

    let prose = updates.try_recv().expect("prose publishes once");
    assert_eq!(prose.lane, record.lane);
    assert_eq!(prose.author, record.occupant);
    assert_eq!(prose.activity, wire::AgentActivityKind::Prose);
    assert_eq!(prose.text, "agent-authored answer");
    assert_eq!(
        prose.locus,
        wire::MessageLocus {
            member: 4,
            offset: 120,
            length: 48,
            event: 2,
        }
    );
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

fn assert_stale_capture_ignored<P: AgentMessagePublisher>(
    bus: &bus::Bus,
    seats: &mut Seats,
    surfaces: &CaptureSurfaces<'_, P>,
    updates: &mut tokio::sync::broadcast::Receiver<wire::AgentMessage>,
    record: &lane::capture::CaptureRecord,
    previews: &mut CallPreviews,
    diagnostics: &mut Vec<u8>,
) {
    seats
        .vacate(record.lane)
        .expect("original occupant remains supervised")
        .wait()
        .expect("original occupant stops");
    seats
        .seat(
            record.lane,
            test_uuid(7),
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("replacement occupant starts");
    let locus = listen::Locus::new(4, 180, 48);
    let stale = lane::capture::CaptureRecord {
        lane: record.lane,
        occupant: record.occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: "stale prior-occupant answer".to_owned(),
                phase: None,
                locus,
            })],
        },
    };
    accept_capture_record(
        bus,
        test_uuid(4),
        seats,
        surfaces,
        &stale,
        previews,
        diagnostics,
    )
    .expect("stale capture is ignored");
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(diagnostics.is_empty());
}

/// A generation that was never published is a diagnostic, not a silent start.
///
/// This is the operator's first start, before anything has been published, and
/// it reaches the announcement through `DiscoveryRead::open` rather than the
/// identity recheck the replaced-generation case above exercises.
///
/// Both arrive as *faults*: the announcement boundary accepts only a typed
/// endpoint or fault, so no unreachable refusal arm can intercept this route.
#[test]
fn announcing_the_canvas_without_a_generation_names_no_endpoint() {
    let root = fixture("announce-missing");
    fs::create_dir_all(&root).expect("announcement fixture root");
    let path = root.join("canvas");
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        None,
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    let diagnostic = String::from_utf8(diagnostics).expect("diagnostic is text");
    assert!(
        diagnostic.starts_with("canvas unavailable: "),
        "a missing generation must be reported: {diagnostic}"
    );
    assert!(
        announced.is_empty(),
        "a shell capturing the endpoint stream would dial this: {}",
        String::from_utf8_lossy(&announced)
    );
    assert_eq!(dialled, None, "a missing generation must not be dialled");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// One codex usage record, the spend a live capture record leaves behind it.
const CODEX_USAGE: &[u8] = br#"{"type":"turn.completed","usage":{"input_tokens":11,"cached_input_tokens":7,"cache_write_input_tokens":3,"output_tokens":5,"reasoning_output_tokens":2}}
"#;

/// A live capture record accounts the tape its lane has been writing.
///
/// The record carries decoded events, but the spend lives in the tape behind
/// it, and the head only moves when something reads that tape. Asserting on the
/// head rather than on the call is what makes this the live arm's proof: a
/// resident that merely compiled the ingestion call beside its capture handling
/// leaves the head absent, and the tape's bytes are then accounted only if some
/// later start happens to sweep them.
#[test]
fn a_live_capture_record_accounts_the_tape_behind_its_lane() {
    let (root, paths) = resident_fixture("capture-ingestion");
    let bus = Bus::start(journal::Journal::in_memory().expect("ingestion authority opens"))
        .expect("ingestion bus starts");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let epoch = test_uuid(4);
    seat_ingestion_residency(&bus, lane, epoch);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the lane tape opens");
    tape.tee(CODEX_USAGE).expect("the usage record is durable");

    let source = canvas::stream::Source::new(bus.reader(), test_uuid(9), 8);
    let publisher = source.message_publisher();
    let locus = listen::Locus::new(1, 0, CODEX_USAGE.len() as u64);
    let record = lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: "agent-authored answer".to_owned(),
                phase: None,
                locus,
            })],
        },
    };
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("ingestion fixture occupant starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher);

    assert_eq!(
        ingested_head(&bus, lane),
        None,
        "the head stood before any record was accepted",
    );
    accept_capture_record(
        &bus,
        epoch,
        &mut seats,
        &surfaces,
        &record,
        &mut previews,
        &mut diagnostics,
    )
    .expect("the live capture record is accepted");

    assert_eq!(
        ingested_head(&bus, lane).map(|head| head.next),
        Some(wire::body::TranscriptPosition {
            member: 1,
            offset: CODEX_USAGE.len() as u64,
        }),
        "accepting a live capture record left its tape unaccounted: {}",
        String::from_utf8_lossy(&diagnostics),
    );
    fs::remove_dir_all(&root).expect("ingestion fixture cleanup");
}

/// Seat one codex lane so the ingestion sweep can find it and its tape.
fn seat_ingestion_residency(bus: &Bus, lane: CanonicalUuid, epoch: CanonicalUuid) {
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    let seated = write
        .fact(wire::body::UpSeated { epoch }, FactCause::Root)
        .expect("resident fact");
    write.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: "/ingestion/worktree".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            occupant: None,
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
    });
    bus.commit(write).expect("residency seeds");
}

/// The stored transcript head for one lane's codex session stream.
fn ingested_head(bus: &Bus, lane: CanonicalUuid) -> Option<wire::TranscriptSourceHead> {
    bus.transact(move |authority| {
        let vendor = "codex".parse().expect("fixture vendor");
        let key = wire::TranscriptStreamKey::new(&lane, &vendor, "session");
        let head = authority
            .read(|read| read.transcript_source_head(&key))
            .expect("the stored head reads");
        bus::JournalTransaction::settled(head.map(|head| head.into_body()))
    })
    .expect("head transaction")
}

//! Canvas announcement and opening checks with resident fixture helpers.
//!
//! The `pub(super)` helpers below are the deliberate seam shared with the
//! resident lifecycle cases that remain in the parent test module.

use super::*;

pub(super) fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
    let root = fixture(name);
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let root = fs::canonicalize(root).expect("fixture root canonicalizes");
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
    worktree: &std::path::Path,
) {
    seed_lane_in_team(bus, epoch, lane, lane, occupant, worktree);
}

pub(super) fn seed_lane_in_team(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    team: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: &std::path::Path,
) {
    let worktree = worktree
        .canonicalize()
        .expect("fixture worktree canonicalizes")
        .to_string_lossy()
        .into_owned();
    seed_lane_in_team_at(bus, epoch, lane, team, Some(occupant), &worktree, "claude");
}

pub(super) fn seed_lane_in_worktree(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: &str,
) {
    seed_lane_in_team_at(bus, epoch, lane, lane, Some(occupant), worktree, "claude");
}

/// Seat one occupied lane run by the named vendor.
pub(super) fn seed_vendor_lane_in_worktree(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: &str,
    vendor: &str,
) {
    seed_lane_in_team_at(bus, epoch, lane, lane, Some(occupant), worktree, vendor);
}

/// Seat one lane nobody sits in yet.
pub(super) fn seed_vacant_lane_in_worktree(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    worktree: &str,
) {
    seed_lane_in_team_at(bus, epoch, lane, lane, None, worktree, "claude");
}

fn seed_lane_in_team_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    team: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    worktree: &str,
    vendor: &str,
) {
    let vendor: wire::Name = vendor.parse().expect("fixture vendor");
    let current = current_residency(bus, epoch).expect("current residency");
    let mut head = current.body().clone();
    head.topology_revision = head
        .topology_revision
        .checked_add(1)
        .expect("fixture topology revision advances");
    head.lanes.push(wire::LaneResidency {
        lane,
        team,
        name: None,
        worktree: worktree.to_owned(),
        life: "drive".to_owned(),
        vendor: vendor.clone(),
        model: None,
        effort: None,
        occupant,
        lineage: occupant.into_iter().collect(),
        capabilities: vec![test_uuid(9)],
        session: None,
        disposition: if occupant.is_some() {
            "occupied"
        } else {
            "vacant"
        }
        .to_owned(),
    });
    if !head
        .subscriptions
        .iter()
        .any(|subscription| subscription.team == team)
    {
        head.subscriptions.push(wire::TeamSubscription {
            team,
            policy: wire::intent::TeamLife::Drive,
        });
        head.subscriptions
            .sort_by_key(|subscription| subscription.team);
    }
    head.lanes.sort_by_key(|lane| lane.lane);
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                team,
                capability: test_uuid(9),
                name: None,
                worktree: worktree.to_owned(),
                life: "drive".to_owned(),
                vendor,
                model: None,
                effort: None,
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane seats");
    let last = match occupant {
        Some(occupant) => write
            .fact(
                wire::body::LaneStarted {
                    lane,
                    occupant,
                    epoch,
                    group: 123,
                },
                FactCause::Pending(seated),
            )
            .expect("occupant starts"),
        None => seated,
    };
    write.residency_head(last, move |_| head);
    commit(bus, write).expect("fixture lane commits");
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

/// Seat one codex lane so the ingestion sweep can find it and its tape.
pub(super) fn seat_ingestion_residency(bus: &Bus, lane: CanonicalUuid, epoch: CanonicalUuid) {
    seat_codex_residency(bus, lane, None, epoch);
}

/// Seat one codex lane whose occupant already declared its session: the row
/// a lane-authenticated reader such as the hook's working state recognizes.
pub(super) fn seat_occupied_ingestion_residency(
    bus: &Bus,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
) {
    seat_codex_residency(bus, lane, Some(occupant), epoch);
}

fn seat_codex_residency(
    bus: &Bus,
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
) {
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
            team: lane,
            name: None,
            worktree: "/ingestion/worktree".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            model: None,
            effort: None,
            occupant,
            lineage: occupant.into_iter().collect(),
            capabilities: Vec::new(),
            session: occupant.map(|occupant| wire::LaneSession {
                lane,
                occupant,
                worktree: "/ingestion/worktree".to_owned(),
                vendor: "codex".parse().expect("fixture vendor"),
                launch: None,
                epoch,
                stream: "stdout".to_owned(),
                identity: "thread-ingestion".to_owned(),
                locus: wire::TapeLocus {
                    member: 0,
                    offset: 0,
                    length: 0,
                },
            }),
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    bus.commit(write).expect("residency seeds");
}

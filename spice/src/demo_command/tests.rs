use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::intent::{BoardAnswer, BoardRead, CommandAnswer, LaneAnswer, LaneRead};
use wire::{
    Command, CommandKind, ResidentState, Response, StreamFrame, StreamRequest, ViewSubject,
    WireVersion,
};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

const FIXTURE_APP_IDENTITY: &str =
    "5555555555555555555555555555555555555555555555555555555555555555";

/// A staged lane app the seed copies into the demo root, so the stage
/// itself lives only as long as the seed that reads it.
fn fixture_app() -> (Scratch, serve::AppSource) {
    let root = Scratch::new("app");
    stage_fixture_app(&root.0, FIXTURE_APP_IDENTITY, "demo fixture app");
    let source = serve::AppSource::resolve(&root.0).expect("demo fixture app resolves");
    (root, source)
}

fn stage_fixture_app(root: &Path, identity: &str, marker: &str) {
    let stage = root.join(serve::APP_STAGE);
    let generation = stage.join(serve::APP_STAGE_GENERATIONS).join(identity);
    fs::create_dir_all(&generation).expect("demo fixture app generation");
    for asset in serve::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("{marker} {}\n", asset.file()),
        )
        .expect("demo fixture app asset");
    }
    fs::write(
        stage.join(serve::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("demo fixture app pointer");
}

fn seed_test_environment(root: Option<&Path>, executable: &Path) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let (root, automatic) = prepare_root(root)?;
    let (_stage, app) = fixture_app();
    let result = born_repository(&root)
        .and_then(|()| finish_seed_prepared_root(&root, executable, automatic, &app));
    result.map_err(|failure| preserve_failure(&root, executable, *failure).into())
}

fn seed_test_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
) -> DemoResult<Environment> {
    born_repository(root)?;
    let (_stage, app) = fixture_app();
    finish_seed_prepared_root(root, executable, automatic, &app)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-demo-unit-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("demo fixture root is exclusively created");
        Self(root)
    }

    fn child(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn new_seed_copies_the_invoking_linked_worktree_app() {
    const APPLIANCE_IDENTITY: &str =
        "4444444444444444444444444444444444444444444444444444444444444444";
    const LINKED_IDENTITY: &str =
        "6666666666666666666666666666666666666666666666666666666666666666";

    let scratch = Scratch::new("linked-app-source");
    let appliance = scratch.child("appliance-source");
    let common = appliance.join(".git");
    let linked_git = common.join("worktrees/invoking-lane");
    fs::create_dir_all(&linked_git).unwrap();
    stage_fixture_app(&appliance, APPLIANCE_IDENTITY, "appliance app");

    let linked = scratch.child("invoking-lane");
    fs::create_dir(&linked).unwrap();
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", linked_git.display()),
    )
    .unwrap();
    fs::write(linked_git.join("commondir"), "../..\n").unwrap();
    stage_fixture_app(&linked, LINKED_IDENTITY, "linked app");
    let nested_start = linked.join("nested/current-directory");
    fs::create_dir_all(&nested_start).unwrap();

    let demo = scratch.child("demo");
    seed_environment_from(Some(&demo), Path::new("/opt/spice"), &nested_start)
        .expect("linked-worktree demo seed succeeds");

    let copied_stage = demo.join(serve::APP_STAGE);
    assert_eq!(
        fs::read_to_string(copied_stage.join(serve::APP_STAGE_CURRENT)).unwrap(),
        format!("{LINKED_IDENTITY}\n")
    );
    assert_eq!(
        fs::read_to_string(
            copied_stage
                .join(serve::APP_STAGE_GENERATIONS)
                .join(LINKED_IDENTITY)
                .join(wire::manifest::APP_MODULE),
        )
        .unwrap(),
        format!("linked app {}\n", wire::manifest::APP_MODULE)
    );
    assert!(
        !copied_stage
            .join(serve::APP_STAGE_GENERATIONS)
            .join(APPLIANCE_IDENTITY)
            .exists()
    );
}

#[test]
fn seed_is_deterministic_quoted_and_reopenable_without_touching_parent_state() {
    let scratch = Scratch::new("seed");
    let root = scratch.child("demo root's");
    let executable = Path::new("/opt/spice bin/spice's");
    let environment = seed_test_environment(Some(&root), executable).expect("demo seed succeeds");

    assert_eq!(environment.root, fs::canonicalize(&root).unwrap());
    assert_eq!(
        environment.transcript,
        environment.root.join(TRANSCRIPT_NAME)
    );
    assert!(environment.state.journal().is_file());
    assert_eq!(
        seeded_head(&environment.root).unwrap(),
        format!("{SEED_BRANCH} at {SEED_COMMIT}")
    );
    assert_eq!(
        fs::read(&environment.transcript).unwrap(),
        transcript_bytes().unwrap()
    );
    assert!(environment.launch_command.starts_with("cd '"));
    assert!(environment.launch_command.contains("'\"'\"'"));
    assert!(environment.launch_command.contains(" demo --reuse "));
    assert_eq!(environment.remove_command.matches("rm -rf --").count(), 1);

    let reader = journal::JournalReader::open(environment.state.journal()).unwrap();
    let facts = reader.fact_snapshot().unwrap();
    let kinds = facts
        .facts()
        .iter()
        .map(|envelope| envelope.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::RoomPublished)
            .count(),
        CANNED_TRANSCRIPT.len()
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::LaneSeated)
            .count(),
        DEMO_LANES.len()
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == wire::FactKind::LaneStarted)
            .count(),
        DEMO_LANES.len()
    );
    for seed in DEMO_LANES {
        assert!(facts.facts().iter().any(|envelope| {
            envelope
                .fact
                .read::<body::LaneStarted>()
                .is_some_and(|started| {
                    started.lane == uuid(seed.lane)
                        && started.occupant == uuid(seed.occupant)
                        && started.epoch == uuid(DEMO_EPOCH)
                })
        }));
    }
    let claim = facts
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<body::ClaimWon>())
        .expect("Ava's demo claim is retained");
    assert_eq!(claim.task, DEMO_STAMP.parse().expect("demo task stamp"));
    assert_eq!(claim.actor, Actor::Lane(uuid(DEMO_LANE)));
    assert_eq!(claim.lane, uuid(DEMO_LANE));
    assert_eq!(claim.occupant, uuid(DEMO_OCCUPANT));
    assert_eq!(claim.epoch, uuid(DEMO_EPOCH));

    let reopened = reopen_environment(&root, executable).expect("complete seed reopens");
    assert_eq!(reopened.root, environment.root);
    assert_eq!(reopened.launch_command, environment.launch_command);
}

fn admitted_demo_group_command(
    id: &str,
    kind: CommandKind,
    expected: Vec<wire::Witness>,
    body: &impl serde::Serialize,
) -> wire::AdmittedCommand {
    let epoch = DEMO_EPOCH.parse().expect("demo epoch is canonical");
    Command {
        v: WireVersion::CURRENT,
        id: id.parse().expect("fixture command identity is canonical"),
        epoch,
        kind,
        expected,
        body: serde_json::to_value(body).expect("fixture command body encodes"),
    }
    .admit(
        &wire::Accepted {
            wire: WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
            release: "1.2.3".parse().expect("fixture release"),
        },
        &wire::principal::PrincipalRoles::default(),
    )
    .expect("fixture command admits")
}

#[test]
fn seeded_oops_group_changes_through_shared_group_authority() {
    let scratch = Scratch::new("oops-group");
    let root = scratch.child("demo");
    let environment =
        seed_test_environment(Some(&root), Path::new("/opt/spice")).expect("demo seed succeeds");
    let journal = journal::Journal::open(environment.state.journal()).expect("seed journal opens");
    let bus = bus::Bus::start(journal).expect("seed journal bus starts");
    let partition = || {
        bus.read(|read| read.group_partition())
            .expect("seed partition reads")
            .expect("seed partition exists")
    };
    let seeded = partition();
    let group = wire::rooms::RoomAuthorityStamp::for_micros(DEMO_OOPS_GROUP_MICROS)
        .expect("demo OOPS group stamp is valid");
    assert_eq!(seeded.body().groups[0].group, group);
    let design = seeded.body().groups[0].anchors[0].clone();
    let revision = seeded.body().revision;
    let leave = wire::rooms::WorkingGroupMove {
        design: design.clone(),
        to: None,
    };
    let command = admitted_demo_group_command(
        "018f82d4-6f59-7d06-92e3-bce913116421",
        CommandKind::WorkingGroupMove,
        vec![
            wire::Witness::Group { revision },
            wire::Witness::Existence {
                task: design.clone(),
            },
        ],
        &leave,
    );
    let left = crate::board_command::dispatch(&bus, &command)
        .expect("working-group move dispatches through the command door");
    assert!(
        matches!(
            left,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                body: CommandAnswer::WorkingGroupChange(_),
            })
        ),
        "the seeded design must leave its group: {left:?}"
    );
    let ended = partition();
    assert!(ended.body().groups.is_empty());

    let rejoin = wire::rooms::WorkingGroupMove {
        design: design.clone(),
        to: Some(group.clone()),
    };
    let command = admitted_demo_group_command(
        "018f82d4-6f59-7d06-92e3-bce913116422",
        CommandKind::WorkingGroupMove,
        vec![
            wire::Witness::Group {
                revision: ended.body().revision,
            },
            wire::Witness::Existence {
                task: design.clone(),
            },
        ],
        &rejoin,
    );
    let rejoined = crate::board_command::dispatch(&bus, &command)
        .expect("working-group move dispatches through the command door");
    assert!(
        matches!(
            rejoined,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                body: CommandAnswer::WorkingGroupChange(_),
            })
        ),
        "the seeded design must begin a group again: {rejoined:?}"
    );
    assert_eq!(partition().body().groups[0].anchors, vec![design]);
}

#[test]
fn seeded_roots_are_born_on_one_commit_wherever_they_are_created() {
    let scratch = Scratch::new("born");
    let first = scratch.child("first root");
    let second = scratch.child("a second root under a much longer name");
    for root in [&first, &second] {
        seed_test_environment(Some(root), Path::new("/opt/spice"))
            .expect("born demo seed succeeds");
    }

    let born = "refs/heads/main at a53ad58b1356a5f7abea7437975321850e04b217";
    assert_eq!(
        seeded_head(&first).unwrap(),
        born,
        "the first seeded root is not born on the deterministic commit"
    );
    assert_eq!(
        seeded_head(&second).unwrap(),
        born,
        "a longer root path moved the seeded commit"
    );
    assert_eq!(
        format!("{SEED_BRANCH} at {SEED_COMMIT}"),
        born,
        "reuse requires descent from a commit the seed never writes"
    );
}

/// Advance a seeded root the way working lanes do: a commit on its branch, an
/// edited project policy, and a residency that seats one more lane.
fn advance_seeded_root(environment: &Environment) {
    let repository = gix::open(&environment.root).unwrap();
    let tree = repository.write_object(gix::objs::Tree::default()).unwrap();
    let identity = gix::actor::SignatureRef {
        name: "lane".into(),
        email: "lane@spice.invalid".into(),
        time: "1783746000 +0000",
    };
    let seed = gix::ObjectId::from_hex(SEED_COMMIT.as_bytes()).unwrap();
    repository
        .commit_as(identity, identity, SEED_BRANCH, "lane work\n", tree, [seed])
        .unwrap();
    fs::write(
        environment.root.join(wire::TRACKED_POLICY_FILE),
        b"[tasks]\nproject_max_depth = 4\n",
    )
    .unwrap();

    let reader = journal::JournalReader::open(environment.state.journal()).unwrap();
    let mut head = reader
        .read(|read| read.residency_head())
        .unwrap()
        .expect("the seed seats a resident")
        .body()
        .clone();
    drop(reader);
    let probe = uuid("018f82d4-6f59-7d06-92e3-bce913116418");
    let capability = uuid("018f82d4-6f59-7d06-92e3-bce913116419");
    let mut lane = head.lanes[0].clone();
    lane.lane = probe;
    lane.name = Some("Probe".to_owned());
    lane.capabilities = vec![capability];
    lane.occupant = None;
    lane.lineage.clear();
    head.topology_revision += 1;
    head.lanes.push(lane);

    let mut journal = journal::Journal::open(environment.state.journal()).unwrap();
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{DEMO_EPOCH}").parse().unwrap(),
        instant("2026-07-11T06:00:00.000000Z"),
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            body::LaneSeated {
                lane: probe,
                team: uuid(DEMO_LANE),
                capability,
                name: Some("Probe".to_owned()),
                worktree: environment.root.display().to_string(),
                life: "drive".to_owned(),
                vendor: crate::resident::SIMULATED_VENDOR.parse().unwrap(),
                model: None,
                effort: None,
            },
            FactCause::Root,
        )
        .unwrap();
    write.residency_head(seated, move |_| head);
    journal.apply(write).unwrap();
}

#[test]
fn reuse_admits_a_root_its_lanes_advanced_but_not_one_grown_from_another_seed() {
    let scratch = Scratch::new("advanced");
    let root = scratch.child("advanced-root");
    let executable = Path::new("/opt/spice");
    let environment =
        seed_test_environment(Some(&root), executable).expect("advanced-root seed succeeds");
    advance_seeded_root(&environment);
    let reopened = reopen_environment(&root, executable).expect("an advanced demo root reopens");
    assert_eq!(reopened.root, environment.root);

    let head = environment.root.join(".git/HEAD");
    let branch = environment.root.join(".git/refs/heads/main");
    let advanced = fs::read(&branch).unwrap();
    let refuses = |observed: &str| {
        let outcome = *reopen_environment(&root, executable).expect_err("a foreign head refuses");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("a foreign head must refuse: {outcome:?}");
        };
        assert_eq!(refusal.reason(), "demo::commit-moved");
        let diagnostic = refusal.diagnostic().expect("demo refusal is diagnostic");
        assert_eq!(diagnostic.observed(), Some(observed));
        assert_eq!(
            diagnostic.detail(),
            Some("check out a commit descending from the seeded commit or seed a new one")
        );
    };

    let repository = gix::open(&environment.root).unwrap();
    let tree = repository.write_object(gix::objs::Tree::default()).unwrap();
    let identity = gix::actor::SignatureRef {
        name: "other".into(),
        email: "other@spice.invalid".into(),
        time: "1783746000 +0000",
    };
    let foreign = repository
        .commit_as(
            identity,
            identity,
            "refs/heads/foreign",
            "another root\n",
            tree,
            gix::commit::NO_PARENT_IDS,
        )
        .unwrap();
    fs::write(&head, "ref: refs/heads/foreign\n").unwrap();
    refuses(&format!("refs/heads/foreign at {foreign}"));

    fs::write(&head, format!("ref: {SEED_BRANCH}\n")).unwrap();
    fs::write(&branch, "9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6\n").unwrap();
    refuses("refs/heads/main at 9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6");

    fs::write(&head, "ref: refs/heads/operator\n").unwrap();
    refuses("refs/heads/operator at no commit");

    fs::write(&head, format!("ref: {SEED_BRANCH}\n")).unwrap();
    fs::write(&branch, &advanced).unwrap();
    assert!(
        reopen_environment(&root, executable).is_ok(),
        "returning to the seed's descendant restores a reusable demo root"
    );
}

#[test]
fn reuse_refuses_roots_whose_demo_identity_does_not_match() {
    let scratch = Scratch::new("identity");
    let executable = Path::new("/opt/spice");
    let refusal_of = |root: &Path| {
        let outcome = *reopen_environment(root, executable).expect_err("a foreign root refuses");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("a foreign root must refuse: {outcome:?}");
        };
        assert!(
            refusal
                .diagnostic()
                .and_then(|diagnostic| diagnostic.detail())
                .is_some(),
            "{} names no repair",
            refusal.reason()
        );
        refusal.reason().to_owned()
    };

    let plain = scratch.child("plain repository");
    fs::create_dir(&plain).unwrap();
    born_repository(&plain).unwrap();
    assert_eq!(refusal_of(&plain), "demo::seed-unrecognized");

    let transcript = scratch.child("edited transcript");
    let environment = seed_test_environment(Some(&transcript), executable).unwrap();
    fs::write(&environment.transcript, b"{}\n").unwrap();
    assert_eq!(refusal_of(&transcript), "demo::transcript-moved");

    let reseeded = scratch.child("foreign journal");
    let environment = seed_test_environment(Some(&reseeded), executable).unwrap();
    let journal = environment.state.journal();
    for suffix in ["", "-wal", "-shm"] {
        let mut path = journal.as_os_str().to_owned();
        path.push(suffix);
        let _ = fs::remove_file(PathBuf::from(path));
    }
    drop(journal::Journal::open(journal).unwrap());
    assert_eq!(refusal_of(&reseeded), "demo::seed-moved");
}

#[test]
fn new_seed_refuses_nonempty_and_non_directory_roots_without_mutation() {
    let scratch = Scratch::new("root-refusal");
    let nonempty = scratch.child("operator repository");
    fs::create_dir(&nonempty).unwrap();
    let sentinel = nonempty.join("sentinel.txt");
    fs::write(&sentinel, b"operator bytes\n").unwrap();
    let file = scratch.child("operator-file");
    fs::write(&file, b"also operator bytes\n").unwrap();

    for root in [&nonempty, &file] {
        let outcome = *seed_test_environment(Some(root), Path::new("/opt/spice"))
            .expect_err("protected root refuses");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("protected root must refuse: {outcome:?}");
        };
        assert!(matches!(
            refusal.reason(),
            "demo::root-not-empty" | "demo::root-refused"
        ));
    }
    assert_eq!(fs::read(&sentinel).unwrap(), b"operator bytes\n");
    assert_eq!(fs::read(&file).unwrap(), b"also operator bytes\n");
}

#[cfg(unix)]
#[test]
fn non_utf8_executable_refuses_before_the_demo_root_exists() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let scratch = Scratch::new("executable-refusal");
    let root = scratch.child("uncreated-root");
    let executable = PathBuf::from(OsString::from_vec(b"/opt/spice-\xff".to_vec()));
    let outcome = *seed_test_environment(Some(&root), &executable)
        .expect_err("non-reproducible executable path refuses");

    let Outcome::Refusal(refusal) = outcome else {
        panic!("non-UTF-8 executable path must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "demo::path-refused");
    assert!(!root.exists(), "path refusal must precede seed mutation");
}

#[test]
fn native_app_stream_begins_live_and_reads_seeded_lane_task() {
    let scratch = Scratch::new("browser");
    let root = scratch.child("browser-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("browser demo seed succeeds");
    let root_for_down = environment.root.clone();
    let discovery_path = environment.state.serve().to_path_buf();
    let outcome = app_environment_with(environment, false, false, true, false, |endpoint| {
        let discovery = read_discovery(&discovery_path);
        assert_eq!(endpoint, discovery.endpoint);
        assert_browser_seed(&discovery);
        let Outcome::Reply(stopped) = crate::resident::down_from_operator(
            &root_for_down,
            crate::cli::contract(&["down"]).expect("down contract"),
        ) else {
            panic!("the demo resident stops on request");
        };
        assert_eq!(
            (stopped.body.from, stopped.body.to),
            (ResidentState::Seated, ResidentState::Dark),
            "down answers with the transition it made"
        );
        Ok(())
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("served browser demo replies: {outcome:?}");
    };
    assert_eq!(answer.body.mode, "app");
    assert_eq!(answer.body.lifecycle, "preserved");
    assert!(
        answer
            .body
            .endpoint
            .as_deref()
            .is_some_and(|value| value.starts_with("http://127.0.0.1:"))
    );
    assert!(root.exists());
}

/// Seed one demo root these gate cases can run to completion.
///
/// Both interrupt after launch rather than stopping the resident from inside
/// the launcher, which is how every other case here ends its run. That pattern
/// is unavailable to the case that matters: a plain run never calls the
/// launcher at all, so a launcher that shuts the resident down would hang
/// exactly when it passes. The injected signal is sent on both paths, so the
/// two cases below differ only in the flag under test.
fn gate_environment(name: &str) -> (Scratch, Environment) {
    let scratch = Scratch::new(name);
    let root = scratch.child("demo-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("gate demo seed succeeds");
    (scratch, environment)
}

/// Drive the generated parser all the way through `run_with`, pinning the
/// adjacent `signals` and `open` arguments to their intended meanings. The
/// test-only interruption ends each resident after the launch gate, independent
/// of whether the launcher runs; swapping those arguments therefore makes the
/// `--open` assertion fail promptly instead of leaving the plain case resident.
/// The two cases own separate roots and residents, so they run side by side.
#[test]
fn parsed_demo_open_flag_reaches_the_app_gate() {
    let control = RunControl {
        signals: false,
        interrupt_after_launch: true,
    };

    std::thread::scope(|scope| {
        let plain = scope.spawn(|| {
            let (_scratch, environment) = gate_environment("plain-no-open");
            let root = environment.root.to_str().unwrap();
            let plain = crate::cli::command()
                .try_get_matches_from(["spice", "demo", "--reuse", root])
                .expect("the plain demo invocation parses");
            let outcome = run_with(crate::cli::selected_matches(&plain), control, |endpoint| {
                panic!("a plain demo run must not open a browser: {endpoint}")
            });
            let Outcome::Reply(answer) = outcome else {
                panic!("a plain demo run replies: {outcome:?}");
            };
            assert_eq!(answer.body.mode, "app");
            assert!(
                answer
                    .body
                    .endpoint
                    .as_deref()
                    .is_some_and(|value| value.starts_with("http://127.0.0.1:")),
                "the endpoint is reported even though nothing was opened: {:?}",
                answer.body.endpoint
            );
        });

        let open = scope.spawn(|| {
            let (_scratch, environment) = gate_environment("explicit-open");
            let root = environment.root.to_str().unwrap();
            let open = crate::cli::command()
                .try_get_matches_from(["spice", "demo", "--reuse", root, "--open"])
                .expect("the opening demo invocation parses");
            let mut dialled = None;
            let outcome = run_with(crate::cli::selected_matches(&open), control, |endpoint| {
                dialled = Some(endpoint.to_owned());
                Ok(())
            });
            let Outcome::Reply(answer) = outcome else {
                panic!("an opening demo run replies: {outcome:?}");
            };
            assert!(dialled.is_some(), "--open reaches the launcher");
            assert_eq!(dialled.as_deref(), answer.body.endpoint.as_deref());
        });

        for case in [plain, open] {
            if let Err(panic) = case.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}

/// The gate above is only reachable if the flag exists and defaults off. This
/// asks the generated parser rather than the contract string, because the
/// string is what registers the flag and a test that re-read it would pass on
/// a `demo` that never grew one.
#[test]
fn demo_registers_open_and_defaults_it_off() {
    let parse = |arguments: &[&str]| {
        let parsed = crate::cli::command()
            .try_get_matches_from(arguments)
            .expect("the demo invocation parses");
        crate::cli::selected_matches(&parsed).get_flag("open")
    };
    assert!(
        !parse(&["spice", "demo"]),
        "a plain demo run is the one an agent makes without thinking about it"
    );
    assert!(parse(&["spice", "demo", "--open"]));
    assert!(
        crate::cli::command()
            .try_get_matches_from(["spice", "demo", "--no-open"])
            .is_err(),
        "opting out of a launching default is the shape this task refused"
    );
}

#[test]
fn launch_failure_preserves_one_reusable_or_removable_recovery_root() {
    let scratch = Scratch::new("launch-failure");
    let root = scratch.child("failed-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("failure demo seed succeeds");
    let outcome = app_environment_with(environment, true, false, true, false, |_endpoint| {
        Err("fixture browser launcher failed".to_owned())
    });
    let Outcome::Fault(fault) = outcome else {
        panic!("launcher failure is typed: {outcome:?}");
    };
    // Printing the whole fault, not just the reason, is what makes a failure
    // here diagnosable. Every step before the launcher can refuse too, and
    // under load one of them has; a bare left/right on the reason threw away
    // the detail naming which one.
    assert_eq!(
        fault.reason, "serve::launch-failed",
        "a step before the launcher refused instead, and this is the whole fault: {fault:?}",
    );
    assert!(fault.detail.contains("fixture browser launcher failed"));
    assert!(root.exists(), "failure never removes recovery state");
    assert!(reopen_environment(&root, Path::new("/opt/spice")).is_ok());
}

#[test]
fn interruption_preserves_even_an_automatic_cleanup_root() {
    let scratch = Scratch::new("interruption");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("automatic demo seed succeeds");
    let outcome = app_environment_with(environment, true, false, false, true, |_endpoint| Ok(()));
    let Outcome::Reply(answer) = outcome else {
        panic!("interrupted demo replies with recovery: {outcome:?}");
    };
    assert_eq!(answer.body.lifecycle, "interrupted-preserved");
    assert!(root.exists());
    assert!(answer.body.launch_command.contains("--reuse"));
    assert!(answer.body.remove_command.contains("rm -rf --"));
}

#[test]
fn clean_automatic_service_removes_only_its_demo_root() {
    let scratch = Scratch::new("cleanup");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).unwrap();
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("automatic cleanup seed succeeds");
    let root_for_down = fs::canonicalize(&root).unwrap();
    let outcome = app_environment_with(environment, true, false, true, false, |_endpoint| {
        let Outcome::Reply(stopped) = crate::resident::down_from_operator(
            &root_for_down,
            crate::cli::contract(&["down"]).expect("down contract"),
        ) else {
            panic!("the demo resident stops on request");
        };
        assert_eq!(
            (stopped.body.from, stopped.body.to),
            (ResidentState::Seated, ResidentState::Dark),
            "down answers with the transition it made"
        );
        Ok(())
    });
    let Outcome::Reply(answer) = outcome else {
        panic!("clean demo service replies: {outcome:?}");
    };
    assert_eq!(answer.body.lifecycle, "removed");
    assert!(!root.exists());
    assert!(scratch.0.exists(), "cleanup stays scoped to the demo root");
}

#[test]
fn disappearing_known_publication_files_are_a_benign_readiness_change() {
    let scratch = Scratch::new("transient-journal");
    let root = scratch.child("automatic-root");
    fs::create_dir(&root).expect("the automatic demo root is created");
    let environment = seed_test_prepared_root(&root, Path::new("/opt/spice"), true)
        .expect("the automatic demo root is seeded");
    assert!(environment.automatic);
    let journal = environment.state.journal();
    let sidecar = journal.with_extension("sqlite3-journal");
    fs::write(&sidecar, []).expect("the journal sidecar is discovered");
    let error =
        notify::Error::io(io::Error::from(io::ErrorKind::NotFound)).add_path(sidecar.clone());

    let destination = atomic::Destination::new(root.join("published")).expect("atomic destination");
    let staged = destination.stage().expect("atomic publication is staged");
    let temporary = staged.temporary_path().to_path_buf();

    fs::remove_file(&sidecar).expect("the journal sidecar disappears before inspection");
    drop(staged);

    assert_eq!(
        readiness_change(Err(error.add_path(temporary)), journal),
        Ok(())
    );
}

#[test]
fn persistent_and_unrelated_readiness_scan_failures_are_preserved() {
    let scratch = Scratch::new("readiness-errors");
    let journal = scratch.child("spicejournal.sqlite3");
    let sidecar = journal.with_extension("sqlite3-journal");
    fs::write(&sidecar, []).expect("the persistent journal sidecar exists");

    let persistent = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound)).add_path(sidecar.clone())),
        &journal,
    )
    .expect_err("a persistent path is not dismissed as a disappearance race");
    assert!(persistent.contains(&sidecar.display().to_string()));

    let destination = atomic::Destination::new(scratch.child("published"))
        .expect("persistent atomic destination");
    let staged = destination.stage().expect("persistent atomic stage");
    let temporary = staged.temporary_path().to_path_buf();
    let persistent_atomic =
        readiness_change(
            Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
                .add_path(temporary.clone())),
            &journal,
        )
        .expect_err("a persistent atomic path is not dismissed as a disappearance race");
    assert!(persistent_atomic.contains(&temporary.display().to_string()));
    drop(staged);

    fs::remove_file(&sidecar).expect("the journal sidecar is absent for the I/O fixture");
    let unrelated_io = readiness_change(
        Err(
            notify::Error::io(io::Error::from(io::ErrorKind::PermissionDenied))
                .add_path(sidecar.clone()),
        ),
        &journal,
    )
    .expect_err("an unrelated I/O error remains a readiness failure");
    assert!(unrelated_io.contains(&sidecar.display().to_string()));

    let unrelated_path = scratch.child("unrelated.tmp");
    let unrelated_missing = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
            .add_path(unrelated_path.clone())),
        &journal,
    )
    .expect_err("an unrelated missing path remains a readiness failure");
    assert!(unrelated_missing.contains(&unrelated_path.display().to_string()));

    let malformed_atomic = scratch.child(".spice-atomic-01-2.tmp");
    let malformed_missing = readiness_change(
        Err(notify::Error::io(io::Error::from(io::ErrorKind::NotFound))
            .add_path(malformed_atomic.clone())),
        &journal,
    )
    .expect_err("a malformed atomic lookalike remains a readiness failure");
    assert!(malformed_missing.contains(&malformed_atomic.display().to_string()));
}

/// A readiness scan that fails refuses under its own name.
///
/// Creating the watcher, registering the watch, and a scan error raised while
/// waiting each refuse under a distinct reason, so a failure under load names
/// which of the three happened.
///
/// This forces the third, which is also the one that actually fires: a
/// `PollWatcher` does *not* reject a missing directory at registration, it
/// accepts the watch and then reports the failure from inside the scan. So an
/// unwatchable root arrives as `demo::readiness-scan-failed`, and the two
/// pre-resident reasons stay reserved for genuine watcher-construction and
/// registration faults rather than absorbing this one.
///
/// The scan is raised while waiting for discovery, which is before the browser
/// is reached, so this also pins that a readiness refusal never launches.
#[test]
fn an_unwatchable_root_refuses_by_its_own_reason_before_the_browser_is_reached() {
    let scratch = Scratch::new("unwatchable");
    let root = scratch.child("missing-root");
    let environment = seed_test_environment(Some(&root), Path::new("/opt/spice"))
        .expect("unwatchable demo seed succeeds");
    // Removed only after seeding, so the environment is well formed and the
    // single thing wrong is that the directory to watch is gone.
    fs::remove_dir_all(&root).expect("the watched root is removed before the watch");

    let mut launched = false;
    let outcome = app_environment_with(environment, false, false, true, false, |_endpoint| {
        launched = true;
        Ok(())
    });
    let Outcome::Fault(fault) = outcome else {
        panic!("an unwatchable root is typed: {outcome:?}");
    };
    assert_eq!(
        fault.reason, "demo::readiness-scan-failed",
        "a failed scan is named apart from watcher creation and watch registration: {fault:?}",
    );
    assert!(
        fault.detail.contains(&root.display().to_string()),
        "the detail names the path that could not be watched: {}",
        fault.detail,
    );
    assert!(
        !launched,
        "a watch refused before the resident starts never reaches the browser",
    );
}

fn read_discovery(path: &Path) -> serve::Discovery {
    wire::decode(&fs::read(path).expect("demo discovery reads")).expect("demo discovery decodes")
}

fn assert_browser_seed(discovery: &serve::Discovery) {
    let (mut stream, accepted) = authenticate(discovery);
    let request = StreamRequest::Subscribe {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce913116420"),
        epoch: discovery.epoch,
        subjects: vec![ViewSubject::Facts],
        after: None,
    };
    send(&mut stream, &request);
    let StreamFrame::Facts { packed, .. } = receive(&mut stream) else {
        panic!("browser stream begins with seeded facts");
    };
    assert!(
        packed.rows.iter().all(|fact| {
            wire::FactKind::LANE_OCCUPANCY.contains(&fact.kind()) && fact.seq() <= packed.horizon
        }),
        "a fresh operator starts at the current live barrier, replaying only recent lane occupancy"
    );
    assert!(packed.overflow.is_none());
    stream.close(None).unwrap();

    let board = issue(
        discovery,
        accepted.epoch,
        CommandKind::BoardRead,
        &BoardRead::All { family: None },
    );
    let CommandAnswer::Board(board) = board else {
        panic!("demo board command returns a board");
    };
    assert_board(&board);
    let lanes = issue(
        discovery,
        accepted.epoch,
        CommandKind::LaneRead,
        &LaneRead {},
    );
    let CommandAnswer::Lane(lanes) = lanes else {
        panic!("demo lane command returns lanes");
    };
    assert_lanes(&lanes);
}

fn assert_board(board: &BoardAnswer) {
    let task = board
        .tasks
        .iter()
        .find(|task| task.statement.body.handle.as_str() == DEMO_TASK)
        .expect("demo task is visible");
    assert_eq!(task.presentation_title(), DEMO_TITLE);
    assert_eq!(task.statement.body.project, DEMO_PROJECT);
    assert_eq!(task.creator_team, None);
    let claim = task.claim.as_ref().expect("Ava holds the demo task");
    assert_eq!(claim.body.actor, Actor::Lane(uuid(DEMO_LANE)));
    assert_eq!(claim.body.lane, uuid(DEMO_LANE));
    assert_eq!(claim.body.occupant, uuid(DEMO_OCCUPANT));
    assert_eq!(claim.body.disposition, wire::ClaimDisposition::Active);
    assert!(task.lease.is_some(), "Ava's active claim carries its lease");
}

fn assert_lanes(lanes: &LaneAnswer) {
    assert_eq!(lanes.lanes.len(), DEMO_LANES.len());
    for seed in DEMO_LANES {
        let lane = lanes
            .lanes
            .iter()
            .find(|lane| lane.lane == uuid(seed.lane))
            .expect("each simulated demo lane is visible");
        assert_eq!(lane.team, uuid(DEMO_LANE));
        assert_eq!(lane.vendor.as_str(), crate::resident::SIMULATED_VENDOR);
        assert_eq!(lane.disposition, crate::resident::SIMULATED_DISPOSITION);
        assert_eq!(lane.occupant, Some(uuid(seed.occupant)));
        assert_eq!(lane.liveness, wire::intent::LaneLiveness::Working);
        assert_eq!(lane.worktree, lanes.lanes[0].worktree);
    }
}

fn issue<B: Serialize>(
    discovery: &serve::Discovery,
    epoch: CanonicalUuid,
    kind: CommandKind,
    body: &B,
) -> CommandAnswer {
    let (mut socket, _) = authenticate(discovery);
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce913116421"),
        epoch,
        kind,
        expected: Vec::new(),
        body,
    };
    send(&mut socket, &command);
    let response: Response<serde_json::Value> = receive(&mut socket);
    socket.close(None).unwrap();
    let Outcome::Reply(reply) = response.outcome else {
        panic!("demo browser command replies: {:?}", response.outcome);
    };
    kind.binding()
        .answer
        .decode(&wire::encode(&reply.body).expect("reply value encodes"))
        .expect("reply body matches its command binding")
}

fn authenticate(discovery: &serve::Discovery) -> (WebSocket<TcpStream>, wire::Accepted) {
    let authority = discovery.endpoint.strip_prefix("http://").unwrap();
    let address: SocketAddr = authority.parse().unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut request = format!("ws://{authority}/socket")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(ORIGIN, discovery.endpoint.parse().unwrap());
    let (mut socket, _) = tungstenite::client(request, stream).unwrap();
    send(
        &mut socket,
        &wire::Hello {
            wire: WireVersion::CURRENT,
            capability: discovery.capability,
            epoch: discovery.epoch,
        },
    );
    let accepted = receive(&mut socket);
    (socket, accepted)
}

fn send<T: Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) {
    socket
        .send(Message::Binary(wire::encode(value).unwrap().into()))
        .unwrap();
}

fn receive<T: serde::de::DeserializeOwned>(socket: &mut WebSocket<TcpStream>) -> T {
    wire::decode(&socket.read().unwrap().into_data()).unwrap()
}

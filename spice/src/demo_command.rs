//! Zero-setup native canvas demonstration environment.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant as Monotonic};

use clap::ArgMatches;
use journal::{FactCause, JournalWrite};
use notify::{RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};
use wire::{
    Actor, CanonicalUuid, DemoAnswer, Fault, FaultClass, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidencyHead, ResidentState, TaskHead, WorkflowHead, body,
};

mod recovery;

use recovery::{preserve_environment_failure, preserve_failure, require_utf8, shell_quote};

const MANIFEST_NAME: &str = ".spice-demo.json";
const TRANSCRIPT_NAME: &str = "spice-demo-transcript.jsonl";
const CONFIG_NAME: &str = "spice.toml";
const CONFIG_BYTES: &[u8] = b"[tasks]\nstems = [\"rim\"]\n";
const DEMO_TASK: &str = "DEMO-preview";
const DEMO_TITLE: &str = "Add machine-readable status output";
const DEMO_PROJECT: &str = "rim.canvas.demo";
const DEMO_UUID: &str = "018f82d4-6f59-7d06-92e3-bce913116410";
const DEMO_LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116411";
const DEMO_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce913116412";
const DEMO_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116413";
const SEED_BRANCH: &str = "refs/heads/main";
const SEED_NAME: &str = "spice demo";
const SEED_EMAIL: &str = "demo@spice.invalid";
const SEED_MESSAGE: &str = "spice demo seed\n";
const SEED_COMMIT: &str = "a53ad58b1356a5f7abea7437975321850e04b217";
const WATCH_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const READY_DEADLINE: Duration = Duration::from_secs(10);
const STOP_DEADLINE: Duration = Duration::from_secs(10);

type DemoResult<T> = Result<T, Box<Outcome<DemoAnswer>>>;

const CANNED_TRANSCRIPT: [(&str, &str); 4] = [
    (
        "2026-07-11T05:00:00.000000Z",
        concat!(
            "Picking up the task to add a `--porcelain` flag to `spice status`. ",
            "Reading the status command to find where the human-readable table is ",
            "assembled, so a machine-readable path can branch from the same data."
        ),
    ),
    (
        "2026-07-11T05:01:00.000000Z",
        concat!(
            "Found it: `render_status` builds the table from a single `StatusReport`. ",
            "I'll serialize that same report to newline-delimited JSON behind ",
            "`--porcelain`, so the table and the JSON always read from one source and ",
            "cannot drift."
        ),
    ),
    (
        "2026-07-11T05:02:00.000000Z",
        concat!(
            "Implemented and covered: `spice status --porcelain` now prints one JSON ",
            "object per lane, and `test_status_porcelain_matches_table` asserts the ",
            "JSON rows carry the same lane ids the table renders. Full suite is green ",
            "and the worktree is clean."
        ),
    ),
    (
        "2026-07-11T05:03:00.000000Z",
        concat!(
            "Done -- `spice status --porcelain` ships behind one flag, with a single ",
            "renderer feeding both views and a test pinning them together. Handing ",
            "the lane back."
        ),
    ),
];

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DemoManifest {
    schema: u32,
    task: String,
    lane: CanonicalUuid,
    transcript: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DemoTurn<'a> {
    timestamp: &'a str,
    role: &'a str,
    text: &'a str,
}

#[derive(Clone, Debug)]
struct Environment {
    root: PathBuf,
    transcript: PathBuf,
    state: spice::repository::StatePaths,
    launch_command: String,
    remove_command: String,
    automatic: bool,
}

enum Event {
    Changed(Result<(), String>),
    Resident(Outcome<()>),
    Signal(i32),
}

struct SignalRelay {
    handle: signal_hook::iterator::Handle,
    worker: Option<JoinHandle<()>>,
}

impl SignalRelay {
    fn install(events: Sender<Event>) -> io::Result<Self> {
        use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])?;
        let handle = signals.handle();
        let worker = std::thread::Builder::new()
            .name("spice-demo-signals".to_owned())
            .spawn(move || {
                for signal in signals.forever() {
                    if events.send(Event::Signal(signal)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }
}

impl Drop for SignalRelay {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Seed or run one isolated native canvas environment.
pub(crate) fn run(matches: &ArgMatches) -> Outcome<DemoAnswer> {
    run_with(matches, true, |path| {
        crate::command_client::open_canvas(path)
    })
}

fn run_with<F>(matches: &ArgMatches, signals: bool, launch: F) -> Outcome<DemoAnswer>
where
    F: FnOnce(&Path) -> Outcome<wire::CanvasOpen>,
{
    let root = matches.get_one::<String>("root").map(PathBuf::from);
    let reuse = matches.get_one::<String>("reuse").map(PathBuf::from);
    let seed_only = matches.get_flag("seed-only");
    let preserve = matches.get_flag("preserve");
    if root.is_some() && reuse.is_some() {
        return refusal(
            "demo::root-selection-refused",
            Some("choose either --root for a new seed or --reuse for an existing seed"),
            Some("both --root and --reuse were supplied"),
            "select one demo root mode",
        );
    }
    if seed_only && reuse.is_some() {
        return refusal(
            "demo::mode-refused",
            Some("--seed-only creates one new deterministic seed"),
            Some("--seed-only was combined with --reuse"),
            "remove --reuse or run the existing seed without --seed-only",
        );
    }

    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => return environment_fault("demo::executable-unavailable", error.to_string()),
    };
    let environment = match reuse {
        Some(root) => reopen_environment(&root, &executable),
        None => seed_environment(root.as_deref(), &executable),
    };
    let environment = match environment {
        Ok(environment) => environment,
        Err(failure) => return *failure,
    };
    if seed_only {
        return reply(&environment, "seeded", "preserved", None);
    }
    let cleanup = environment.automatic && !preserve;
    canvas_environment(environment, cleanup, signals, launch)
}

fn seed_environment(root: Option<&Path>, executable: &Path) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let (root, automatic) = prepare_root(root)?;
    let result = seed_prepared_root(&root, executable, automatic);
    result.map_err(|failure| preserve_failure(&root, executable, *failure).into())
}

fn seed_prepared_root(root: &Path, executable: &Path, automatic: bool) -> DemoResult<Environment> {
    born_repository(root)?;
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| {
            environment_fault(
                "demo::source-repository-unavailable",
                "the spice crate has no workspace parent".to_owned(),
            )
        })?;
    let source = canvas::AppSource::resolve(source_root)
        .map_err(|error| environment_fault("demo::app-unavailable", error.to_string()))?;
    finish_seed_prepared_root(root, executable, automatic, &source)
}

fn finish_seed_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
    source: &canvas::AppSource,
) -> DemoResult<Environment> {
    seed_lane_app(root, source)?;
    let repository = spice::repository::Repository::discover(root)
        .map_err(|error| environment_fault("demo::repository-unavailable", error.to_string()))?;
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    fs::write(root.join(CONFIG_NAME), CONFIG_BYTES)
        .map_err(|error| environment_fault("demo::config-unwritable", error.to_string()))?;
    fs::create_dir_all(state.data()).map_err(|error| {
        environment_fault(
            "demo::state-unavailable",
            format!("{}: {error}", state.data().display()),
        )
    })?;
    let transcript = root.join(TRANSCRIPT_NAME);
    fs::write(&transcript, transcript_bytes()?).map_err(|error| {
        environment_fault(
            "demo::transcript-unwritable",
            format!("{}: {error}", transcript.display()),
        )
    })?;
    seed_journal(&state, root)?;
    let manifest = DemoManifest {
        schema: 1,
        task: DEMO_TASK.to_owned(),
        lane: uuid(DEMO_LANE),
        transcript: TRANSCRIPT_NAME.to_owned(),
    };
    let mut manifest_bytes = wire::encode(&manifest)
        .map_err(|error| protocol_fault("demo::manifest-invalid", error.to_string()))?;
    manifest_bytes.push(b'\n');
    let manifest_path = root.join(MANIFEST_NAME);
    fs::write(&manifest_path, manifest_bytes).map_err(|error| {
        environment_fault(
            "demo::manifest-unwritable",
            format!("{}: {error}", manifest_path.display()),
        )
    })?;
    environment(root.to_path_buf(), transcript, state, executable, automatic)
}

/// Copy the binary's current immutable app generation into the demo lane.
fn seed_lane_app(root: &Path, source: &canvas::AppSource) -> DemoResult<()> {
    let generation = source
        .current_generation()
        .map_err(|error| environment_fault("demo::app-unavailable", error.to_string()))?;
    let stage = root.join(canvas::APP_STAGE);
    let target_generation = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(generation.identity().as_str());
    fs::create_dir_all(&target_generation).map_err(|error| {
        environment_fault(
            "demo::app-stage-unwritable",
            format!("{}: {error}", target_generation.display()),
        )
    })?;
    for asset in canvas::APP.assets() {
        let served = generation
            .served(*asset)
            .map_err(|error| environment_fault("demo::app-unavailable", error.to_string()))?;
        let target = target_generation.join(asset.file());
        fs::write(&target, served.into_bytes()).map_err(|error| {
            environment_fault(
                "demo::app-stage-unwritable",
                format!("{}: {error}", target.display()),
            )
        })?;
    }
    let pointer = stage.join(canvas::APP_STAGE_CURRENT);
    fs::write(&pointer, format!("{}\n", generation.identity())).map_err(|error| {
        environment_fault(
            "demo::app-stage-unwritable",
            format!("{}: {error}", pointer.display()),
        )
    })?;
    Ok(())
}

fn reopen_environment(root: &Path, executable: &Path) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let root = canonical_demo_root(root)?;
    let manifest_path = root.join(MANIFEST_NAME);
    let manifest_bytes = fs::read(&manifest_path).map_err(|error| {
        refusal(
            "demo::seed-unrecognized",
            Some("one complete spice demo manifest"),
            Some(&format!("{}: {error}", manifest_path.display())),
            "seed a new root with `spice demo --seed-only` or name its exact output root",
        )
    })?;
    let manifest: DemoManifest = wire::decode(&manifest_bytes).map_err(|error| {
        refusal(
            "demo::seed-unrecognized",
            Some("the current spice demo manifest schema"),
            Some(&error.to_string()),
            "remove this disposable root or seed a new one",
        )
    })?;
    if manifest != expected_manifest() {
        return Err(refusal(
            "demo::seed-unrecognized",
            Some("the exact deterministic spice demo identity"),
            Some(&format!("{manifest:?}")),
            "remove this disposable root or seed a new one",
        )
        .into());
    }
    let transcript = root.join(&manifest.transcript);
    let observed = fs::read(&transcript).map_err(|error| {
        environment_fault(
            "demo::transcript-unreadable",
            format!("{}: {error}", transcript.display()),
        )
    })?;
    let expected = transcript_bytes()?;
    if observed != expected {
        return Err(refusal(
            "demo::transcript-moved",
            Some("the deterministic canned transcript"),
            Some(&transcript.display().to_string()),
            "restore the seeded transcript or remove this disposable root",
        )
        .into());
    }
    if !matches!(fs::read(root.join(CONFIG_NAME)), Ok(bytes) if bytes == CONFIG_BYTES) {
        return Err(refusal(
            "demo::config-moved",
            Some("the deterministic demo project policy"),
            Some(&root.join(CONFIG_NAME).display().to_string()),
            "restore the seeded config or remove this disposable root",
        )
        .into());
    }
    let expected = expected_head();
    let head = seeded_head(&root)?;
    if head != expected {
        return Err(refusal(
            "demo::commit-moved",
            Some(&expected),
            Some(&head),
            "reset this demo root to its seeded commit or seed a new one",
        )
        .into());
    }
    let repository = spice::repository::Repository::discover(&root)
        .map_err(|error| environment_fault("demo::repository-unavailable", error.to_string()))?;
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    validate_seeded_journal(&state)?;
    environment(root, transcript, state, executable, false)
}

fn prepare_root(requested: Option<&Path>) -> DemoResult<(PathBuf, bool)> {
    match requested {
        Some(requested) => {
            match fs::symlink_metadata(requested) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(refusal(
                        "demo::root-refused",
                        Some("a new path or one real empty directory"),
                        Some(&requested.display().to_string()),
                        "choose a new or empty disposable directory",
                    )
                    .into());
                }
                Ok(_) => {
                    let mut entries = fs::read_dir(requested).map_err(|error| {
                        environment_fault(
                            "demo::root-unreadable",
                            format!("{}: {error}", requested.display()),
                        )
                    })?;
                    if entries
                        .next()
                        .transpose()
                        .map_err(|error| {
                            environment_fault("demo::root-unreadable", error.to_string())
                        })?
                        .is_some()
                    {
                        return Err(refusal(
                            "demo::root-not-empty",
                            Some("a new path or one empty disposable directory"),
                            Some(&requested.display().to_string()),
                            concat!(
                                "choose a new or empty disposable directory; existing ",
                                "contents were left unchanged"
                            ),
                        )
                        .into());
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let parent = requested.parent().unwrap_or_else(|| Path::new("."));
                    fs::canonicalize(parent).map_err(|error| {
                        environment_fault(
                            "demo::root-parent-unavailable",
                            format!("{}: {error}", parent.display()),
                        )
                    })?;
                    fs::create_dir(requested).map_err(|error| {
                        environment_fault(
                            "demo::root-unavailable",
                            format!("{}: {error}", requested.display()),
                        )
                    })?;
                }
                Err(error) => {
                    return Err(environment_fault(
                        "demo::root-unavailable",
                        format!("{}: {error}", requested.display()),
                    )
                    .into());
                }
            }
            let root = canonical_demo_root(requested)?;
            require_utf8(&root, "demo root")?;
            Ok((root, false))
        }
        None => {
            for _ in 0..16 {
                let root = std::env::temp_dir().join(format!(
                    "spice-demo-{}-{}",
                    std::process::id(),
                    uuid::Uuid::new_v4().simple()
                ));
                match fs::create_dir(&root) {
                    Ok(()) => {
                        let root = canonical_demo_root(&root)?;
                        require_utf8(&root, "demo root")?;
                        return Ok((root, true));
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(environment_fault(
                            "demo::root-unavailable",
                            format!("{}: {error}", root.display()),
                        )
                        .into());
                    }
                }
            }
            Err(environment_fault(
                "demo::root-unavailable",
                "sixteen exclusive temporary root attempts collided",
            )
            .into())
        }
    }
}

fn canonical_demo_root(root: &Path) -> DemoResult<PathBuf> {
    Ok(fs::canonicalize(root).map_err(|error| {
        environment_fault(
            "demo::root-unavailable",
            format!("{}: {error}", root.display()),
        )
    })?)
}

/// Create one demo repository already born on its seeded `main` commit.
///
/// Tree, identity, message, and time are all compiled in, so every disposable
/// root reaches the same commit id and reopening can hold a root to it. The
/// branch has to be compiled in the same way: a plain init resolves the initial
/// `HEAD` through the operator's own `init.defaultBranch`, which would park a
/// freshly seeded root on an unborn branch beside the seeded commit and make
/// reopening refuse a root nobody moved. Isolated options read no configuration
/// outside this root, so the seed describes itself and not the machine.
fn born_repository(root: &Path) -> DemoResult<()> {
    let repository = gix::ThreadSafeRepository::init_opts(
        root,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )
    .map_err(|error| {
        environment_fault(
            "demo::git-init-failed",
            format!("{}: {error}", root.display()),
        )
    })?
    .to_thread_local();
    let empty_tree = repository
        .write_object(gix::objs::Tree::default())
        .map_err(|error| environment_fault("demo::seed-commit-failed", error.to_string()))?;
    let stamp = format!(
        "{} +0000",
        instant(CANNED_TRANSCRIPT[0].0).get().as_second()
    );
    let identity = gix::actor::SignatureRef {
        name: SEED_NAME.into(),
        email: SEED_EMAIL.into(),
        time: &stamp,
    };
    repository
        .commit_as(
            identity,
            identity,
            SEED_BRANCH,
            SEED_MESSAGE,
            empty_tree,
            gix::commit::NO_PARENT_IDS,
        )
        .map_err(|error| environment_fault("demo::seed-commit-failed", error.to_string()))?;
    Ok(())
}

/// Name the branch and commit a demo root is parked on, however it got there.
///
/// A detached, unborn, or moved head all describe themselves in this one
/// spelling, so the seeded expectation is a single string to compare against.
fn seeded_head(root: &Path) -> DemoResult<String> {
    let repository = gix::open(root).map_err(|error| {
        environment_fault(
            "demo::repository-unavailable",
            format!("{}: {error}", root.display()),
        )
    })?;
    let head = repository
        .head()
        .map_err(|error| environment_fault("demo::repository-unavailable", error.to_string()))?;
    let branch = head.referent_name().map_or_else(
        || "detached HEAD".to_owned(),
        |name| name.as_bstr().to_string(),
    );
    let commit = head
        .id()
        .map_or_else(|| "no commit".to_owned(), |id| id.detach().to_string());
    Ok(format!("{branch} at {commit}"))
}

fn seed_journal(state: &spice::repository::StatePaths, root: &Path) -> DemoResult<()> {
    let mut journal = journal::Journal::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    let task: wire::TaskHandle = DEMO_TASK.parse().expect("demo task handle is valid");
    let previous = seed_task(&mut journal, &task)?;
    seed_turns(&mut journal, &task, previous)?;
    seed_lane(&mut journal, root)?;
    journal
        .verify()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    Ok(())
}

fn seed_task(journal: &mut journal::Journal, task: &wire::TaskHandle) -> DemoResult<wire::Seq> {
    let task_uuid = uuid(DEMO_UUID);
    let phase: wire::Phase = "todo".parse().expect("demo phase is valid");
    let origin: wire::Origin = format!("task:{DEMO_TASK}")
        .parse()
        .expect("demo task origin is valid");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(CANNED_TRANSCRIPT[0].0),
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                uuid: task_uuid,
                title: DEMO_TITLE.to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: DEMO_PROJECT.to_owned(),
                tags: vec!["demo".to_owned()],
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    let statement_origin = origin;
    let statement_task = task.clone();
    let workflow_task = task.clone();
    let workflow_phase = phase.clone();
    write.task_head(added, move |revision| TaskHead {
        handle: statement_task,
        uuid: task_uuid,
        title: DEMO_TITLE.to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: DEMO_PROJECT.to_owned(),
        tags: vec!["demo".to_owned()],
        dependencies: Vec::new(),
        provenance: statement_origin,
        revision,
    });
    write.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![workflow_phase.clone()],
        phase: workflow_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    Ok(journal.apply(write).map_err(commit_failure)?.last())
}

fn seed_turns(
    journal: &mut journal::Journal,
    task: &wire::TaskHandle,
    mut previous: wire::Seq,
) -> DemoResult<()> {
    for (timestamp, text) in CANNED_TRANSCRIPT {
        let mut write = JournalWrite::new(
            Actor::Spice,
            format!("task:{DEMO_TASK}")
                .parse()
                .expect("demo task origin is valid"),
            instant(timestamp),
            Producer::CommandDoor,
        );
        write
            .fact(
                body::TaskNoted {
                    task: task.clone(),
                    note: text.to_owned(),
                },
                FactCause::Existing(previous),
            )
            .map_err(build_failure)?;
        previous = journal.apply(write).map_err(commit_failure)?.last();
    }
    Ok(())
}

fn seed_lane(journal: &mut journal::Journal, root: &Path) -> DemoResult<()> {
    let epoch = uuid(DEMO_EPOCH);
    let lane = uuid(DEMO_LANE);
    let capability = uuid(DEMO_CAPABILITY);
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("demo resident origin is valid"),
        instant(CANNED_TRANSCRIPT[3].0),
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: Some("Ava".to_owned()),
                worktree: root.display().to_string(),
                life: "drive".to_owned(),
                vendor: "codex".parse().expect("demo vendor is valid"),
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    let worktree = root.display().to_string();
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Dark,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: Some("Ava".to_owned()),
            worktree,
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("demo vendor is valid"),
            occupant: None,
            lineage: Vec::new(),
            capabilities: vec![capability],
            session: None,
            disposition: "vacant".to_owned(),
        }],
    });
    journal.apply(write).map_err(commit_failure)?;
    Ok(())
}

fn validate_seeded_journal(state: &spice::repository::StatePaths) -> DemoResult<()> {
    let reader = journal::JournalReader::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    reader
        .verify()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let snapshot = reader
        .fact_snapshot()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let task = snapshot.facts().iter().any(|envelope| {
        envelope.fact.kind() == wire::FactKind::TaskAdded
            && envelope
                .fact
                .body()
                .get("task")
                .and_then(serde_json::Value::as_str)
                == Some(DEMO_TASK)
    });
    let lane = snapshot.facts().iter().any(|envelope| {
        envelope.fact.kind() == wire::FactKind::LaneSeated
            && envelope
                .fact
                .body()
                .get("lane")
                .and_then(serde_json::Value::as_str)
                == Some(DEMO_LANE)
    });
    let notes = snapshot
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TaskNoted)
        .count();
    if !task || !lane || notes < CANNED_TRANSCRIPT.len() {
        return Err(refusal(
            "demo::seed-moved",
            Some("the seeded task, lane, and four canned turns"),
            Some(&format!("task={task}, lane={lane}, turns={notes}")),
            "restore the seeded journal or remove this disposable root",
        )
        .into());
    }
    Ok(())
}

fn transcript_bytes() -> DemoResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for (timestamp, text) in CANNED_TRANSCRIPT {
        bytes.extend(
            wire::encode(&DemoTurn {
                timestamp,
                role: "assistant",
                text,
            })
            .map_err(|error| protocol_fault("demo::transcript-invalid", error.to_string()))?,
        );
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn expected_head() -> String {
    format!("{SEED_BRANCH} at {SEED_COMMIT}")
}

fn expected_manifest() -> DemoManifest {
    DemoManifest {
        schema: 1,
        task: DEMO_TASK.to_owned(),
        lane: uuid(DEMO_LANE),
        transcript: TRANSCRIPT_NAME.to_owned(),
    }
}

fn environment(
    root: PathBuf,
    transcript: PathBuf,
    state: spice::repository::StatePaths,
    executable: &Path,
    automatic: bool,
) -> DemoResult<Environment> {
    let root_text = require_utf8(&root, "demo root")?;
    let executable = require_utf8(executable, "current spice executable")?;
    let quoted_root = shell_quote(root_text);
    let quoted_executable = shell_quote(executable);
    Ok(Environment {
        launch_command: format!(
            "cd {quoted_root} && {quoted_executable} demo --reuse {quoted_root}"
        ),
        remove_command: format!("rm -rf -- {quoted_root}"),
        root,
        transcript,
        state,
        automatic,
    })
}

fn canvas_environment<F>(
    environment: Environment,
    cleanup: bool,
    signals: bool,
    launch: F,
) -> Outcome<DemoAnswer>
where
    F: FnOnce(&Path) -> Outcome<wire::CanvasOpen>,
{
    canvas_environment_with(environment, cleanup, signals, false, launch)
}

fn canvas_environment_with<F>(
    environment: Environment,
    cleanup: bool,
    signals: bool,
    interrupt_after_launch: bool,
    launch: F,
) -> Outcome<DemoAnswer>
where
    F: FnOnce(&Path) -> Outcome<wire::CanvasOpen>,
{
    let (events, receiver) = mpsc::channel();
    let _watcher = match watch_environment(&environment, events.clone()) {
        Ok(watcher) => watcher,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };
    let relay = match signal_relay(signals, events.clone()) {
        Ok(relay) => relay,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };
    let resident = match start_resident(&environment, events.clone()) {
        Ok(resident) => resident,
        Err(failure) => return preserve_environment_failure(&environment, *failure),
    };

    let ready = wait_for_discovery(&environment, &receiver);
    let interrupted_before_launch = match ready {
        Ok(false) => false,
        Ok(true) => true,
        Err(failure) => {
            drop(relay);
            drop(resident);
            return preserve_environment_failure(&environment, *failure);
        }
    };
    if interrupted_before_launch {
        let outcome = stop_resident(&environment, &receiver);
        drop(relay);
        return match outcome {
            Ok(()) => {
                let _ = resident.join();
                reply(&environment, "interrupted", "preserved", None)
            }
            Err(failure) => preserve_environment_failure(&environment, *failure),
        };
    }

    let opened = launch(environment.state.canvas());
    let endpoint = match opened {
        Outcome::Reply(answer) => answer.body.endpoint,
        Outcome::Refusal(refusal) => {
            let _ = stop_resident(&environment, &receiver);
            drop(relay);
            let _ = resident.join();
            return preserve_environment_failure(&environment, Outcome::Refusal(refusal));
        }
        Outcome::Fault(fault) => {
            let _ = stop_resident(&environment, &receiver);
            drop(relay);
            let _ = resident.join();
            return preserve_environment_failure(&environment, Outcome::Fault(fault));
        }
    };
    if interrupt_after_launch {
        let _ = events.send(Event::Signal(signal_hook::consts::signal::SIGINT));
    }
    let completion = wait_for_resident(&environment, &receiver);
    drop(relay);
    let _ = resident.join();
    finish_canvas(&environment, cleanup, endpoint, completion)
}

fn watch_environment(
    environment: &Environment,
    events: Sender<Event>,
) -> DemoResult<notify::PollWatcher> {
    let mut watcher = notify::PollWatcher::new(
        move |event: notify::Result<notify::Event>| {
            let observed = event.map(|_| ()).map_err(|error| error.to_string());
            let _ = events.send(Event::Changed(observed));
        },
        notify::Config::default().with_poll_interval(WATCH_SCAN_INTERVAL),
    )
    .map_err(|error| environment_fault("demo::readiness-unavailable", error.to_string()))?;
    watcher
        .watch(&environment.root, RecursiveMode::Recursive)
        .map_err(|error| environment_fault("demo::readiness-unavailable", error.to_string()))?;
    Ok(watcher)
}

fn signal_relay(signals: bool, events: Sender<Event>) -> DemoResult<Option<SignalRelay>> {
    if signals {
        SignalRelay::install(events).map(Some).map_err(|error| {
            environment_fault("demo::signals-unavailable", error.to_string()).into()
        })
    } else {
        Ok(None)
    }
}

fn start_resident(environment: &Environment, events: Sender<Event>) -> DemoResult<JoinHandle<()>> {
    let root = environment.root.clone();
    std::thread::Builder::new()
        .name("spice-demo-resident".to_owned())
        .spawn(move || {
            let outcome = crate::resident::up_from(
                &root,
                crate::resident::CanvasBinding::local(crate::resident::EPHEMERAL_LOOPBACK),
            );
            let _ = events.send(Event::Resident(outcome));
        })
        .map_err(|error| {
            environment_fault("demo::resident-launch-failed", error.to_string()).into()
        })
}

fn finish_canvas(
    environment: &Environment,
    cleanup: bool,
    endpoint: String,
    completion: DemoResult<bool>,
) -> Outcome<DemoAnswer> {
    match completion {
        Ok(true) => reply(
            environment,
            "canvas",
            "interrupted-preserved",
            Some(endpoint),
        ),
        Ok(false) if cleanup => match fs::remove_dir_all(&environment.root) {
            Ok(()) => reply(environment, "canvas", "removed", Some(endpoint)),
            Err(error) => preserve_environment_failure(
                environment,
                environment_fault(
                    "demo::cleanup-failed",
                    format!("{}: {error}", environment.root.display()),
                ),
            ),
        },
        Ok(false) => reply(environment, "canvas", "preserved", Some(endpoint)),
        Err(failure) => preserve_environment_failure(environment, *failure),
    }
}

fn wait_for_discovery(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<bool> {
    let deadline = Monotonic::now() + READY_DEADLINE;
    loop {
        if environment.state.canvas().is_file() {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Event::Changed(Ok(()))) => {}
            Ok(Event::Changed(Err(error))) => {
                return Err(environment_fault("demo::readiness-unavailable", error).into());
            }
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Ok(Event::Signal(_)) => return Ok(true),
            Err(RecvTimeoutError::Timeout) => {
                return Err(environment_fault(
                    "demo::resident-readiness-timeout",
                    format!("canvas discovery was not published within {READY_DEADLINE:?}"),
                )
                .into());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident readiness channel disconnected",
                )
                .into());
            }
        }
    }
}

fn wait_for_resident(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<bool> {
    loop {
        match receiver.recv() {
            Ok(Event::Changed(_)) => {}
            Ok(Event::Resident(Outcome::Reply(_))) => return Ok(false),
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Ok(Event::Signal(_signal)) => {
                stop_resident(environment, receiver)?;
                return Ok(true);
            }
            Err(_) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident lifecycle channel disconnected",
                )
                .into());
            }
        }
    }
}

fn stop_resident(environment: &Environment, receiver: &Receiver<Event>) -> DemoResult<()> {
    let contract = crate::cli::contract(&["down"]).expect("down contract is registered");
    match crate::resident::down_from(&environment.root, contract) {
        Outcome::Reply(_) => {}
        outcome => return Err(rebody(outcome).into()),
    }
    let deadline = Monotonic::now() + STOP_DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Event::Changed(_)) | Ok(Event::Signal(_)) => {}
            Ok(Event::Resident(Outcome::Reply(_))) => return Ok(()),
            Ok(Event::Resident(outcome)) => return Err(rebody(outcome).into()),
            Err(RecvTimeoutError::Timeout) => {
                return Err(environment_fault(
                    "demo::resident-stop-timeout",
                    format!("resident did not stop within {STOP_DEADLINE:?}"),
                )
                .into());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(environment_fault(
                    "demo::resident-events-closed",
                    "resident lifecycle channel disconnected",
                )
                .into());
            }
        }
    }
}

fn reply(
    environment: &Environment,
    mode: &str,
    lifecycle: &str,
    endpoint: Option<String>,
) -> Outcome<DemoAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: DemoAnswer {
            root: environment.root.display().to_string(),
            transcript: environment.transcript.display().to_string(),
            mode: mode.to_owned(),
            lifecycle: lifecycle.to_owned(),
            endpoint,
            launch_command: environment.launch_command.clone(),
            remove_command: environment.remove_command.clone(),
        },
    })
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("demo UUID constants are canonical")
}

fn instant(value: &str) -> wire::Instant {
    wire::Instant::new(value.parse().expect("demo timestamp constants parse"))
        .expect("demo timestamps fit the native instant range")
}

fn build_failure(error: journal::BuildError) -> Outcome<DemoAnswer> {
    protocol_fault("demo::journal-proposal-invalid", error.to_string())
}

fn commit_failure(error: journal::CommitError) -> Outcome<DemoAnswer> {
    environment_fault("demo::journal-write-failed", error.to_string())
}

fn rebody(outcome: Outcome<()>) -> Outcome<DemoAnswer> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "demo::resident-ended-before-ready".to_owned(),
            detail: "the demo resident ended before its lifecycle wait completed".to_owned(),
        }),
    }
}

fn refusal(
    reason: &str,
    expected: Option<&str>,
    observed: Option<&str>,
    remedy: &str,
) -> Outcome<DemoAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("demo-root".to_owned()),
        expected: expected.map(str::to_owned),
        observed: observed.map(str::to_owned),
        remedy: remedy.to_owned(),
    })
}

fn protocol_fault(reason: &str, detail: impl Into<String>) -> Outcome<DemoAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}

fn environment_fault(reason: &str, detail: impl Into<String>) -> Outcome<DemoAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}

#[cfg(test)]
#[path = "demo_command/tests.rs"]
mod tests;

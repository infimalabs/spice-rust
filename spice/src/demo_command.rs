//! Zero-setup native app demonstration environment.

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

use crate::control_signal::ControlRelay;

mod recovery;

use recovery::{preserve_environment_failure, preserve_failure, require_utf8, shell_quote};

const MANIFEST_NAME: &str = ".spice-demo.json";
const TRANSCRIPT_NAME: &str = "spice-demo-transcript.jsonl";
const CONFIG_BYTES: &[u8] = b"[tasks]\nproject_max_depth = 3\n";
const DEMO_TASK: &str = "DEMO-1kKRRsDP";
const DEMO_STAMP: &str = "1kKRRsDP";
const DEMO_TITLE: &str = "Add machine-readable status output";
const DEMO_PROJECT: &str = "rim.spice.demo";
const DEMO_LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116411";
const DEMO_OOPS_BATCH_STAMP: &str = "1kKRRsDQ";
const DEMO_OOPS_REPORT_STAMP: &str = "1kKRRsDR";
const DEMO_OOPS_GROUP_MICROS: i64 = 1_783_748_760_000_000;
const DEMO_OOPS_REPORT_TITLE: &str = "Observed command friction";
const DEMO_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce913116412";
const DEMO_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116413";
const DEMO_OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce913116414";
const DEMO_SECOND_LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116415";
const DEMO_SECOND_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce913116416";
const DEMO_SECOND_OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce913116417";
const DEMO_RESIDENT_AT: &str = "2026-07-11T04:59:00.000000Z";
const DEMO_CLAIM_EXPIRY: &str = "2099-07-11T05:00:00.000000Z";
const DEMO_CLAIM_MAXIMUM: &str = "2100-07-11T05:00:00.000000Z";
const SEED_BRANCH: &str = "refs/heads/main";
const SEED_NAME: &str = "spice demo";
const SEED_EMAIL: &str = "demo@spice.invalid";
const SEED_MESSAGE: &str = "spice demo seed\n";
const SEED_COMMIT: &str = "a53ad58b1356a5f7abea7437975321850e04b217";
const WATCH_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const READY_DEADLINE: Duration = Duration::from_secs(10);
const STOP_DEADLINE: Duration = Duration::from_secs(10);

type DemoResult<T> = Result<T, Box<Outcome<DemoAnswer>>>;

#[derive(Clone, Copy)]
struct DemoLane {
    lane: &'static str,
    capability: &'static str,
    occupant: &'static str,
    name: &'static str,
}

const DEMO_LANES: [DemoLane; 2] = [
    DemoLane {
        lane: DEMO_LANE,
        capability: DEMO_CAPABILITY,
        occupant: DEMO_OCCUPANT,
        name: "Ava",
    },
    DemoLane {
        lane: DEMO_SECOND_LANE,
        capability: DEMO_SECOND_CAPABILITY,
        occupant: DEMO_SECOND_OCCUPANT,
        name: "Milo",
    },
];

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
            "renderer feeding both views and a test pinning them together. Keeping ",
            "the task with Ava while it is reviewed."
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
    Signal,
}

#[derive(Clone, Copy)]
struct RunControl {
    signals: bool,
    interrupt_after_launch: bool,
}

/// Seed or run one isolated native app environment.
pub(crate) fn run(matches: &ArgMatches) -> Outcome<DemoAnswer> {
    run_with(
        matches,
        RunControl {
            signals: true,
            interrupt_after_launch: false,
        },
        crate::command_client::system_browser,
    )
}

fn run_with<F>(matches: &ArgMatches, control: RunControl, browser: F) -> Outcome<DemoAnswer>
where
    F: FnOnce(&str) -> Result<(), String>,
{
    let root = matches.get_one::<String>("root").map(PathBuf::from);
    let reuse = matches.get_one::<String>("reuse").map(PathBuf::from);
    let seed_only = matches.get_flag("seed-only");
    let preserve = matches.get_flag("preserve");
    let open = matches.get_flag("open");
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
    app_environment_with(
        environment,
        cleanup,
        control.signals,
        open,
        control.interrupt_after_launch,
        browser,
    )
}

fn seed_environment(root: Option<&Path>, executable: &Path) -> DemoResult<Environment> {
    let working = std::env::current_dir().map_err(|error| {
        environment_fault(
            "demo::source-repository-unavailable",
            format!("current directory: {error}"),
        )
    })?;
    seed_environment_from(root, executable, &working)
}

fn seed_environment_from(
    root: Option<&Path>,
    executable: &Path,
    source_start: &Path,
) -> DemoResult<Environment> {
    require_utf8(executable, "current spice executable")?;
    let (root, automatic) = prepare_root(root)?;
    let result = seed_prepared_root(&root, executable, automatic, source_start);
    result.map_err(|failure| preserve_failure(&root, executable, *failure).into())
}

fn seed_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
    source_start: &Path,
) -> DemoResult<Environment> {
    born_repository(root)?;
    let source_root = application_source_root(source_start)?;
    let source = serve::AppSource::resolve(&source_root)
        .map_err(|error| environment_fault("demo::app-unavailable", error.to_string()))?;
    finish_seed_prepared_root(root, executable, automatic, &source)
}

fn application_source_root(start: &Path) -> DemoResult<PathBuf> {
    spice::repository::Repository::discover(start)
        .map(|repository| repository.worktree_root().to_path_buf())
        .map_err(|error| {
            environment_fault("demo::source-repository-unavailable", error.to_string()).into()
        })
}

fn finish_seed_prepared_root(
    root: &Path,
    executable: &Path,
    automatic: bool,
    source: &serve::AppSource,
) -> DemoResult<Environment> {
    seed_lane_app(root, source)?;
    let repository = spice::repository::Repository::discover(root).map_err(|error| {
        environment_fault(spice::repository::ResolveError::REASON, error.to_string())
    })?;
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    fs::write(root.join(wire::TRACKED_POLICY_FILE), CONFIG_BYTES)
        .map_err(|error| environment_fault("demo::config-unwritable", error.to_string()))?;
    let skill = spice::install::installed_skill(&repository);
    fs::create_dir_all(skill.parent().expect("the installed skill has a directory"))
        .and_then(|()| fs::write(&skill, spice::install::skill_document()))
        .map_err(|error| {
            environment_fault(
                "demo::skill-unwritable",
                format!("{}: {error}", skill.display()),
            )
        })?;
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

/// Copy the invoking worktree's current immutable app generation into the demo lane.
fn seed_lane_app(root: &Path, source: &serve::AppSource) -> DemoResult<()> {
    let generation = source
        .current_generation()
        .map_err(|error| environment_fault("demo::app-unavailable", error.to_string()))?;
    let stage = root.join(serve::APP_STAGE);
    let target_generation = stage
        .join(serve::APP_STAGE_GENERATIONS)
        .join(generation.identity().as_str());
    fs::create_dir_all(&target_generation).map_err(|error| {
        environment_fault(
            "demo::app-stage-unwritable",
            format!("{}: {error}", target_generation.display()),
        )
    })?;
    for (file, served) in generation.files() {
        let target = target_generation.join(file.as_ref());
        fs::write(&target, served.bytes()).map_err(|error| {
            environment_fault(
                "demo::app-stage-unwritable",
                format!("{}: {error}", target.display()),
            )
        })?;
    }
    let pointer = stage.join(serve::APP_STAGE_CURRENT);
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
    // Lanes legitimately advance a demo root's branch, policy, residency, and
    // journal. Reopening proves only that this root was born from the seed:
    // its history still contains the seed commit and its journal still holds
    // the seeded facts, which append-only writes never remove.
    if !descends_from_seed(&root)? {
        return Err(refusal(
            "demo::commit-moved",
            Some(&format!("a HEAD commit descending from {SEED_COMMIT}")),
            Some(&seeded_head(&root)?),
            "check out a commit descending from the seeded commit or seed a new one",
        )
        .into());
    }
    let repository = spice::repository::Repository::discover(&root).map_err(|error| {
        environment_fault(spice::repository::ResolveError::REASON, error.to_string())
    })?;
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
/// spelling, which both the seed contract and reuse refusals report.
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

/// Whether the demo root's HEAD commit is the seed commit or descends from it.
///
/// An unborn head, or one whose history cannot be walked to the seed, did not
/// grow from this seed and is not reusable as a demo root.
fn descends_from_seed(root: &Path) -> DemoResult<bool> {
    let repository = gix::open(root).map_err(|error| {
        environment_fault(
            "demo::repository-unavailable",
            format!("{}: {error}", root.display()),
        )
    })?;
    let head = repository
        .head()
        .map_err(|error| environment_fault("demo::repository-unavailable", error.to_string()))?;
    let Some(tip) = head.id() else {
        return Ok(false);
    };
    let seed = gix::ObjectId::from_hex(SEED_COMMIT.as_bytes())
        .expect("the demo seed commit constant is a full object id");
    let Ok(walk) = repository.rev_walk([tip.detach()]).all() else {
        return Ok(false);
    };
    Ok(walk.map_while(Result::ok).any(|commit| commit.id == seed))
}

fn seed_journal(state: &spice::repository::StatePaths, root: &Path) -> DemoResult<()> {
    let mut journal = journal::Journal::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    crate::resident::admit_steering_index(&mut journal)
        .map_err(|outcome| environment_fault("demo::journal-invalid", format!("{outcome:?}")))?;
    seed_lanes(&mut journal, root)?;
    let task: wire::TaskHandle = DEMO_TASK.parse().expect("demo task handle is valid");
    let previous = seed_task(&mut journal, &task)?;
    let born = task.born().expect("demo task carries a canonical stamp");
    seed_claim(
        &mut journal,
        &born,
        &["todo"],
        DEMO_LANES[0],
        CANNED_TRANSCRIPT[0].0,
    )?;
    seed_turns(&mut journal, &task, previous)?;
    seed_oops_group(&mut journal)?;
    journal
        .verify()
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    Ok(())
}

fn seed_task(journal: &mut journal::Journal, task: &wire::TaskHandle) -> DemoResult<wire::Seq> {
    let born = task.born().expect("demo task carries a canonical stamp");
    let phase: wire::Phase = "todo".parse().expect("demo phase is valid");
    let origin: wire::Origin = format!("task:{DEMO_STAMP}")
        .parse()
        .expect("demo task origin is valid");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin,
        instant(CANNED_TRANSCRIPT[0].0),
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                preparation: None,
                intent: None,
                task: born.clone(),
                title: DEMO_TITLE.to_owned(),
                pitch: wire::pitch::derive(DEMO_TITLE, None),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: DEMO_PROJECT.to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                taste_proposal: None,
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    let statement_origin = wire::TaskProvenance::Task(born.clone());
    let statement_task = born.clone();
    let workflow_task = born.clone();
    let workflow_phase = phase.clone();
    write.task_head(added, move |revision| TaskHead {
        preparation: None,
        intent: None,
        born: statement_task,
        queue_origin: revision,
        title: DEMO_TITLE.to_owned(),
        pitch: wire::pitch::derive(DEMO_TITLE, None),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: DEMO_PROJECT.to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: statement_origin,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision,
    });
    write.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![workflow_phase.clone()],
        phase: workflow_phase,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    Ok(journal.apply(write).map_err(commit_failure)?.last())
}

/// Give a seeded lane the same canonical claim authority a real allocation
/// writes, so every consumer derives team ownership from production evidence.
fn seed_claim(
    journal: &mut journal::Journal,
    born: &wire::TaskStamp,
    flow: &[&str],
    lane: DemoLane,
    at: &str,
) -> DemoResult<()> {
    let flow = flow
        .iter()
        .map(|phase| phase.parse().expect("demo phase is valid"))
        .collect::<Vec<wire::Phase>>();
    let workflow = WorkflowHead {
        phase: flow[0].clone(),
        flow,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    };
    let actor = Actor::Lane(uuid(lane.lane));
    let at = instant(at);
    let mut write = JournalWrite::new(
        actor.clone(),
        format!("task:{born}")
            .parse()
            .expect("demo task origin is valid"),
        at,
        Producer::CommandDoor,
    );
    board::allocate::mint(
        &mut write,
        board::allocate::Mint {
            task: born,
            actor: &actor,
            workflow: &workflow,
            project: DEMO_PROJECT,
            lane: uuid(lane.lane),
            occupant: uuid(lane.occupant),
            epoch: uuid(DEMO_EPOCH),
            at,
            expiry: instant(DEMO_CLAIM_EXPIRY),
            maximum: instant(DEMO_CLAIM_MAXIMUM),
            git: None,
            recovery: None,
            source: body::SourceBoundary::Proven { heads: Vec::new() },
            cause: None,
        },
    )
    .map_err(|error| build_failure(error.into_build()))?;
    journal.apply(write).map_err(commit_failure)?;
    Ok(())
}

fn seed_oops_group(journal: &mut journal::Journal) -> DemoResult<()> {
    let batch: wire::TaskStamp = DEMO_OOPS_BATCH_STAMP
        .parse()
        .expect("demo OOPS batch stamp is valid");
    let report: wire::TaskStamp = DEMO_OOPS_REPORT_STAMP
        .parse()
        .expect("demo OOPS report stamp is valid");
    seed_oops_task(
        journal,
        batch.clone(),
        wire::OOPS_DESIGN_TITLE,
        wire::OOPS_DESIGN_PROJECT,
        Vec::new(),
        None,
        Some(wire::TaskIntent::OpportunisticDesign),
        &["design", "review"],
        "2026-07-11T05:04:00.000000Z",
    )?;
    seed_oops_task(
        journal,
        report.clone(),
        DEMO_OOPS_REPORT_TITLE,
        DEMO_PROJECT,
        Vec::new(),
        Some(wire::TaskPreparation {
            parent: batch.clone(),
            controller: batch.clone(),
            construction: report,
        }),
        Some(wire::TaskIntent::Plan),
        &["plan", "review"],
        "2026-07-11T05:05:00.000000Z",
    )?;
    let group = wire::rooms::RoomAuthorityStamp::for_micros(DEMO_OOPS_GROUP_MICROS)
        .expect("demo OOPS group stamp is valid");
    let mut write = JournalWrite::new(
        Actor::Spice,
        wire::rooms::working_group_origin(),
        instant("2026-07-11T05:06:00.000000Z"),
        Producer::CommandDoor,
    );
    write
        .fact(
            wire::rooms::WorkingGroupMoved {
                design: batch.clone(),
                from: None,
                to: Some(group.clone()),
                partition: wire::rooms::WorkingGroupPartition {
                    revision: 1,
                    groups: vec![wire::rooms::WorkingGroup {
                        group,
                        anchors: vec![batch.clone()],
                        stance: wire::rooms::WorkingGroupStance::Passive,
                    }],
                    fast: Vec::new(),
                },
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    journal.apply(write).map_err(commit_failure)?;
    // A second lane works the group's Design, so the group's rail has a lane.
    seed_claim(
        journal,
        &batch,
        &["design", "review"],
        DEMO_LANES[1],
        "2026-07-11T05:07:00.000000Z",
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one seeded task spells each statement field the demo journal records"
)]
fn seed_oops_task(
    journal: &mut journal::Journal,
    born: wire::TaskStamp,
    title: &str,
    project: &str,
    acceptance: Vec<String>,
    preparation: Option<wire::TaskPreparation>,
    intent: Option<wire::TaskIntent>,
    flow: &[&str],
    at: &str,
) -> DemoResult<()> {
    let phases = flow
        .iter()
        .map(|phase| phase.parse().expect("demo OOPS phase is valid"))
        .collect::<Vec<wire::Phase>>();
    let phase = phases[0].clone();
    let origin: wire::Origin = format!("task:{born}")
        .parse()
        .expect("demo OOPS task origin is valid");
    let dependencies = preparation
        .as_ref()
        .map(|preparation| vec![preparation.parent.clone()])
        .unwrap_or_default();
    let mut write = JournalWrite::new(Actor::Spice, origin, instant(at), Producer::CommandDoor);
    let added = write
        .fact(
            body::TaskAdded {
                preparation: preparation.clone(),
                intent,
                task: born.clone(),
                title: title.to_owned(),
                pitch: wire::pitch::derive(title, None),
                description: None,
                acceptance: acceptance.clone(),
                requirements: wire::EnvironmentRequirements::default(),
                project: project.to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                taste_proposal: None,
                depends: dependencies.clone(),
                flow: phases.clone(),
                phase: phase.clone(),
                revision: 1,
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
            },
            FactCause::Root,
        )
        .map_err(build_failure)?;
    let statement_task = born.clone();
    let statement_provenance = wire::TaskProvenance::Task(born.clone());
    let workflow_task = born;
    let statement_title = title.to_owned();
    let statement_pitch = wire::pitch::derive(title, None);
    let statement_project = project.to_owned();
    write.task_head(added, move |revision| TaskHead {
        preparation,
        intent,
        born: statement_task,
        queue_origin: revision,
        title: statement_title,
        pitch: statement_pitch,
        description: None,
        acceptance,
        requirements: wire::EnvironmentRequirements::default(),
        project: statement_project,
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies,
        provenance: statement_provenance,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision,
    });
    write.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: phases,
        phase,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    journal.apply(write).map_err(commit_failure)?;
    Ok(())
}

fn seed_turns(
    journal: &mut journal::Journal,
    task: &wire::TaskHandle,
    mut previous: wire::Seq,
) -> DemoResult<()> {
    let room = task.born().expect("demo task carries a canonical stamp");
    for (timestamp, text) in CANNED_TRANSCRIPT {
        let at = instant(timestamp);
        let mut write = JournalWrite::new(
            Actor::Spice,
            format!("task:{DEMO_STAMP}")
                .parse()
                .expect("demo task origin is valid"),
            at,
            Producer::CommandDoor,
        );
        write
            .fact(
                wire::rooms::RoomPublished {
                    room: room.clone(),
                    publication: wire::rooms::RoomAuthorityStamp::for_micros(
                        at.get().as_microsecond(),
                    )
                    .expect("each canned turn instant names a publication"),
                    content: wire::rooms::RoomPublicationContent::Authored {
                        text: text.to_owned(),
                    },
                },
                FactCause::Existing(previous),
            )
            .map_err(build_failure)?;
        previous = journal.apply(write).map_err(commit_failure)?.last();
    }
    Ok(())
}

fn seed_lanes(journal: &mut journal::Journal, root: &Path) -> DemoResult<()> {
    let epoch = uuid(DEMO_EPOCH);
    let team = uuid(DEMO_LANE);
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("demo resident origin is valid"),
        instant(DEMO_RESIDENT_AT),
        Producer::ResidentRecognition,
    );
    let mut cause = FactCause::Root;
    let mut lifecycle = None;
    for seed in DEMO_LANES {
        let seated = write
            .fact(
                body::LaneSeated {
                    lane: uuid(seed.lane),
                    team,
                    capability: uuid(seed.capability),
                    name: Some(seed.name.to_owned()),
                    worktree: root.display().to_string(),
                    life: "drive".to_owned(),
                    vendor: crate::resident::SIMULATED_VENDOR
                        .parse()
                        .expect("simulated demo vendor is valid"),
                    model: None,
                    effort: None,
                },
                cause,
            )
            .map_err(build_failure)?;
        let started = write
            .fact(
                body::LaneStarted {
                    lane: uuid(seed.lane),
                    occupant: uuid(seed.occupant),
                    epoch,
                    group: 0,
                    fresh_session: None,
                },
                FactCause::Pending(seated),
            )
            .map_err(build_failure)?;
        cause = FactCause::Pending(started);
        lifecycle = Some(started);
    }
    let worktree = root.display().to_string();
    write.residency_head(
        lifecycle.expect("the demo always seeds lane lifecycles"),
        move |_| ResidencyHead {
            epoch,
            state: ResidentState::Dark,
            topology_revision: 0,
            lanes: DEMO_LANES
                .into_iter()
                .map(|seed| wire::LaneResidency {
                    lane: uuid(seed.lane),
                    team,
                    name: Some(seed.name.to_owned()),
                    worktree: worktree.clone(),
                    life: "drive".to_owned(),
                    vendor: crate::resident::SIMULATED_VENDOR
                        .parse()
                        .expect("simulated demo vendor is valid"),
                    model: None,
                    effort: None,
                    fast_mode: false,
                    occupant: Some(uuid(seed.occupant)),
                    lineage: vec![uuid(seed.occupant)],
                    capabilities: vec![uuid(seed.capability)],
                    session: None,
                    disposition: crate::resident::SIMULATED_DISPOSITION.to_owned(),
                })
                .collect(),
            subscriptions: vec![wire::TeamSubscription {
                team,
                formed_at: Some(instant(DEMO_RESIDENT_AT)),
                policy: wire::intent::TeamLife::Drive,
                fast_mode: false,
            }],
            renewals: Vec::new(),
        },
    );
    journal.apply(write).map_err(commit_failure)?;
    Ok(())
}

fn validate_seeded_journal(state: &spice::repository::StatePaths) -> DemoResult<()> {
    let reader = journal::JournalReader::open(state.journal())
        .map_err(|error| environment_fault("demo::journal-unavailable", error.to_string()))?;
    let (_, snapshot) = reader
        .read_with(|read| read.verified_fact_snapshot())
        .map_err(|error| environment_fault("demo::journal-invalid", error.to_string()))?;
    let task = snapshot.facts().iter().any(|envelope| {
        envelope.fact.kind() == wire::FactKind::TaskAdded
            && envelope
                .fact
                .body()
                .get("task")
                .and_then(serde_json::Value::as_str)
                == Some(DEMO_STAMP)
    });
    let lanes = DEMO_LANES.iter().all(|seed| {
        snapshot.facts().iter().any(|envelope| {
            envelope.fact.kind() == wire::FactKind::LaneSeated
                && envelope
                    .fact
                    .body()
                    .get("lane")
                    .and_then(serde_json::Value::as_str)
                    == Some(seed.lane)
        })
    });
    let starts = DEMO_LANES.iter().all(|seed| {
        snapshot.facts().iter().any(|envelope| {
            envelope
                .fact
                .read::<body::LaneStarted>()
                .is_some_and(|started| {
                    started.lane == uuid(seed.lane)
                        && started.occupant == uuid(seed.occupant)
                        && started.epoch == uuid(DEMO_EPOCH)
                })
        })
    });
    let claimed = snapshot.facts().iter().any(|envelope| {
        envelope.fact.read::<body::ClaimWon>().is_some_and(|claim| {
            claim.task == DEMO_STAMP.parse().expect("demo task stamp is valid")
                && claim.actor == Actor::Lane(uuid(DEMO_LANE))
                && claim.lane == uuid(DEMO_LANE)
                && claim.occupant == uuid(DEMO_OCCUPANT)
                && claim.epoch == uuid(DEMO_EPOCH)
        })
    });
    let notes = snapshot
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::RoomPublished)
        .count();
    if !task || !lanes || !starts || !claimed || notes < CANNED_TRANSCRIPT.len() {
        return Err(refusal(
            "demo::seed-moved",
            Some("the seeded task, Ava's claim, two seated simulated lanes, and four canned turns"),
            Some(&format!(
                "task={task}, claimed={claimed}, lanes={lanes}, starts={starts}, turns={notes}"
            )),
            "restore the seeded journal or remove this disposable root",
        )
        .into());
    }
    Ok(())
}

mod runtime;
use runtime::*;

#[cfg(test)]
#[path = "demo_command/tests.rs"]
mod tests;

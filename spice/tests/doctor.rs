use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::config::Root;
use spice::placeholder::PlaceholderReason;
use wire::{
    ConfigSource, DoctorCommand, DoctorObservation, DoctorReport, DoctorSubsystem,
    DoctorSubsystemDiagnosis, DoctorToolSource, Outcome, ReplyPosition, SpiceVariable,
};

const SECRET: &str = "doctor-must-not-render-this-value";

struct Scratch(PathBuf);

impl Scratch {
    fn repository(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spice-doctor-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(path.join(".git/.spice-rs/data")).expect("create repository fixture");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn tracked(&self, text: &str) {
        fs::write(self.0.join("spice-rust.toml"), text).expect("write tracked configuration");
    }

    fn local(&self, text: &str) {
        fs::write(self.0.join(".git/.spice-rs/config.toml"), text)
            .expect("write local configuration");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove Doctor repository fixture");
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(
    repository: &Path,
    arguments: &[&str],
    search_path: &OsStr,
    environment: &[(SpiceVariable, &str)],
) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(arguments)
        .current_dir(repository)
        .env("PATH", search_path)
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1");
    for variable in SpiceVariable::ALL {
        command.env_remove(variable.as_str());
    }
    for (variable, value) in environment {
        command.env(variable.as_str(), value);
    }
    let output = command.output().expect("Doctor process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

fn reply(spoken: &Spoken) -> wire::Reply<DoctorReport<Root>> {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let decoded: Outcome<DoctorReport<Root>> =
        wire::decode(&spoken.out).expect("Doctor emits its typed outcome");
    let Outcome::Reply(reply) = decoded else {
        panic!("Doctor must complete inspection: {decoded:?}");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply
}

fn command<'a>(report: &'a DoctorReport<Root>, name: &str) -> &'a DoctorCommand {
    report
        .commands
        .iter()
        .find(|row| row.command == name)
        .unwrap_or_else(|| panic!("Doctor omitted command {name:?}"))
}

fn subsystem(report: &DoctorReport<Root>, name: DoctorSubsystem) -> &DoctorSubsystemDiagnosis {
    report
        .subsystems
        .iter()
        .find(|row| row.subsystem == name)
        .unwrap_or_else(|| panic!("Doctor omitted subsystem {name:?}"))
}

fn non_ready_reason(observation: &DoctorObservation) -> Option<&str> {
    match observation {
        DoctorObservation::Ready {} => None,
        DoctorObservation::Unavailable { reason }
        | DoctorObservation::InputRequired { reason }
        | DoctorObservation::NotApplicable { reason }
        | DoctorObservation::Unsupported { reason } => Some(reason),
    }
}

#[cfg(unix)]
struct HealthyFixture {
    repository: Scratch,
    bin: PathBuf,
    marker: PathBuf,
    journal: PathBuf,
    install: PathBuf,
    canvas: PathBuf,
    before: BTreeMap<PathBuf, Entry>,
}

#[cfg(unix)]
fn healthy_fixture() -> HealthyFixture {
    use std::os::unix::fs::PermissionsExt as _;

    let repository = Scratch::repository("healthy");
    repository.tracked("[lane]\nlease = \"20m\"\n");
    let journal = repository
        .path()
        .join(".git/.spice-rs/data/spicejournal.sqlite3");
    fs::write(&journal, b"not an opened journal\n").expect("write authority sentinel");
    let install = repository.path().join(".git/.spice-rs/install.json");
    fs::write(&install, b"not an install receipt\n").expect("write install sentinel");
    let canvas = repository.path().join(".git/.spice-rs/canvas");
    fs::write(&canvas, b"not a discovery document\n").expect("write transport sentinel");
    fs::set_permissions(&canvas, fs::Permissions::from_mode(0o600))
        .expect("secure transport sentinel");

    for (relative, bytes) in [
        (
            ".git/.spice/hooks/pre-commit",
            b"hook sentinel\n".as_slice(),
        ),
        (".git/.spice-rs/inbox/steering", b"steering sentinel\n"),
        (
            ".git/.spice-rs/artifacts/tasks/projection",
            b"projection sentinel\n",
        ),
        (".spice/team", b"team sentinel\n"),
    ] {
        let path = repository.path().join(relative);
        fs::create_dir_all(path.parent().expect("sentinel parent"))
            .expect("create state sentinel parent");
        fs::write(path, bytes).expect("write state sentinel");
    }

    let bin = repository.path().join("fixture-bin");
    fs::create_dir(&bin).expect("create fixture executable directory");
    let marker = repository.path().join("rtk-was-started");
    let rtk = bin.join("rtk");
    fs::write(
        &rtk,
        format!("#!/bin/sh\nprintf started > '{}'\n", marker.display()),
    )
    .expect("write mutation-probe executable");
    fs::set_permissions(&rtk, fs::Permissions::from_mode(0o755))
        .expect("make mutation-probe executable");
    repository.local(&format!(
        "[listen]\nwindow = \"45s\"\n[proof]\ncontainer = \"{}\"\n",
        rtk.display()
    ));
    let before = directory_state(repository.path());

    HealthyFixture {
        repository,
        bin,
        marker,
        journal,
        install,
        canvas,
        before,
    }
}

fn same_report(text: &Spoken, json: &Spoken) -> DoctorReport<Root> {
    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let text_body: DoctorReport<Root> =
        wire::decode(&text.out).expect("text is the stable Doctor body");
    let json_reply = reply(json);

    assert_eq!(json_reply.body, text_body);
    assert_eq!(json_reply.position, ReplyPosition::Direct {});
    json_reply.body
}

fn assert_task_metadata(report: &DoctorReport<Root>) {
    assert_eq!(
        report
            .subsystems
            .iter()
            .map(|row| row.subsystem)
            .collect::<Vec<_>>(),
        DoctorSubsystem::ALL,
    );
    assert_eq!(
        report.configuration.tasks().stems(),
        [
            "app",
            "atomic",
            "board",
            "bus",
            "canvas",
            "consent",
            "corpus",
            "dialect",
            "dist",
            "doctrine",
            "fold",
            "gate",
            "journal",
            "lane",
            "listen",
            "proof",
            "rim",
            "scheduler",
            "sill",
            "spice",
            "steer",
            "tape",
            "taste",
            "vendor",
            "wire",
        ]
    );
    assert_eq!(
        report.configuration.tasks().project_min_depth(),
        dialect::PROJECT_MIN_DEPTH as u64
    );
    assert_eq!(
        report.configuration.tasks().project_max_depth(),
        dialect::PROJECT_MAX_DEPTH as u64
    );
    assert_tracked_environment_vocabulary(report);
    for path in [
        "tasks.stems",
        "tasks.project_min_depth",
        "tasks.project_max_depth",
        "policy.env_names",
    ] {
        assert!(
            report
                .provenance
                .iter()
                .any(|row| { row.path == path && row.source == ConfigSource::Tracked }),
            "missing tracked provenance for {path}"
        );
    }
}

#[test]
fn both_forms_admit_the_exact_tracked_shared_metadata_without_mutation() {
    let repository = Scratch::repository("shared-metadata");
    let tracked = include_bytes!("../../spice-rust.toml");
    fs::write(repository.path().join("spice-rust.toml"), tracked)
        .expect("copy exact tracked configuration");
    let hook = repository.path().join(".git/.spice/hooks/pre-commit");
    fs::create_dir_all(hook.parent().expect("hook parent")).expect("create hook directory");
    fs::write(&hook, b"active operator hook sentinel\n").expect("write active hook sentinel");
    let empty_path = repository.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty executable search path");
    let before = directory_state(repository.path());

    let text = invoke(repository.path(), &["doctor"], empty_path.as_os_str(), &[]);
    let json = invoke(
        repository.path(),
        &["--json", "doctor"],
        empty_path.as_os_str(),
        &[],
    );
    let report = same_report(&text, &json);

    assert_task_metadata(&report);
    assert_eq!(
        fs::read(repository.path().join("spice-rust.toml")).unwrap(),
        tracked
    );
    assert_eq!(fs::read(&hook).unwrap(), b"active operator hook sentinel\n");
    assert_eq!(directory_state(repository.path()), before);
}

fn assert_tracked_environment_vocabulary(report: &DoctorReport<Root>) {
    // Doctor reports the variables Spice owns, while the tracked vocabulary also
    // accounts the ones its fixtures and Git shadow use. The gate's name ledger
    // is pinned separately in wire/tests/environment_vocabulary.rs.
    const UNOWNED: [&str; 14] = [
        "GIT_CONFIG_COUNT",              // env-policy: allow
        "GIT_CONFIG_SYSTEM",             // env-policy: allow
        "GIT_DIR",                       // env-policy: allow
        "GIT_EDITOR",                    // env-policy: allow
        "GIT_INDEX_FILE",                // env-policy: allow
        "GIT_WORK_TREE",                 // env-policy: allow
        "SPICE_CANVAS_BINARY",           // env-policy: allow
        "SPICE_HOOKS",                   // env-policy: allow
        "SPICE_INBOX",                   // env-policy: allow
        "SPICE_INSTALL_TEST_EXECUTABLE", // env-policy: allow
        "SPICE_LANE",                    // env-policy: allow
        "SPICE_NATIVE_SHELL",            // env-policy: allow
        "SPICE_PROOF",                   // env-policy: allow
        "TMPDIR",                        // env-policy: allow
    ];
    let vocabulary = report.configuration.policy().env_names();
    assert_eq!(
        vocabulary
            .iter()
            .filter(|name| !UNOWNED.contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>(),
        SpiceVariable::ALL
            .iter()
            .map(|variable| variable.as_str().to_owned())
            .collect::<Vec<_>>()
    );
    for name in UNOWNED {
        assert!(
            vocabulary.iter().any(|accounted| accounted == name),
            "the tracked vocabulary accounts {name}, which Doctor reports no row for"
        );
    }
}

fn assert_locations_configuration_and_provenance(report: &DoctorReport<Root>, repository: &Path) {
    assert_eq!(report.target.os, std::env::consts::OS);
    assert_eq!(report.target.architecture, std::env::consts::ARCH);
    let canonical = fs::canonicalize(repository).expect("canonical fixture");
    assert_eq!(report.locations.working, canonical.display().to_string());
    assert_eq!(report.locations.repository, canonical.display().to_string());
    assert_eq!(
        report.locations.repository_common,
        canonical.join(".git").display().to_string()
    );
    assert_eq!(
        report.locations.tracked_configuration,
        canonical.join("spice-rust.toml").display().to_string()
    );
    assert_eq!(
        report.locations.local_configuration,
        canonical
            .join(".git/.spice-rs/config.toml")
            .display()
            .to_string()
    );
    assert!(
        report
            .provenance
            .iter()
            .any(|row| { row.path == "lane.lease" && row.source == ConfigSource::Tracked })
    );
    assert!(
        report
            .provenance
            .iter()
            .any(|row| { row.path == "listen.window" && row.source == ConfigSource::Local })
    );
    assert!(
        report
            .provenance
            .iter()
            .any(|row| { row.path == "proof.container" && row.source == ConfigSource::Local })
    );
    assert!(
        report
            .provenance
            .iter()
            .any(|row| row.source == ConfigSource::Builtin)
    );
    assert_eq!(report.configuration.lane().lease().to_string(), "20m");
    assert_eq!(
        report.configuration.listen().window_word().to_string(),
        "45s"
    );
    assert!(report.configuration.proof().container().is_some());
}

fn assert_environment_presence(report: &DoctorReport<Root>) {
    assert_eq!(report.environment.len(), SpiceVariable::ALL.len());
    for row in &report.environment {
        if row.variable == SpiceVariable::Lane {
            assert!(matches!(row.observation, DoctorObservation::Ready {}));
        } else {
            assert!(matches!(
                row.observation,
                DoctorObservation::Unavailable { .. }
            ));
        }
    }
}

fn assert_tool_posture_and_text(report: &DoctorReport<Root>, text: &[u8]) {
    let doctor = command(report, "doctor");
    assert!(matches!(doctor.observation, DoctorObservation::Ready {}));
    assert!(doctor.tools.iter().any(|tool| {
        tool.name == "rtk"
            && tool.source == DoctorToolSource::WorktreeConfiguration
            && matches!(tool.observation, DoctorObservation::Ready {})
    }));
    assert!(matches!(
        command(report, "run").observation,
        DoctorObservation::InputRequired { .. }
    ));
    assert!(
        command(report, "proof")
            .tools
            .iter()
            .any(|tool| { matches!(tool.observation, DoctorObservation::Unsupported { .. }) })
    );
    assert!(command(report, "proof").tools.iter().any(|tool| {
        tool.source == DoctorToolSource::ProofRuntime
            && matches!(tool.observation, DoctorObservation::Ready {})
    }));

    let text = std::str::from_utf8(text).expect("Doctor text is UTF-8");
    assert!(text.contains("\"source\":\"tracked\""));
    assert!(text.contains("\"source\":\"local\""));
    for reason in report
        .environment
        .iter()
        .filter_map(|row| non_ready_reason(&row.observation))
        .chain(
            report
                .commands
                .iter()
                .filter_map(|row| non_ready_reason(&row.observation)),
        )
        .chain(report.commands.iter().flat_map(|row| {
            row.tools
                .iter()
                .filter_map(|tool| non_ready_reason(&tool.observation))
        }))
        .chain(
            report
                .subsystems
                .iter()
                .filter_map(|row| non_ready_reason(&row.observation)),
        )
    {
        assert!(text.contains(reason), "text omitted {reason:?}");
    }
}

#[cfg(unix)]
#[test]
fn both_forms_report_the_same_read_only_state_without_executing_rtk_or_leaking_environment() {
    let fixture = healthy_fixture();

    let environment = [(SpiceVariable::Lane, SECRET)];
    let text = invoke(
        fixture.repository.path(),
        &["doctor"],
        fixture.bin.as_os_str(),
        &environment,
    );
    let json = invoke(
        fixture.repository.path(),
        &["doctor", "--json"],
        fixture.bin.as_os_str(),
        &environment,
    );

    let report = same_report(&text, &json);
    assert!(
        !text
            .out
            .windows(SECRET.len())
            .any(|bytes| bytes == SECRET.as_bytes())
    );
    assert!(
        !json
            .out
            .windows(SECRET.len())
            .any(|bytes| bytes == SECRET.as_bytes())
    );
    assert_locations_configuration_and_provenance(&report, fixture.repository.path());
    assert_environment_presence(&report);
    assert_tool_posture_and_text(&report, &text.out);
    assert!(matches!(
        subsystem(&report, DoctorSubsystem::Journal).observation,
        DoctorObservation::Unavailable { .. }
    ));
    for name in [
        DoctorSubsystem::Board,
        DoctorSubsystem::Steering,
        DoctorSubsystem::Lane,
        DoctorSubsystem::Release,
    ] {
        assert!(matches!(
            subsystem(&report, name).observation,
            DoctorObservation::Unavailable { .. }
        ));
    }
    assert!(matches!(
        subsystem(&report, DoctorSubsystem::Fold).observation,
        DoctorObservation::NotApplicable { .. }
    ));
    assert!(matches!(
        subsystem(&report, DoctorSubsystem::Transport).observation,
        DoctorObservation::Unavailable { .. }
    ));
    assert!(matches!(
        subsystem(&report, DoctorSubsystem::Installation).observation,
        DoctorObservation::Unavailable { .. }
    ));

    assert!(
        !fixture.marker.exists(),
        "Doctor started the observed RTK executable"
    );
    assert_eq!(
        fs::read(&fixture.journal).expect("read authority sentinel"),
        b"not an opened journal\n"
    );
    assert_eq!(
        fs::read(&fixture.install).expect("read install sentinel"),
        b"not an install receipt\n"
    );
    assert_eq!(
        fs::read(&fixture.canvas).expect("read transport sentinel"),
        b"not a discovery document\n"
    );
    assert_eq!(directory_state(fixture.repository.path()), fixture.before);
}

#[test]
fn initialized_authority_and_folds_are_diagnosed_without_writing_any_state() {
    let repository = Scratch::repository("initialized-state");
    repository.tracked("");
    let journal_path = repository
        .path()
        .join(".git/.spice-rs/data/spicejournal.sqlite3");
    let journal = journal::Journal::open(&journal_path).expect("initialize journal fixture");
    assert_eq!(
        journal.schema_version().expect("fixture schema"),
        journal::SCHEMA_VERSION
    );
    drop(journal);
    let folds_path = repository
        .path()
        .join(".git/.spice-rs/data/spicefolds.sqlite3");
    drop(fold::Store::open(&folds_path).expect("initialize fold fixture"));
    let epoch: wire::CanonicalUuid = "00000000-0000-4000-8000-000000000051"
        .parse()
        .expect("fixture epoch");
    let resident_path = repository.path().join(".git/.spice-rs/resident");
    let lock = sill::LockFile::new(
        sill::LockName::new("resident").expect("fixture lock name"),
        &resident_path,
    );
    let sill::Acquisition::Acquired(publisher) =
        lock.try_acquire().expect("acquire fixture resident lock")
    else {
        panic!("fixture resident lock must be available");
    };
    publisher
        .rewrite_record(
            spice::resident_lock::holder_record(std::process::id(), Some(epoch)).as_bytes(),
        )
        .expect("publish fixture holder record");
    canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: "00000000-0000-4000-8000-000000000052"
            .parse()
            .expect("fixture capability"),
        epoch,
    }
    .publish(repository.path().join(".git/.spice-rs/canvas"))
    .expect("publish fixture discovery");
    let empty_path = repository.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty executable search path");
    let before = directory_state(repository.path());

    let text = invoke(repository.path(), &["doctor"], empty_path.as_os_str(), &[]);
    let json = invoke(
        repository.path(),
        &["--json", "doctor"],
        empty_path.as_os_str(),
        &[],
    );
    let report = same_report(&text, &json);

    for name in [
        DoctorSubsystem::Board,
        DoctorSubsystem::Journal,
        DoctorSubsystem::Fold,
        DoctorSubsystem::Steering,
        DoctorSubsystem::Transport,
    ] {
        assert!(matches!(
            subsystem(&report, name).observation,
            DoctorObservation::Ready {}
        ));
    }
    for name in [
        DoctorSubsystem::Lane,
        DoctorSubsystem::Installation,
        DoctorSubsystem::Release,
    ] {
        assert!(matches!(
            subsystem(&report, name).observation,
            DoctorObservation::NotApplicable { .. }
        ));
    }
    assert_eq!(directory_state(repository.path()), before);
}

#[test]
fn canonical_journal_violation_is_unavailable_without_writing_any_state() {
    let repository = Scratch::repository("corrupt-journal");
    repository.tracked("");
    let journal_path = repository
        .path()
        .join(".git/.spice-rs/data/spicejournal.sqlite3");
    drop(journal::Journal::open(&journal_path).expect("initialize journal fixture"));
    let connection = rusqlite::Connection::open(&journal_path).expect("open corruption fixture");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (9, 'task:DOCTOR-review', NULL, 9, 0);",
        )
        .expect("insert orphan thread row");
    drop(connection);
    let empty_path = repository.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty executable search path");
    let before = directory_state(repository.path());

    let spoken = invoke(
        repository.path(),
        &["doctor", "--json"],
        empty_path.as_os_str(),
        &[],
    );
    let report = reply(&spoken).body;

    assert!(matches!(
        subsystem(&report, DoctorSubsystem::Journal).observation,
        DoctorObservation::Unavailable { ref reason }
            if reason.contains("journal authority verification failed")
    ));
    assert_eq!(directory_state(repository.path()), before);
}

#[test]
fn missing_optional_rtk_is_an_unhealthy_row_inside_a_direct_reply() {
    let repository = Scratch::repository("missing-rtk");
    repository.tracked("");
    let empty_path = repository.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty executable search path");

    let spoken = invoke(
        repository.path(),
        &["--json", "doctor"],
        empty_path.as_os_str(),
        &[],
    );
    let reply = reply(&spoken);
    let report = &reply.body;
    let doctor = command(report, "doctor");

    assert!(matches!(
        doctor.observation,
        DoctorObservation::Unavailable { ref reason } if reason.contains("rtk")
    ));
    assert!(doctor.tools.iter().any(|tool| {
        tool.requirement == wire::DoctorToolRequirement::Optional
            && matches!(tool.observation, DoctorObservation::Unavailable { .. })
    }));
}

#[test]
fn missing_required_proof_runtime_is_an_unhealthy_row_inside_a_direct_reply() {
    let repository = Scratch::repository("missing-proof-runtime");
    repository.tracked("");
    let missing = repository.path().join("missing-container-runtime");
    repository.local(&format!("[proof]\ncontainer = \"{}\"\n", missing.display()));
    let empty_path = repository.path().join("empty-path");
    fs::create_dir(&empty_path).expect("create empty executable search path");

    let spoken = invoke(
        repository.path(),
        &["doctor", "--json"],
        empty_path.as_os_str(),
        &[],
    );
    let report = reply(&spoken).body;
    let runtime = command(&report, "proof")
        .tools
        .iter()
        .find(|tool| tool.source == DoctorToolSource::ProofRuntime)
        .expect("Proof runtime declaration");

    assert!(matches!(
        runtime.observation,
        DoctorObservation::Unavailable { ref reason }
            if reason.contains("missing-container-runtime")
    ));
}

#[test]
fn malformed_and_unknown_configuration_are_typed_refusals_from_both_exact_paths() {
    for (label, tracked, local, expected) in [
        ("tracked-syntax", "[lane\n", "", "spice-rust.toml"),
        (
            "local-syntax",
            "",
            "[listen]\nwindow = [\n",
            ".spice-rs/config.toml",
        ),
        ("unknown", "[unknown]\nvalue = 1\n", "", "unknown"),
    ] {
        let repository = Scratch::repository(label);
        repository.tracked(tracked);
        repository.local(local);
        let empty_path = repository.path().join("empty-path");
        fs::create_dir(&empty_path).expect("create empty executable search path");

        let spoken = invoke(
            repository.path(),
            &["doctor", "--json"],
            empty_path.as_os_str(),
            &[],
        );
        assert_eq!(spoken.code, Some(2), "{label}");
        assert!(spoken.out.is_empty(), "{label}");
        let outcome: Outcome<DoctorReport<Root>> =
            wire::decode(&spoken.err).expect("configuration refusal is typed");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("{label} must refuse: {outcome:?}");
        };
        assert_eq!(refusal.reason(), "doctor::configuration-refused", "{label}");
        assert!(
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .is_some_and(|observed| observed.contains(expected)),
            "{label}: {refusal:?}"
        );
    }
}

#[test]
fn doctor_exposes_no_fix_or_aggregate_execution_path() {
    let repository = Scratch::repository("no-fix");
    repository.tracked("");
    let spoken = invoke(repository.path(), &["doctor", "--fix"], OsStr::new(""), &[]);

    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let stderr = String::from_utf8(spoken.err).expect("Clap speaks UTF-8");
    assert!(stderr.contains("unexpected argument '--fix'"));
    assert!(!stderr.contains(PlaceholderReason::NotLanded.as_str()));
}

#[derive(Debug, Eq, PartialEq)]
enum Entry {
    Directory,
    File(Vec<u8>),
}

fn directory_state(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut state = BTreeMap::new();
    collect_directory_state(root, root, &mut state);
    state
}

fn collect_directory_state(root: &Path, directory: &Path, state: &mut BTreeMap<PathBuf, Entry>) {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory).expect("read fixture directory") {
        let path = entry.expect("read fixture entry").path();
        let relative = path
            .strip_prefix(root)
            .expect("fixture entry remains below root")
            .to_path_buf();
        assert!(names.insert(relative.clone()), "fixture paths are unique");
        let metadata = fs::symlink_metadata(&path).expect("read fixture metadata");
        if metadata.is_dir() {
            assert!(state.insert(relative, Entry::Directory).is_none());
            collect_directory_state(root, &path, state);
        } else {
            assert!(
                state
                    .insert(
                        relative,
                        Entry::File(fs::read(&path).expect("read fixture bytes")),
                    )
                    .is_none()
            );
        }
    }
}

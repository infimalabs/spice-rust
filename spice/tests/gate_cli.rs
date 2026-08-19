#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gate::{
    BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryReceipt, BatteryReport, BatteryState, CampReport,
    CampStanding, ChangeReachAuthority, ChangeReachMode, ChangeReachObject, ChangeSelectionReport,
    CoverageCompleteness, GateId, TestSelectionReport,
};
use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Output,
    ProcessGroup, Redaction, Running, Spec, StdinPolicy, StreamPolicy, WaitPolicy, WaitStep,
};
use spice::change_gate::ChangeGateReport;
use spice::placeholder::PlaceholderReason;
use wire::{Digest, Domain, Outcome, Refusal, ReplyPosition};

#[path = "gate_cli/cargo_optional.rs"]
mod cargo_optional;
#[path = "gate_cli/dispatch.rs"]
mod dispatch;
#[path = "gate_cli/fixture.rs"]
mod fixture;
#[path = "gate_cli/refusal.rs"]
mod refusal_artifact;

use fixture::*;

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;
const PROCESS_PATH: &str = concat!("PA", "TH");
const PRE_COMMIT_PATHS: &str = wire::SpiceVariable::PreCommitPaths.as_str();
const PRE_COMMIT_SOURCE: &str = wire::SpiceVariable::PreCommitSource.as_str();
const COMMIT_WORKER_STARTUP_ALLOWANCE: Duration = Duration::from_secs(15);
const COMMIT_WORKER_CONTAINMENT_RUNTIME: Duration = Duration::from_secs(2);
const FIXTURE_PUBLICATION_INACTIVITY: Duration = Duration::from_secs(5);
const FIXTURE_PUBLICATION_POLL: Duration = Duration::from_millis(50);
const COMPACT_CHANGE_REPORT_MAX_BYTES: usize = 128 * 1_024;

#[test]
fn commit_worker_executes_known_targets_and_refuses_unknown_sources_before_selected_work() {
    let marker_scratch = Scratch::new("selected-test-marker");
    let marker = marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&marker);
    repository.add("src/lib.rs");

    let known = run_commit_worker(repository.path());
    let Some((kind, document)) = known.terminal.split_first() else {
        panic!("known staged source produced no terminal record");
    };
    assert_eq!(*kind, b'R', "known staged source did not admit: {known:?}");
    let gate_report = packaged_change_report(document);
    assert_eq!(
        gate_report.selection().mode(),
        gate::ChangeSelectionMode::Local,
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("selected target wrote its execution marker"),
        "canonical selected target executed\n",
    );
    let report = selection_report(&known.progress);
    assert!(report.admits());
    assert_eq!(report.changed_sources(), &["src/lib.rs"]);
    assert_eq!(report.engines(), &["cargo-workspace"]);
    assert_eq!(report.tests(), &["gate-fixture::lib:gate_fixture"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.edges()[0].source(), "src/lib.rs");
    assert_eq!(report.edges()[0].test(), "gate-fixture::lib:gate_fixture");
    assert_eq!(report.coverage(), &CoverageCompleteness::Complete);
    assert_execution_matches_reach(&known.progress);

    let refused_marker_scratch = Scratch::new("refused-selected-test-marker");
    let refused_marker = refused_marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&refused_marker);
    repository.add("src/lib.rs");
    fs::create_dir(repository.path().join("tools")).expect("create unknown-source directory");
    fs::write(
        repository.path().join("tools/outside.rs"),
        "pub fn outside_every_package() {}\n",
    )
    .expect("write unknown staged Rust source");
    repository.add("tools/outside.rs");

    let unknown = run_commit_worker(repository.path());
    assert_selection_refusal(
        &unknown,
        "\"tools/outside.rs\" is Rust source outside every compiled workspace root",
        &refused_marker,
    );
    let authority_marker_scratch = Scratch::new("authority-selected-test-marker");
    let authority_marker = authority_marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&authority_marker);
    repository.add("src/lib.rs");
    let policy = repository.path().join("spice-rust.toml");
    let mut policy_text = fs::read_to_string(&policy).expect("read authority fixture policy");
    policy_text.push_str("\n# staged authority change\n");
    fs::write(&policy, policy_text).expect("write valid authority change");
    repository.add("spice-rust.toml");

    let authority = run_commit_worker(repository.path());
    assert_selection_refusal(
        &authority,
        "\"spice-rust.toml\" changes packaged selection policy",
        &authority_marker,
    );
}

#[test]
#[allow(
    clippy::disallowed_methods,
    reason = "the configured process-boundary fixture must preserve the invoking test PATH"
)]
fn mixed_language_worker_executes_exactly_its_published_components() {
    let repository = GateRepository::new();
    let root = repository.path();
    commit_mixed_language_fixture(&repository);
    stage_mixed_language_changes(&repository);

    let scratch = Scratch::new("mixed-language-reach");
    let programs = scratch.path().join("programs");
    fs::create_dir(&programs).expect("create mixed reach program directory");
    let pyright_log = scratch.path().join("pyright.log");
    executable(
        &programs.join("pyright"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then printf '%s\\n' 'pyright 1.1.411'; exit 0; fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "printf '%s\\n' '{{\"generalDiagnostics\":[],\"summary\":{{\"errorCount\":0}}}}'\n",
            ),
            pyright_log.display(),
        ),
    );
    let mut path_entries = vec![programs];
    path_entries.extend(std::env::split_paths(
        &std::env::var_os(PROCESS_PATH).expect("test process carries PATH"),
    ));
    let path = std::env::join_paths(path_entries).expect("compose mixed reach PATH");

    let boundary = run_commit_worker_with_path(root, Some(Path::new(&path)));

    assert_eq!(
        boundary.terminal.first(),
        Some(&b'R'),
        "mixed fixture refused before complete execution: {}",
        String::from_utf8_lossy(&boundary.terminal)
    );
    assert_execution_matches_reach(&boundary.progress);
    let selection = change_selection_report(&boundary.progress);
    assert_eq!(
        reached_paths(&selection, GateId::JavascriptUnused),
        ["web/dep.js", "web/entry.js"],
    );
    assert_eq!(
        reached_paths(&selection, GateId::CsharpUnusedCandidates),
        ["csharp/Child.cs", "csharp/Widget.cs"],
    );
    assert_eq!(
        reached_paths(&selection, GateId::PythonTypecheck),
        [
            "src/pkg/__init__.py",
            "src/pkg/a.py",
            "src/pkg/b.py",
            "tests/test_pkg.py",
        ],
    );
    assert_eq!(
        reached_paths(&selection, GateId::MarkdownLinks),
        ["docs/changed.md", "src/lib.rs"],
    );
    let pyright = fs::read_to_string(pyright_log).expect("read Pyright execution log");
    assert_eq!(
        pyright.lines().count(),
        1,
        "Pyright must execute once: {pyright}"
    );
    for reached in [
        "src/pkg/__init__.py",
        "src/pkg/a.py",
        "src/pkg/b.py",
        "tests/test_pkg.py",
    ] {
        assert!(
            pyright.contains(reached),
            "Pyright omitted {reached}: {pyright}"
        );
    }
    assert!(!pyright.contains("other/unused.py"), "{pyright}");
    let terminal = String::from_utf8_lossy(&boundary.terminal);
    for unrelated in [
        "other/isolated.js",
        "other/Other.cs",
        "other/unused.py",
        "docs/unrelated.md",
    ] {
        assert!(
            !terminal.contains(unrelated),
            "unrelated finding escaped focused execution: {unrelated}"
        );
    }
}

fn commit_mixed_language_fixture(repository: &GateRepository) {
    let root = repository.path();
    let policy_path = root.join("spice-rust.toml");
    let policy = fs::read_to_string(&policy_path)
        .expect("read mixed reach policy")
        .replace("[policy]\n", "[policy]\npackage_roots = [\"src/pkg\"]\n");
    fs::write(policy_path, policy).expect("configure explicit Python package directory");
    for directory in ["web", "csharp", "src/pkg", "tests", "other", "docs"] {
        fs::create_dir_all(root.join(directory)).expect("create mixed reach fixture directory");
    }
    fs::write(
        root.join("package.json"),
        "{\"name\":\"mixed-reach-fixture\"}\n",
    )
    .expect("write JavaScript package authority");
    fs::write(root.join("web/dep.js"), "export const used = 1;\n")
        .expect("write JavaScript dependency");
    fs::write(
        root.join("web/entry.js"),
        "import { used } from './dep.js';\nconsole.log(used);\n",
    )
    .expect("write JavaScript dependent");
    fs::write(root.join("other/isolated.js"), "const unrelatedDebt = 1;\n")
        .expect("write unrelated JavaScript debt");
    fs::write(
        root.join("csharp/Widget.cs"),
        "namespace Product.Core;\nclass Widget { }\n",
    )
    .expect("write C# dependency");
    fs::write(
        root.join("csharp/Child.cs"),
        "using Product.Core;\nclass Child : Widget { }\n",
    )
    .expect("write C# dependent");
    fs::write(
        root.join("other/Other.cs"),
        "namespace Other;\nclass OtherType { private int unrelatedDebt; }\n",
    )
    .expect("write unrelated C# debt");
    fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = 'mixed-reach'\n",
    )
    .expect("write Python project authority");
    fs::write(root.join("src/pkg/__init__.py"), "\n").expect("write Python package");
    fs::write(root.join("src/pkg/b.py"), "# dependency generation 1\n")
        .expect("write Python dependency");
    fs::write(root.join("src/pkg/a.py"), "from pkg import b\n").expect("write Python dependent");
    fs::write(root.join("tests/test_pkg.py"), "from pkg import a\n")
        .expect("write Python test dependent");
    fs::write(
        root.join("other/unused.py"),
        "def unrelated_debt():\n    pass\n",
    )
    .expect("write unrelated Python debt");
    fs::write(root.join("docs/changed.md"), "# Mixed reach\n")
        .expect("write changed Markdown source");
    fs::write(root.join("docs/unrelated.md"), "[broken](missing.md)\n")
        .expect("write unrelated Markdown debt");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "mixed reach fixture"]);
}

fn stage_mixed_language_changes(repository: &GateRepository) {
    let root = repository.path();
    fs::write(root.join("web/dep.js"), "export const used = 2;\n")
        .expect("change JavaScript dependency");
    fs::write(
        root.join("csharp/Widget.cs"),
        "namespace Product.Core;\nclass Widget { } // generation 2\n",
    )
    .expect("change C# dependency");
    fs::write(root.join("src/pkg/b.py"), "# dependency generation 2\n")
        .expect("change Python dependency");
    fs::write(root.join("docs/changed.md"), "# Mixed reach generation 2\n")
        .expect("change Markdown source");
    repository.write_source(2);
    for path in [
        "web/dep.js",
        "csharp/Widget.cs",
        "src/pkg/b.py",
        "docs/changed.md",
        "src/lib.rs",
    ] {
        repository.add(path);
    }
}

#[test]
fn rename_and_delete_worker_binds_both_sides_without_scanning_unrelated_debt() {
    let repository = GateRepository::new();
    let root = repository.path();
    fs::create_dir(root.join("docs")).expect("create rename fixture directory");
    fs::write(root.join("docs/old.md"), "# Renamed document\n").expect("write rename source");
    fs::write(root.join("docs/deleted.md"), "# Deleted document\n").expect("write deletion source");
    fs::write(root.join("docs/unrelated.md"), "[broken](missing.md)\n")
        .expect("write unrelated Markdown debt");
    git(root, &["add", "docs"]);
    git(root, &["commit", "-qm", "rename fixture"]);
    git(root, &["mv", "docs/old.md", "docs/new.md"]);
    git(root, &["rm", "-q", "docs/deleted.md"]);

    let boundary = run_commit_worker(root);

    assert_eq!(
        boundary.terminal.first(),
        Some(&b'R'),
        "rename/delete fixture refused: {}",
        String::from_utf8_lossy(&boundary.terminal)
    );
    assert_execution_matches_reach(&boundary.progress);
    let selection = change_selection_report(&boundary.progress);
    assert_eq!(
        selection.changed_sources(),
        ["docs/deleted.md", "docs/new.md", "docs/old.md"]
    );
    for gate_id in [GateId::Taste, GateId::MarkdownLinks] {
        assert_eq!(
            reached_paths(&selection, gate_id),
            ["docs/deleted.md", "docs/new.md", "docs/old.md"],
            "{gate_id} lost one rename/delete side"
        );
    }
    assert!(
        !String::from_utf8_lossy(&boundary.terminal).contains("docs/unrelated.md"),
        "unrelated Markdown debt escaped focused execution"
    );
}

#[test]
fn dynamic_javascript_worker_publishes_and_executes_explicit_exhaustive_reach() {
    let repository = GateRepository::new();
    let root = repository.path();
    fs::create_dir_all(root.join("web")).expect("create dynamic JavaScript directory");
    fs::create_dir_all(root.join("other")).expect("create exhaustive peer directory");
    fs::write(
        root.join("package.json"),
        "{\"name\":\"dynamic-reach-fixture\"}\n",
    )
    .expect("write dynamic package authority");
    fs::write(root.join("web/changed.js"), "console.log('initial');\n")
        .expect("write initial JavaScript source");
    fs::write(root.join("web/dependency.js"), "export const VALUE = 1;\n")
        .expect("write dynamic dependency");
    fs::write(
        root.join("other/peer.js"),
        "export function exhaustivePeer() {}\n",
    )
    .expect("write exhaustive peer debt");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "dynamic reach fixture"]);
    fs::write(
        root.join("web/changed.js"),
        "const target = './dependency.js';\nimport(target);\n",
    )
    .expect("write dynamic JavaScript change");
    repository.add("web/changed.js");

    let boundary = run_commit_worker(root);

    assert_execution_matches_reach(&boundary.progress);
    let selection = change_selection_report(&boundary.progress);
    let reach = selection
        .reach_for(GateId::JavascriptUnused)
        .expect("JavaScript arm retains exhaustive reach");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("dynamic or unsupported specifier")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    let terminal = String::from_utf8_lossy(&boundary.terminal);
    assert!(
        terminal.contains("javascript-unused found 2 findings across 3 selected paths"),
        "explicit exhaustive execution omitted its three-file, two-finding inventory: {terminal}"
    );
}

#[test]
fn commit_worker_deadline_terminates_the_active_gate_tool_process_group() {
    let repository = GateRepository::new();
    repository.write_source(2);
    repository.add("src/lib.rs");
    let scratch = Scratch::new("deadline-process-tree");
    let programs = scratch.path().join("programs");
    fs::create_dir(&programs).expect("create deadline program directory");
    let tool_pid = scratch.path().join("tool.pid");
    executable(
        &programs.join("cargo"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"--version\" ]; then\n",
            "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
            "  exit 0\n",
            "fi\n",
            "case \"$1\" in\n",
            "  metadata)\n",
            "    root=$(/bin/pwd -P)\n",
            "    printf '%s\\n' ",
            "'{\"packages\":[{\"id\":\"gate-fixture-id\",\"name\":\"gate-fixture\",",
            "\"manifest_path\":\"'\"$root\"'/Cargo.toml\",\"dependencies\":[],",
            "\"targets\":[{\"name\":\"gate_fixture\",\"kind\":[\"lib\"],",
            "\"test\":true,\"src_path\":\"'\"$root\"'/src/lib.rs\"}]}],",
            "\"workspace_members\":[\"gate-fixture-id\"]}'\n",
            "    exit 0\n",
            "    ;;\n",
            "  test) exit 0 ;;\n",
            "  fmt) exit 0 ;;\n",
            "  check) printf '%s\\n' 'unexpected stale cargo check arm' >&2; exit 97 ;;\n",
            "  *) exit 0 ;;\n",
            "esac\n",
        ),
    );
    executable(
        &programs.join("rustfmt"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n",
                "  exit 0\n",
                "fi\n",
                "/bin/sleep 30 & child=$!\n",
                "printf '%s\\n' \"$child\" > '{}'\n",
                "wait\n",
            ),
            tool_pid.display(),
        ),
    );
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create deadline progress file");
    fs::write(&terminal, []).expect("create deadline terminal file");
    let cancellation = CancellationToken::new();
    let absolute = COMMIT_WORKER_STARTUP_ALLOWANCE + COMMIT_WORKER_CONTAINMENT_RUNTIME;
    let spec = deadline_worker_spec(
        repository.path(),
        &programs,
        &progress,
        &terminal,
        cancellation.clone(),
    );

    let running = sill::spawn(spec).expect("launch hidden commit worker");
    let (running, publication) = await_fixture_publication(
        running,
        &cancellation,
        &progress,
        &tool_pid,
        PublicationBounds {
            inactivity: FIXTURE_PUBLICATION_INACTIVITY,
            absolute,
        },
    )
    .unwrap_or_else(|detail| panic!("observe active gate-tool descendant: {detail}"));
    let owned_group = observed_process_group(publication.pid)
        .expect("inspect published gate-tool descendant ownership");
    assert!(
        owned_group == Some(running.id()),
        "published gate-tool descendant {} belongs to group {owned_group:?}, not owned group {}",
        publication.pid,
        running.id(),
    );
    assert!(publication.generation >= 2, "{publication:?}");
    assert!(publication.reason.contains("descendant-publication"));
    assert!(publication.absolute_remaining < absolute);

    cancellation.cancel();
    let waited = running
        .wait()
        .expect("cancel and reap hidden commit worker group");

    assert!(matches!(waited.completion(), Completion::Cancelled(_)));
    assert!(
        observed_process_group(publication.pid)
            .expect("inspect cancelled gate-tool descendant absence")
            .is_none(),
        "commit worker cancellation leaked gate-tool descendant {}",
        publication.pid,
    );
    let progress = fs::read_to_string(progress).expect("read worker phase evidence");
    assert!(
        progress.contains("[change-gate-policy] gate=formatters"),
        "deadline phase evidence omitted the active selected formatter arm: {progress}"
    );
}

#[derive(Clone, Copy)]
struct PublicationBounds {
    inactivity: Duration,
    absolute: Duration,
}

#[derive(Debug)]
struct FixturePublication {
    pid: u32,
    generation: u64,
    reason: String,
    absolute_remaining: Duration,
}

fn await_fixture_publication(
    mut running: Running,
    cancellation: &CancellationToken,
    progress: &Path,
    publication: &Path,
    bounds: PublicationBounds,
) -> Result<(Running, FixturePublication), String> {
    let started = Instant::now();
    let mut last_progress = started;
    let mut generation = 0_u64;
    let mut reason = "worker-launch".to_owned();
    let mut progress_records = BTreeSet::new();
    loop {
        match newly_observed_progress(progress, &mut progress_records) {
            Ok(records) if !records.is_empty() => {
                generation = generation.saturating_add(records.len() as u64);
                last_progress = Instant::now();
                reason = records.last().expect("new records are nonempty").clone();
            }
            Ok(_) => {}
            Err(error) => {
                return Err(stop_publication_fixture(
                    running,
                    cancellation,
                    format!("read typed worker progress: {error}"),
                ));
            }
        }
        match published_pid(publication) {
            Ok(Some(pid)) => {
                generation = generation.saturating_add(1);
                reason = "descendant-publication".to_owned();
                return Ok((
                    running,
                    FixturePublication {
                        pid,
                        generation,
                        reason,
                        absolute_remaining: bounds.absolute.saturating_sub(started.elapsed()),
                    },
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(stop_publication_fixture(
                    running,
                    cancellation,
                    format!("invalid descendant publication: {error}"),
                ));
            }
        }

        let now = Instant::now();
        let absolute_remaining = bounds.absolute.saturating_sub(now.duration_since(started));
        let inactivity_remaining = bounds
            .inactivity
            .saturating_sub(now.duration_since(last_progress));
        let expiry = if absolute_remaining.is_zero() {
            Some("absolute")
        } else if inactivity_remaining.is_zero() {
            Some("inactivity")
        } else {
            None
        };
        if let Some(expiry) = expiry {
            return Err(stop_publication_fixture(
                running,
                cancellation,
                format!(
                    "publication {expiry} expired at generation={generation} reason={reason:?}"
                ),
            ));
        }

        let observation = FIXTURE_PUBLICATION_POLL
            .min(absolute_remaining)
            .min(inactivity_remaining);
        match running
            .wait_for(observation)
            .map_err(|error| format!("observe commit worker: {error}"))?
        {
            WaitStep::Running(next) => running = *next,
            WaitStep::Exited(outcome) => {
                return Err(format!(
                    "commit worker ended as {:?} before descendant publication; \
                     generation={generation} reason={reason:?}",
                    outcome.completion(),
                ));
            }
        }
    }
}

fn published_pid(path: &Path) -> std::io::Result<Option<u32>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if text.is_empty() || !text.ends_with('\n') {
        return Ok(None);
    }
    let mut records = text.lines();
    let Some(record) = records.next() else {
        return Ok(None);
    };
    if records.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "descendant publication contains more than one record",
        ));
    }
    record
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .map(Some)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "descendant PID is not a positive integer",
            )
        })
}

fn observed_process_group(pid: u32) -> std::io::Result<Option<u32>> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "descendant PID exceeds pid_t",
        )
    })?;
    // SAFETY: getpgid only observes the exact published process identity.
    let group = unsafe { libc::getpgid(pid) };
    if group >= 0 {
        return u32::try_from(group).map(Some).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "observed process group is negative",
            )
        });
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

fn newly_observed_progress(
    path: &Path,
    observed: &mut BTreeSet<String>,
) -> std::io::Result<Vec<String>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .split_inclusive('\n')
        .filter_map(|record| record.strip_suffix('\n'))
        .filter(|record| record.starts_with("[change-gate-") || record.starts_with("[rust-test"))
        .filter(|record| observed.insert((*record).to_owned()))
        .map(str::to_owned)
        .collect())
}

fn stop_publication_fixture(
    running: Running,
    cancellation: &CancellationToken,
    detail: String,
) -> String {
    cancellation.cancel();
    match running.wait() {
        Ok(outcome) => format!("{detail}; cleanup={:?}", outcome.completion()),
        Err(error) => format!("{detail}; cleanup-error={error}"),
    }
}

#[test]
fn fixture_progress_renews_only_for_distinct_typed_records() {
    let scratch = Scratch::new("typed-publication-progress");
    let progress = scratch.path().join("progress.jsonl");
    fs::write(
        &progress,
        concat!(
            "raw compiler bytes\n",
            "[change-gate-policy] gate=formatters\n",
            "[change-gate-policy] gate=formatters\n",
        ),
    )
    .expect("write repeated fixture progress");
    let mut observed = BTreeSet::new();

    assert_eq!(
        newly_observed_progress(&progress, &mut observed).expect("observe typed progress"),
        ["[change-gate-policy] gate=formatters"]
    );
    assert!(
        newly_observed_progress(&progress, &mut observed)
            .expect("observe duplicate progress")
            .is_empty(),
        "duplicate and raw bytes renewed the fixture lease",
    );
    fs::write(
        &progress,
        concat!(
            "raw compiler bytes\n",
            "[change-gate-policy] gate=formatters\n",
            "[rust-test] stage=terminal shard=1/1 disposition=success",
        ),
    )
    .expect("write partial typed fixture progress");
    assert!(
        newly_observed_progress(&progress, &mut observed)
            .expect("observe partial progress")
            .is_empty(),
        "partial typed bytes renewed the fixture lease",
    );
    fs::write(
        &progress,
        concat!(
            "raw compiler bytes\n",
            "[change-gate-policy] gate=formatters\n",
            "[rust-test] stage=terminal shard=1/1 disposition=success\n",
        ),
    )
    .expect("complete newly advanced fixture progress");
    assert_eq!(
        newly_observed_progress(&progress, &mut observed).expect("observe new typed progress"),
        ["[rust-test] stage=terminal shard=1/1 disposition=success"]
    );
}

#[test]
#[allow(
    clippy::disallowed_methods,
    reason = "the process-boundary fixture preserves the invoking test environment"
)]
fn absent_descendant_publication_expires_at_the_inactivity_owner_and_reaps() {
    let scratch = Scratch::new("absent-descendant-publication");
    let progress = scratch.path().join("progress.jsonl");
    let publication = scratch.path().join("never-published.pid");
    fs::write(&progress, []).expect("create empty fixture progress");
    let cancellation = CancellationToken::new();
    let capture = StreamPolicy::Capture { limit: 4096 };
    let spec = Spec {
        argv: Argv::new("/bin/sleep", ["30"]).expect("declare silent fixture"),
        cwd: scratch.path().to_owned(),
        environment: Environment::new(env::vars_os()).expect("capture silent fixture environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            Duration::from_millis(250),
            Duration::from_millis(100),
            Duration::from_millis(100),
        )
        .expect("declare silent fixture bound"),
        wait: WaitPolicy::new(
            "absent descendant publication fixture",
            Cancellation::Token(cancellation.clone()),
            sill::RestartDisposition::Never,
        )
        .expect("declare silent fixture wait"),
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: Duration::from_millis(100),
    };
    let started = Instant::now();

    let error = await_fixture_publication(
        sill::spawn(spec).expect("launch silent fixture"),
        &cancellation,
        &progress,
        &publication,
        PublicationBounds {
            inactivity: Duration::from_millis(40),
            absolute: Duration::from_millis(200),
        },
    )
    .expect_err("silent fixture publication expires");

    assert!(error.contains("publication inactivity expired"), "{error}");
    assert!(error.contains("cleanup=Cancelled"), "{error}");
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process-boundary fixture must preserve the invoking test environment"
)]
fn deadline_worker_spec(
    root: &Path,
    programs: &Path,
    progress: &Path,
    terminal: &Path,
    cancellation: CancellationToken,
) -> Spec {
    let mut environment = env::vars_os().collect::<BTreeMap<_, _>>();
    for variable in [
        wire::SpiceVariable::PreCommitPaths,
        wire::SpiceVariable::PreCommitSource,
    ] {
        environment.remove(OsStr::new(variable.as_str()));
    }
    environment.insert(
        OsString::from(PROCESS_PATH),
        programs.as_os_str().to_owned(),
    );
    let capture = StreamPolicy::Capture { limit: 1024 * 1024 };
    Spec {
        argv: Argv::new(
            env!("CARGO_BIN_EXE_spice"),
            [
                OsString::from("__gate-pre-commit-worker"),
                progress.as_os_str().to_owned(),
                terminal.as_os_str().to_owned(),
                OsString::from(r#"{"version":1}"#),
            ],
        )
        .expect("declare hidden commit worker"),
        cwd: root.to_path_buf(),
        environment: Environment::new(environment).expect("capture deadline environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            COMMIT_WORKER_STARTUP_ALLOWANCE + COMMIT_WORKER_CONTAINMENT_RUNTIME,
            Duration::from_millis(200),
            Duration::from_millis(200),
        )
        .expect("declare scaled deadline"),
        wait: WaitPolicy::new(
            "commit worker process-tree fixture",
            Cancellation::Token(cancellation),
            sill::RestartDisposition::Never,
        )
        .expect("declare worker wait"),
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: Duration::from_millis(200),
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[derive(Debug)]
struct WorkerBoundary {
    progress: String,
    terminal: Vec<u8>,
}

fn run_commit_worker(root: &Path) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-pre-commit-worker", None)
}

fn run_commit_worker_with_path(root: &Path, path: Option<&Path>) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-pre-commit-worker", path)
}

fn run_resolution_worker(root: &Path) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-resolution-worker", None)
}

fn run_worker_with_path(root: &Path, worker: &str, path: Option<&Path>) -> WorkerBoundary {
    let scratch = Scratch::new("commit-worker-boundary");
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create worker progress file");
    fs::write(&terminal, []).expect("create worker terminal file");
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command.arg(worker).arg(&progress).arg(&terminal);
    if worker == "__gate-pre-commit-worker" {
        command.arg(r#"{"version":1}"#);
    }
    command
        .current_dir(root)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .env_remove(steer::hook::INBOX);
    if let Some(path) = path {
        command.env(PROCESS_PATH, path);
    }
    let output = command
        .output()
        .expect("launch normal commit worker boundary");
    assert!(
        output.status.success(),
        "commit worker failed before publishing its boundary: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    WorkerBoundary {
        progress: fs::read_to_string(progress).expect("read commit worker progress"),
        terminal: fs::read(terminal).expect("read commit worker terminal"),
    }
}

fn assert_selection_refusal(boundary: &WorkerBoundary, detail: &str, test_marker: &Path) {
    let Some((kind, document)) = boundary.terminal.split_first() else {
        panic!("refused staged source produced no terminal record");
    };
    assert_eq!(
        *kind, b'F',
        "staged source did not refuse at selection: {boundary:?}"
    );
    let failure: serde_json::Value =
        serde_json::from_slice(document).expect("decode selected-change refusal boundary");
    assert_eq!(
        failure["reason"], "gate::change-selection-refused",
        "worker surfaced the wrong refusal: {failure}",
    );
    let actual = failure["detail"]
        .as_str()
        .expect("selection refusal detail is text");
    assert!(
        actual.starts_with(
            "full-verifier admission missed at stage=document: \
             no full-verifier evidence is published for this worktree; "
        ),
        "worker collapsed the exact evidence-admission miss: {actual}",
    );
    assert!(
        actual.ends_with(detail),
        "worker replaced the selection cause: {actual}",
    );
    assert_eq!(failure["fault"], false);
    assert!(!test_marker.exists(), "refused selection executed a test");
    assert!(
        boundary
            .progress
            .contains("[change-gate-full-verifier] disposition=miss stage=document")
            && !boundary.progress.contains("[change-gate-policy]")
            && !boundary.progress.contains("[change-gate-execution]")
            && !boundary.progress.contains("[rust-test"),
        "refused selection started policy or workspace tests: {}",
        boundary.progress,
    );
}

fn selection_report(progress: &str) -> TestSelectionReport {
    const PREFIX: &str = "[rust-test-selection-report] ";
    let encoded = progress
        .lines()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(
        encoded.len(),
        1,
        "worker must publish exactly one selection report: {progress}"
    );
    TestSelectionReport::decode(encoded[0].as_bytes()).expect("decode worker selection report")
}

fn assert_execution_matches_reach(progress: &str) {
    const EXECUTION_PREFIX: &str = "[change-gate-execution] gate=";
    let report = change_selection_report(progress);
    let expected = report
        .reach()
        .iter()
        .map(|reach| {
            (
                reach.gate_id().as_str().to_owned(),
                reach.objects().to_vec(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut executed = BTreeMap::new();
    for line in progress
        .lines()
        .filter_map(|line| line.strip_prefix(EXECUTION_PREFIX))
    {
        let (gate, objects) = line
            .split_once(" objects=")
            .expect("execution scope separates gate and objects");
        let objects = serde_json::from_str::<Vec<ChangeReachObject>>(objects)
            .expect("execution scope carries typed objects");
        assert!(
            executed.insert(gate.to_owned(), objects).is_none(),
            "gate executed more than once: {gate}: {progress}"
        );
    }
    assert_eq!(
        executed, expected,
        "executed scope diverged from reach plan: {progress}"
    );
}

fn change_selection_report(progress: &str) -> ChangeSelectionReport {
    const PREFIX: &str = "[change-gate-selection-report] ";
    let reports = progress
        .lines()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(
        reports.len(),
        1,
        "worker must publish exactly one change reach report: {progress}"
    );
    ChangeSelectionReport::decode(reports[0].as_bytes()).expect("decode worker change reach report")
}

fn reached_paths(report: &ChangeSelectionReport, gate_id: GateId) -> Vec<&str> {
    report
        .reach_for(gate_id)
        .expect("selected arm retains reach")
        .objects()
        .iter()
        .filter_map(|object| match object {
            ChangeReachObject::Path { path } => Some(path.as_str()),
            ChangeReachObject::Repository
            | ChangeReachObject::Package { .. }
            | ChangeReachObject::Target { .. }
            | ChangeReachObject::Check { .. } => None,
        })
        .collect()
}

fn packaged_change_report(document: &[u8]) -> ChangeGateReport {
    let report: serde_json::Value =
        serde_json::from_slice(document).expect("worker terminal is a composite pre-commit report");
    let packaged = report
        .get("packaged")
        .expect("default pre-commit report carries the packaged result");
    serde_json::from_value(packaged.clone())
        .expect("composite terminal carries a shaped packaged change-gate report")
}

fn invoke(argv: &[&str], current: &Path) -> Spoken {
    invoke_with_path(argv, current, None)
}

fn invoke_with_path(argv: &[&str], current: &Path, path: Option<&Path>) -> Spoken {
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .current_dir(current)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .env_remove(steer::hook::INBOX);
    if let Some(path) = path {
        command.env(PROCESS_PATH, path);
    }
    let output = command.output().expect("spice gate process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

fn reply<B>(spoken: &Spoken) -> B
where
    B: serde::de::DeserializeOwned + std::fmt::Debug,
{
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1, "one terminated document");
    let outcome: Outcome<B> = wire::decode(&spoken.out).expect("typed gate outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("gate leaf did not reply: {outcome:?}");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply.body
}

fn newline_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

fn report_body(spoken: &Spoken) -> BatteryReport {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1);
    let bytes = spoken.out.strip_suffix(b"\n").unwrap().to_vec();
    BatteryReport::decode(&bytes).expect("complete canonical battery report")
}

fn receipt_body(spoken: &Spoken) -> BatteryReceipt {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let bytes = spoken.out.strip_suffix(b"\n").unwrap().to_vec();
    BatteryReceipt::verify(&bytes).expect("complete canonical battery receipt")
}

fn assert_structured_body(spoken: &Spoken, expected: &impl serde::Serialize) {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let value: serde_json::Value =
        serde_json::from_slice(&spoken.out).expect("structured gate outcome");
    assert_eq!(value["kind"], "reply");
    assert_equivalent_except_rust_elapsed(&value["body"], expected);
}

fn assert_equivalent_except_rust_elapsed<Left, Right>(left: &Left, right: &Right)
where
    Left: serde::Serialize + ?Sized,
    Right: serde::Serialize + ?Sized,
{
    let mut left = serde_json::to_value(left).expect("left body encodes");
    let mut right = serde_json::to_value(right).expect("right body encodes");
    let left_timings = normalize_rust_elapsed(&mut left);
    let right_timings = normalize_rust_elapsed(&mut right);
    assert!(left_timings <= 1);
    assert_eq!(left_timings, right_timings);
    assert_eq!(left, right);
}

fn normalize_rust_elapsed(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().map(normalize_rust_elapsed).sum(),
        serde_json::Value::Object(fields) => {
            if fields.contains_key("run") && fields.contains_key("baseline") {
                let report = fields
                    .get_mut("report")
                    .expect("a receipt carries its report digest");
                assert!(report.is_string(), "a receipt report digest is a string");
                *report =
                    serde_json::Value::String("normalized-run-specific-report-digest".to_owned());
            }
            let is_rust = fields.get("gate_id").and_then(serde_json::Value::as_str)
                == Some(gate::GateId::RustTypecheck.as_str());
            let mut normalized = 0;
            if is_rust && let Some(timing) = fields.get_mut("timing") {
                let elapsed = timing["elapsed_ms"]
                    .as_u64()
                    .expect("rust timing has typed elapsed milliseconds");
                let bound = timing["observation_bound_ms"]
                    .as_u64()
                    .expect("rust timing has a typed observation bound");
                assert!(bound > 0);
                assert!(elapsed <= bound);
                timing["elapsed_ms"] = serde_json::Value::from(0);
                normalized += 1;
            }
            normalized
                + fields
                    .values_mut()
                    .map(normalize_rust_elapsed)
                    .sum::<usize>()
        }
        _ => 0,
    }
}

#[test]
fn ordinary_gate_uses_only_the_captured_index_and_one_selected_commit() {
    let repository = GateRepository::new();

    let text = invoke(&["gate"], repository.path());
    let json = invoke(&["--json", "gate"], repository.path());
    let text_report = report_body(&text);
    assert_structured_body(&json, &text_report);
    assert_eq!(text_report.results().len(), gate::GateId::BATTERY.len());
    assert!(
        text_report.admits(),
        "fixture battery must admit publication: {:?}",
        text_report
            .results()
            .iter()
            .filter(|result| !result.state().admits())
            .map(|result| (result.gate_id(), result.state(), result.reason()))
            .collect::<Vec<_>>()
    );
    assert!(text_report.results().iter().all(|result| {
        !matches!(
            result.state(),
            BatteryState::Unsupported | BatteryState::Unmeasured
        )
    }));

    repository.write_source(2);
    let unstaged = report_body(&invoke(&["gate"], repository.path()));
    assert_eq!(unstaged.context().source(), text_report.context().source());

    repository.add("src/lib.rs");
    let staged = report_body(&invoke(&["gate"], repository.path()));
    assert_ne!(staged.context().source(), text_report.context().source());

    let committed = report_body(&invoke(&["gate", "--at", "HEAD"], repository.path()));
    assert_eq!(committed.context().source(), text_report.context().source());
    assert_eq!(committed.gate_set(), text_report.gate_set());

    let receipt = receipt_body(&invoke(&["gate", "receipt"], repository.path()));
    assert_eq!(receipt.tree(), staged.context().source());
    assert_eq!(receipt.gate_set(), staged.gate_set());
    assert_eq!(
        receipt
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        staged
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>()
    );
}

#[test]
fn camp_receipt_and_kill_are_fresh_direct_leaves_without_checkout_mutation() {
    let repository = GateRepository::new();
    repository.write_camp_lines(100);
    repository.add("gate/src/camp.rs");
    repository.add("gate/src/input.rs");
    repository.add("gate/src/kill.rs");
    repository.add("gate/src/verdict.rs");

    let baseline = fs::read(repository.path().join("baseline.toml")).expect("read baseline");
    let selected_source = fs::read(repository.path().join("src/lib.rs")).expect("read source");
    let git_state = git_stdout(repository.path(), &["status", "--porcelain=v1", "-z"]);
    let index = fs::read(repository.path().join(".git/index")).expect("read captured index");
    assert!(!repository.path().join(".git/.spice-rs").exists());

    let camp_text = invoke(&["gate", "camp"], repository.path());
    let camp_json = invoke(&["gate", "camp", "--json"], repository.path());
    assert_eq!(camp_text.code, Some(0));
    assert!(camp_text.err.is_empty());
    assert_eq!(newline_count(&camp_text.out), 1);
    let camp_text: CampReport = wire::decode(&camp_text.out).expect("camp text is typed body");
    let camp_json = reply::<CampReport>(&camp_json);
    assert_eq!(camp_text, camp_json);
    assert_eq!(camp_json.standing, CampStanding::Camped);

    let receipt_text = invoke(&["gate", "receipt"], repository.path());
    let receipt_json = invoke(&["--json", "gate", "receipt"], repository.path());
    assert_eq!(receipt_text.code, Some(0));
    assert!(receipt_text.err.is_empty());
    let receipt_text = receipt_body(&receipt_text);
    assert_structured_body(&receipt_json, &receipt_text);
    assert_eq!(receipt_text.results().len(), gate::GateId::BATTERY.len());

    let kill = invoke(&["--json", "gate", "kill"], repository.path());
    assert_eq!(kill.code, Some(REFUSAL_EXIT));
    assert!(kill.out.is_empty());
    assert_eq!(newline_count(&kill.err), 1);
    let kill: Outcome<gate::KillReport> =
        wire::decode(&kill.err).expect("unavailable mutation run is typed");
    let Outcome::Refusal(kill) = kill else {
        panic!("fixture mutation tool cannot complete: {kill:?}");
    };
    assert_eq!(kill.reason(), "gate::kill-refused");

    assert_eq!(
        fs::read(repository.path().join("baseline.toml")).expect("reread baseline"),
        baseline
    );
    assert_eq!(
        fs::read(repository.path().join("src/lib.rs")).expect("reread source"),
        selected_source
    );
    let current =
        gate::CurrentState::load(gate::current_state_path(&repository.path().join(".git")))
            .expect("accepted receipt published canonical gate state");
    assert_eq!(
        current.before().version(),
        gate::state::STATE_LEDGER_VERSION
    );
    assert!(
        repository
            .path()
            .join(".git/.spice-rs/data/gate-state.json")
            .is_file(),
        "the accepted battery publishes only its Git-common quality state"
    );
    assert!(
        !repository.path().join(".spice-gate-mutants").exists(),
        "mutation output never reached the caller tree"
    );
    assert_eq!(
        fs::read(repository.path().join(".git/index")).expect("reread captured index"),
        index,
        "gate leaves never changed the captured index"
    );
    assert_eq!(
        git_stdout(repository.path(), &["status", "--porcelain=v1", "-z"]),
        git_state,
        "the caller worktree and captured index stayed unchanged"
    );
}

#[test]
fn repository_selection_and_contract_failures_are_named_and_never_fall_through() {
    let outside = Scratch::new("outside");
    let missing_repository = invoke(&["--json", "gate"], outside.path());
    assert_eq!(missing_repository.code, Some(FAULT_EXIT));
    assert!(missing_repository.out.is_empty());
    let missing_repository: Outcome<serde_json::Value> =
        wire::decode(&missing_repository.err).expect("repository fault is typed");
    let Outcome::Fault(missing_repository) = missing_repository else {
        panic!("missing repository must fault: {missing_repository:?}");
    };
    assert_eq!(missing_repository.reason, "gate::repository-unresolved");

    let repository = GateRepository::new();
    let revision = invoke(
        &["--json", "gate", "--at", "does-not-exist"],
        repository.path(),
    );
    assert_eq!(revision.code, Some(REFUSAL_EXIT));
    let revision: Outcome<serde_json::Value> =
        wire::decode(&revision.err).expect("selection refusal is typed");
    let Outcome::Refusal(revision) = revision else {
        panic!("missing revision must refuse: {revision:?}");
    };
    assert_eq!(revision.reason(), "gate::selection-refused");

    fs::write(repository.path().join("baseline.toml"), "version = 77\n")
        .expect("write malformed current baseline");
    repository.add("baseline.toml");
    let contract = invoke(&["gate", "--json"], repository.path());
    assert_eq!(contract.code, Some(REFUSAL_EXIT));
    let contract: Outcome<serde_json::Value> =
        wire::decode(&contract.err).expect("contract refusal is typed");
    let Outcome::Refusal(contract) = contract else {
        panic!("foreign contract must refuse: {contract:?}");
    };
    assert_eq!(contract.reason(), "gate::contract-refused");
    assert!(
        !contract
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::detail)
            .expect("contract refusal carries detail")
            .contains(PlaceholderReason::NotLanded.as_str())
    );
}

#[test]
fn gate_findings_refuse_unregistered_and_cannot_run_before_board_dispatch() {
    let repository = GateRepository::new();
    let unregistered = invoke(
        &["--json", "gate", "findings", "not-a-gate"],
        repository.path(),
    );
    assert_eq!(unregistered.code, Some(REFUSAL_EXIT));
    assert!(unregistered.out.is_empty());
    let unregistered: Outcome<serde_json::Value> =
        wire::decode(&unregistered.err).expect("unregistered refusal is typed");
    let Outcome::Refusal(unregistered) = unregistered else {
        panic!("unregistered focused instrument must refuse: {unregistered:?}");
    };
    assert_eq!(unregistered.reason(), "gate::instrument-unregistered");
    assert!(matches!(unregistered, Refusal::Invocation { .. }));
    assert!(
        !repository.path().join(".git/.spice-rs").exists(),
        "an unregistered instrument must not reach board state"
    );

    let outside = Scratch::new("focused-cannot-run");
    let cannot_run = invoke(
        &["--json", "gate", "findings", "file-shape"],
        outside.path(),
    );
    assert_eq!(cannot_run.code, Some(REFUSAL_EXIT));
    assert!(cannot_run.out.is_empty());
    let cannot_run: Outcome<serde_json::Value> =
        wire::decode(&cannot_run.err).expect("cannot-run refusal is typed");
    let Outcome::Refusal(cannot_run) = cannot_run else {
        panic!("unavailable focused instrument must refuse: {cannot_run:?}");
    };
    assert_eq!(cannot_run.reason(), "gate::instrument-cannot-run");
    assert!(
        cannot_run
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
            .is_some_and(|observed| observed.contains("gate::repository-unresolved"))
    );
}

//! Real-process regressions for configured pre-commit execution.

use super::*;

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use sill::CancellationToken;

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = env::temp_dir().join(format!(
            "spice-configured-pre-commit-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create configured-command scratch");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn parsed_commands(rows: &[String]) -> Vec<PreCommitStep> {
    let document = format!("[policy]\npre_commit = [\n{},\n]\n", rows.join(",\n"));
    toml::from_str::<spice::config::Root>(&document)
        .unwrap_or_else(|error| panic!("parse configured command fixture {document:?}: {error}"))
        .policy()
        .pre_commit()
        .to_vec()
}

fn row(label: &str, argv: &[&str], scopes: &[&str]) -> String {
    let scopes = if scopes.is_empty() {
        String::new()
    } else {
        format!(", scopes = {{ paths = {} }}", json(&scopes))
    };
    format!(
        "{{ label = {}, run = {}{scopes} }}",
        json(&label),
        json(&argv),
    )
}

fn source() -> Digest {
    Digest::compute(
        wire::Domain::GateTree,
        b"configured pre-commit staged source",
    )
}

fn base_environment() -> Vec<(OsString, OsString)> {
    vec![
        (OsString::from("GIT_CONFIG_KEY_0"), OsString::from("poison")),
        (
            OsString::from("GIT_CONFIG_VALUE_0"),
            OsString::from("poison"),
        ),
        (OsString::from("GIT_DIR"), OsString::from("poison")),
        (OsString::from("PASSTHROUGH"), OsString::from("retained")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ]
}

fn test_bounds(runtime: Duration) -> ExecutionBounds {
    ExecutionBounds {
        runtime,
        output_drain: Duration::from_millis(200),
        reap: Duration::from_millis(200),
        grace: Duration::from_millis(100),
        output_limit: 4 * 1024,
    }
}

#[test]
fn only_unscoped_resolution_source_gate_argv_selects_the_private_exhaustive_leaf() {
    let current = parsed_commands(&[row(
        "Spice source gate",
        &[
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "--package",
            "spice",
            "--bin",
            "spice",
            "--",
            "gate",
            "selection",
        ],
        &[],
    )]);
    let ordinary = effective_argv(&current[0], ExecutionMode::Ordinary);
    assert_eq!(ordinary, current[0].run());
    let resolution = effective_argv(&current[0], ExecutionMode::Resolution);
    assert_eq!(
        resolution,
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "--package",
            "spice",
            "--bin",
            "spice",
            "--",
            super::super::RESOLUTION_SELECTION_WORKER_ARGUMENT,
        ],
    );

    let scoped = parsed_commands(&[row(
        "scoped source gate",
        &["cargo", "run", "--", "gate", "selection"],
        &["src/**"],
    )]);
    assert_eq!(
        effective_argv(&scoped[0], ExecutionMode::Resolution),
        scoped[0].run(),
    );
}

#[test]
fn full_verifier_proves_only_the_exact_unscoped_source_gate_contract() {
    let commands = parsed_commands(&[
        row(
            "label is not authority",
            &[
                "cargo",
                "run",
                "--quiet",
                "--locked",
                "--package",
                "spice",
                "--bin",
                "spice",
                "--",
                "gate",
                "selection",
            ],
            &[],
        ),
        row(
            "different selector",
            &[
                "cargo",
                "run",
                "--quiet",
                "--locked",
                "--package",
                "spice",
                "--bin",
                "spice",
                "--",
                "gate",
                "staging",
            ],
            &[],
        ),
        row(
            "scoped source gate",
            &[
                "cargo",
                "run",
                "--quiet",
                "--locked",
                "--package",
                "spice",
                "--bin",
                "spice",
                "--",
                "gate",
                "selection",
            ],
            &["spice/**"],
        ),
        row("unrelated configured command", &["/bin/true"], &[]),
    ]);

    assert!(full_verifier_proves(&commands[0]));
    assert!(!full_verifier_proves(&commands[1]));
    assert!(!full_verifier_proves(&commands[2]));
    assert!(!full_verifier_proves(&commands[3]));

    let mut remaining = commands;
    assert_eq!(omit_full_verifier_proven(&mut remaining), 1);
    assert_eq!(
        remaining
            .iter()
            .map(PreCommitStep::label)
            .collect::<Vec<_>>(),
        [
            "different selector",
            "scoped source gate",
            "unrelated configured command",
        ],
    );
}

#[test]
fn ordered_steps_receive_exact_scope_source_working_directory_and_sanitized_environment() {
    let scratch = Scratch::new("ordered-environment");
    let log = scratch.root.join("commands.log");
    let script = concat!(
        "printf '%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' ",
        "\"$1\" \"$SPICE_RS_PRE_COMMIT_SOURCE\" \"$SPICE_RS_PRE_COMMIT_PATHS\" ",
        "\"${GIT_DIR-unset}\" \"${PASSTHROUGH-unset}\" \"$PWD\" >> \"$2\"",
    );
    let log_text = log.to_str().expect("fixture log path is UTF-8");
    let commands = parsed_commands(&[
        row(
            "all",
            &[
                "/bin/sh",
                "-c",
                script,
                "configured-fixture",
                "all",
                log_text,
            ],
            &[],
        ),
        row(
            "source",
            &[
                "/bin/sh",
                "-c",
                script,
                "configured-fixture",
                "source",
                log_text,
            ],
            &["src/**"],
        ),
        row(
            "docs",
            &[
                "/bin/sh",
                "-c",
                script,
                "configured-fixture",
                "docs",
                log_text,
            ],
            &["docs/**"],
        ),
        row(
            "skipped",
            &["/bin/sh", "-c", "exit 91", "configured-fixture"],
            &["assets/**"],
        ),
    ]);
    let staged = vec![
        "docs/readme.md".to_owned(),
        "src/lib.rs".to_owned(),
        "tests/integration.rs".to_owned(),
    ];
    let source = source();
    let mut verifications = 0;
    let report = execute_steps(
        &commands,
        &source,
        &staged,
        &scratch.root,
        &base_environment(),
        test_bounds(Duration::from_secs(2)),
        Cancellation::Never,
        ExecutionMode::Ordinary,
        &mut || {
            verifications += 1;
            Ok(())
        },
    )
    .expect("ordered configured commands execute");

    assert!(report.admits(), "{}", report.refusal_evidence());
    assert_eq!(report.source(), &source);
    assert_eq!(report.steps().len(), 4);
    assert_eq!(
        report
            .steps()
            .iter()
            .map(StepReport::label)
            .collect::<Vec<_>>(),
        ["all", "source", "docs", "skipped"],
    );
    assert_eq!(report.steps()[0].selected_paths(), staged);
    assert_eq!(report.steps()[1].selected_paths(), ["src/lib.rs"]);
    assert_eq!(report.steps()[2].selected_paths(), ["docs/readme.md"]);
    assert!(!report.steps()[3].enabled());
    assert!(report.steps()[3].findings().is_empty());
    assert_eq!(
        verifications, 8,
        "every enabled boundary and the terminal verify"
    );

    let lines = fs::read_to_string(log)
        .expect("read configured command log")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let canonical_root = fs::canonicalize(&scratch.root).expect("canonical scratch root");
    assert_eq!(lines.len(), 3);
    for (line, label) in lines.iter().zip(["all", "source", "docs"]) {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields[0], label);
        assert_eq!(fields[1], source.to_string());
        assert_eq!(fields[3], "unset", "Git-local environment leaked: {line}");
        assert_eq!(fields[4], "retained");
        assert_eq!(
            fields[5],
            canonical_root.to_str().expect("scratch is UTF-8")
        );
    }
    let all_paths = json(&staged);
    let source_paths = json(&["src/lib.rs"]);
    let docs_paths = json(&["docs/readme.md"]);
    assert_eq!(lines[0].split('\t').nth(2), Some(all_paths.as_str()));
    assert_eq!(lines[1].split('\t').nth(2), Some(source_paths.as_str()),);
    assert_eq!(lines[2].split('\t').nth(2), Some(docs_paths.as_str()),);
}

#[test]
fn independent_child_failures_are_aggregated_in_authored_order() {
    let scratch = Scratch::new("failure-aggregation");
    let commands = parsed_commands(&[
        row(
            "exit",
            &["/bin/sh", "-c", "printf exit-stderr >&2; exit 7", "fixture"],
            &[],
        ),
        row(
            "signal",
            &["/bin/sh", "-c", "kill -TERM $$", "fixture"],
            &[],
        ),
        row(
            "overflow",
            &["/bin/sh", "-c", "yes overflow | head -c 1024", "fixture"],
            &[],
        ),
        row("spawn", &["/definitely/missing/configured-command"], &[]),
    ]);
    let source = source();
    let mut bounds = test_bounds(Duration::from_secs(2));
    bounds.output_limit = 32;
    let report = execute_steps(
        &commands,
        &source,
        &["src/lib.rs".to_owned()],
        &scratch.root,
        &base_environment(),
        bounds,
        Cancellation::Never,
        ExecutionMode::Ordinary,
        &mut || Ok(()),
    )
    .expect("independent failures remain report evidence");

    assert!(!report.admits());
    assert_eq!(report.steps().len(), 4);
    assert_eq!(
        report.steps()[0].findings()[0].kind(),
        StepFindingKind::Exit
    );
    assert!(
        report.steps()[0].findings()[0]
            .detail()
            .contains("code=Some(7)")
    );
    assert!(
        report.steps()[0].findings()[0]
            .detail()
            .contains("exit-stderr")
    );
    assert_eq!(
        report.steps()[1].findings()[0].kind(),
        StepFindingKind::Signal
    );
    assert_eq!(
        report.steps()[2].findings()[0].kind(),
        StepFindingKind::OutputOverflow,
    );
    assert_eq!(
        report.steps()[3].findings()[0].kind(),
        StepFindingKind::Spawn
    );
    let evidence = report.refusal_evidence();
    for label in ["exit", "signal", "overflow", "spawn"] {
        assert!(
            evidence.contains(&format!("label={}", json(&label))),
            "{evidence}"
        );
    }
    assert!(
        evidence.find("label=\"exit\"") < evidence.find("label=\"spawn\""),
        "failure fold lost authored order: {evidence}",
    );
}

#[test]
fn one_shared_deadline_terminates_and_reaps_a_late_step_process_tree() {
    let scratch = Scratch::new("shared-deadline");
    let pid = scratch.root.join("descendant.pid");
    let pid_text = pid.to_str().expect("pid path is UTF-8");
    let commands = parsed_commands(&[
        row("first", &["/bin/sh", "-c", "sleep 0.10", "fixture"], &[]),
        row(
            "tree",
            &[
                "/bin/sh",
                "-c",
                "sleep 30 & child=$!; printf '%s\\n' \"$child\" > \"$1\"; wait",
                "fixture",
                pid_text,
            ],
            &[],
        ),
    ]);
    let started = Instant::now();
    let report = execute_steps(
        &commands,
        &source(),
        &["src/lib.rs".to_owned()],
        &scratch.root,
        &base_environment(),
        test_bounds(Duration::from_millis(180)),
        Cancellation::Never,
        ExecutionMode::Ordinary,
        &mut || Ok(()),
    )
    .expect("deadline is report evidence");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(report.steps().len(), 2);
    assert!(report.steps()[0].findings().is_empty());
    assert!(
        report.steps()[1]
            .findings()
            .iter()
            .any(|finding| finding.kind() == StepFindingKind::Timeout),
        "{}",
        report.refusal_evidence(),
    );
    let descendant = fs::read_to_string(pid)
        .expect("late step published descendant")
        .trim()
        .parse::<u32>()
        .expect("descendant pid is numeric");
    wait_absent(descendant);
}

#[test]
fn cancellation_terminates_and_reaps_the_configured_process_tree() {
    let scratch = Scratch::new("cancellation");
    let pid = scratch.root.join("descendant.pid");
    let pid_text = pid.to_str().expect("pid path is UTF-8");
    let commands = parsed_commands(&[row(
        "cancel",
        &[
            "/bin/sh",
            "-c",
            "sleep 30 & child=$!; printf '%s\\n' \"$child\" > \"$1\"; wait",
            "fixture",
            pid_text,
        ],
        &[],
    )]);
    let token = CancellationToken::new();
    let canceller = token.clone();
    let pid_for_thread = pid.clone();
    let cancellation = thread::spawn(move || {
        for _ in 0..200 {
            let descendant = fs::read_to_string(&pid_for_thread)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok());
            if let Some(descendant) = descendant {
                canceller.cancel();
                return descendant;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("configured child never published its descendant");
    });
    let report = execute_steps(
        &commands,
        &source(),
        &["src/lib.rs".to_owned()],
        &scratch.root,
        &base_environment(),
        test_bounds(Duration::from_secs(2)),
        Cancellation::Token(token),
        ExecutionMode::Ordinary,
        &mut || Ok(()),
    )
    .expect("cancellation is report evidence");
    let descendant = cancellation.join().expect("canceller completes");

    assert!(
        report.steps()[0]
            .findings()
            .iter()
            .any(|finding| finding.kind() == StepFindingKind::Cancellation),
        "{}",
        report.refusal_evidence(),
    );
    wait_absent(descendant);
}

#[test]
fn authority_movement_between_steps_stops_before_the_next_spawn() {
    let scratch = Scratch::new("movement");
    let log = scratch.root.join("movement.log");
    let log_text = log.to_str().expect("log path is UTF-8");
    let commands = parsed_commands(&[
        row(
            "first",
            &[
                "/bin/sh",
                "-c",
                "printf first >> \"$1\"",
                "fixture",
                log_text,
            ],
            &[],
        ),
        row(
            "second",
            &[
                "/bin/sh",
                "-c",
                "printf second >> \"$1\"",
                "fixture",
                log_text,
            ],
            &[],
        ),
    ]);
    let mut checks = 0;
    let failure = execute_steps(
        &commands,
        &source(),
        &["src/lib.rs".to_owned()],
        &scratch.root,
        &base_environment(),
        test_bounds(Duration::from_secs(2)),
        Cancellation::Never,
        ExecutionMode::Ordinary,
        &mut || {
            checks += 1;
            if checks == 2 {
                Err(ConfiguredFailure::refusal(
                    "gate::pre-commit-staged-source-moved",
                    "fixture movement",
                ))
            } else {
                Ok(())
            }
        },
    )
    .expect_err("movement after the first step must stop the battery");

    assert_eq!(failure.reason(), "gate::pre-commit-staged-source-moved");
    assert!(!failure.is_fault());
    assert_eq!(failure.detail(), "fixture movement");
    assert_eq!(fs::read_to_string(log).expect("read movement log"), "first");
}

#[test]
fn trust_refusal_names_exact_digest_provenance_path_and_command_words() {
    let scratch = Scratch::new("trust-refusal");
    Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&scratch.root)
        .status()
        .expect("launch fixture Git")
        .success()
        .then_some(())
        .expect("initialize fixture repository");
    fs::write(
        scratch.root.join("spice-rust.toml"),
        concat!(
            "[policy]\n",
            "pre_commit = [{ label = 'exact', run = ['/bin/echo', 'one', 'two'] }]\n",
        ),
    )
    .expect("write executable policy fixture");
    let repository = Repository::discover(&scratch.root).expect("discover trust fixture");
    let policy = DispatchPolicy::load(&repository).expect("load executable policy");

    let detail = policy.trust_refusal_detail(Some(TrustState::Pended));
    assert!(
        detail.contains(&policy.surface.digest().to_string()),
        "{detail}"
    );
    assert!(detail.contains("standing=pended"), "{detail}");
    assert!(detail.contains("provenance=tracked"), "{detail}");
    assert!(detail.contains("spice-rust.toml"), "{detail}");
    for word in ["exact", "/bin/echo", "one", "two"] {
        assert!(detail.contains(word), "{detail}");
    }
    assert!(detail.contains("spice trust grant"), "{detail}");
}

#[test]
fn durable_trust_is_read_only_and_required_before_execution() {
    let scratch = Scratch::new("durable-trust");
    Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&scratch.root)
        .status()
        .expect("launch fixture Git")
        .success()
        .then_some(())
        .expect("initialize fixture repository");
    fs::write(
        scratch.root.join("spice-rust.toml"),
        "[policy]\npre_commit = [{ label = 'exact', run = ['/bin/true'] }]\n",
    )
    .expect("write trusted policy fixture");
    let repository = Repository::discover(&scratch.root).expect("discover trust fixture");
    let policy = DispatchPolicy::load(&repository).expect("load executable policy");
    let state = repository.admit_state().expect("admit fixture state paths");
    fs::create_dir_all(state.data()).expect("create fixture authority parent");
    drop(journal::Journal::open(state.journal()).expect("baseline fixture authority"));
    let before = fs::read(state.journal()).expect("read unruled authority");

    let refusal = policy
        .require_approved(&repository)
        .expect_err("an absent trust head must refuse execution");
    assert_eq!(refusal.reason(), "gate::pre-commit-configuration-untrusted");
    assert!(refusal.detail().contains("standing=absent"));
    assert_eq!(
        fs::read(state.journal()).expect("reread unruled authority"),
        before,
        "trust admission must not auto-pend or auto-grant the digest",
    );

    let mut authority = journal::Journal::open(state.journal()).expect("open fixture authority");
    let mut write = journal::JournalWrite::new(
        wire::Actor::Spice,
        "recovery:configured-trust-fixture"
            .parse()
            .expect("fixture origin"),
        "2030-04-12T18:44:01.271Z".parse().expect("fixture instant"),
        wire::Producer::CommandDoor,
    );
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: policy.surface.digest().clone(),
                previous: None,
            },
            journal::FactCause::Root,
        )
        .expect("build fixture trust fact");
    let digest = policy.surface.digest().clone();
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: TrustState::Granted,
    });
    authority.apply(write).expect("commit fixture trust grant");
    drop(authority);
    let granted = fs::read(state.journal()).expect("read granted authority");

    policy
        .require_approved(&repository)
        .expect("the exact durable grant admits execution");
    assert_eq!(
        fs::read(state.journal()).expect("reread granted authority"),
        granted,
        "approval checks are read-only",
    );
}

#[test]
fn missing_trust_authority_refuses_with_the_exact_executable_capability() {
    let scratch = Scratch::new("missing-trust-authority");
    Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&scratch.root)
        .status()
        .expect("launch fixture Git")
        .success()
        .then_some(())
        .expect("initialize fixture repository");
    fs::write(
        scratch.root.join("spice-rust.toml"),
        "[policy]\npre_commit = [{ label = 'exact', run = ['/bin/true'] }]\n",
    )
    .expect("write executable policy fixture");
    let repository = Repository::discover(&scratch.root).expect("discover trust fixture");
    let policy = DispatchPolicy::load(&repository).expect("load executable policy");

    let refusal = policy
        .require_approved(&repository)
        .expect_err("a missing authority cannot silently admit execution");

    assert_eq!(refusal.reason(), "gate::pre-commit-configuration-untrusted");
    for evidence in [
        "standing=absent",
        "provenance=tracked",
        "spice-rust.toml",
        "/bin/true",
        "trust_authority=",
    ] {
        assert!(refusal.detail().contains(evidence), "{}", refusal.detail());
    }
    assert!(
        refusal
            .detail()
            .contains(&policy.surface.digest().to_string()),
        "{}",
        refusal.detail(),
    );
}

#[test]
fn tracked_policy_movement_after_capture_refuses_even_when_bytes_are_restored() {
    let scratch = Scratch::new("policy-movement");
    Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&scratch.root)
        .status()
        .expect("launch fixture Git")
        .success()
        .then_some(())
        .expect("initialize fixture repository");
    let configuration = scratch.root.join("spice-rust.toml");
    let original = "[policy]\npre_commit = [{ label = 'exact', run = ['/bin/true'] }]\n";
    fs::write(&configuration, original).expect("write executable policy fixture");
    let repository = Repository::discover(&scratch.root).expect("discover policy fixture");
    let policy = DispatchPolicy::load(&repository).expect("load executable policy");
    fs::write(
        &configuration,
        "[policy]\npre_commit = [{ label = 'moved', run = ['/bin/false'] }]\n",
    )
    .expect("move policy bytes");
    fs::write(&configuration, original).expect("restore policy bytes");

    let refusal = policy
        .verify_current(&repository)
        .expect_err("capture generation must retain transient policy movement");

    assert_eq!(refusal.reason(), "gate::pre-commit-policy-moved");
    assert!(refusal.detail().contains("spice-rust.toml"));
    assert!(
        refusal
            .detail()
            .contains(&policy.surface.digest().to_string())
    );
}

#[test]
fn recursive_dispatch_is_a_typed_refusal_before_supervision() {
    let outcome: Outcome<()> = recursion_refusal(OsStr::new("fixture-source"));
    let Outcome::Refusal(refusal) = outcome else {
        panic!("recursive dispatch must refuse");
    };
    assert_eq!(refusal.reason(), "gate::pre-commit-recursion");
    assert!(
        refusal
            .diagnostic()
            .and_then(RefusalDiagnostic::observed)
            .is_some_and(|observed| observed.contains("fixture-source")),
    );
}

#[test]
fn dispatch_possession_blocks_recursion_even_without_the_environment_marker() {
    let scratch = Scratch::new("dispatch-possession");
    Command::new("/usr/bin/git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&scratch.root)
        .status()
        .expect("launch fixture Git")
        .success()
        .then_some(())
        .expect("initialize fixture repository");
    let repository = Repository::discover(&scratch.root).expect("discover dispatch fixture");
    let _outer = super::super::DispatchGuard::acquire_in::<()>(&repository)
        .expect("outer commit gate owns dispatch");

    let Outcome::Refusal(refusal) = super::super::DispatchGuard::acquire_in::<()>(&repository)
        .expect_err("nested commit gate must contend")
    else {
        panic!("dispatch contention must be a typed refusal");
    };

    assert_eq!(refusal.reason(), "gate::pre-commit-dispatch-active");
}

fn wait_absent(pid: u32) {
    for _ in 0..100 {
        if !process_exists(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !process_exists(pid),
        "configured execution leaked descendant {pid}"
    );
}

fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero changes no process state and probes this exact pid.
    unsafe { libc::kill(pid, 0) == 0 }
}

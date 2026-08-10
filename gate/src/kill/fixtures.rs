#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    const FAKE_TOOL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"

if [ "${0##*/}" = "cargo-mutants" ]; then
    [ "$1" = "mutants" ] || exit 73
    shift
fi

if [ "$1" = "--version" ]; then
    if [ -f "$FAKE_BAD_VERSION" ]; then
        printf 'cargo-mutants 0.0.0\n'
    else
        printf 'cargo-mutants 27.1.0\n'
    fi
    exit 0
fi

if [ "$1" = "test" ]; then
    if [ -f baseline-fails ]; then
        exit 1
    fi
    if [ -f drift-after-baseline ]; then
        printf 'yes\n' > "$FAKE_DRIFT"
    fi
    exit 0
fi

for argument in "$@"; do
    if [ "$argument" = "--list" ]; then
        if [ -f "$FAKE_DRIFT" ]; then
            printf '[{"name":"Drifted","file":"src/lib.rs"}]\n'
        else
            /bin/cat inventory.json
        fi
        exit 0
    fi
done

if [ "$SPICE_MUTATION_SEED" != "41" ] || [ -z "$SPICE_MUTATION_ATTEMPT" ]; then
    exit 72
fi

if [ -f "$FAKE_TOOL_DRIFT" ]; then
    printf '\n# same-version drift\n' >> "$FAKE_PROGRAM"
    /bin/rm "$FAKE_TOOL_DRIFT"
fi

identity=
output_dir=
previous=
for argument in "$@"; do
    if [ "$previous" = "--re" ]; then
        identity=$argument
    fi
    if [ "$previous" = "--output" ]; then
        output_dir=$argument
    fi
    previous=$argument
done

write_outcome() {
    /bin/mkdir -p "$output_dir/mutants.out"
    printf '%s\n' "$1" > "$output_dir/mutants.out/outcomes.json"
}

caught='{
    "cargo_mutants_version":"27.1.0",
    "total_mutants":1,
    "missed":0,
    "caught":1,
    "timeout":0,
    "unviable":0,
    "outcomes":[{"summary":"CaughtMutant"}]
}'
survived='{
    "cargo_mutants_version":"27.1.0",
    "total_mutants":1,
    "missed":1,
    "caught":0,
    "timeout":0,
    "unviable":0,
    "outcomes":[{"summary":"MissedMutant"}]
}'
timed_out='{
    "cargo_mutants_version":"27.1.0",
    "total_mutants":1,
    "missed":0,
    "caught":0,
    "timeout":1,
    "unviable":0,
    "outcomes":[{"summary":"Timeout"}]
}'
unviable='{
    "cargo_mutants_version":"27.1.0",
    "total_mutants":1,
    "missed":0,
    "caught":0,
    "timeout":0,
    "unviable":1,
    "outcomes":[{"summary":"Unviable"}]
}'

printf 'mutated\n' >> src/lib.rs
case "$identity" in
    *Killed*)
        write_outcome "$caught"
        exit 0
        ;;
    *Survived*)
        write_outcome "$survived"
        exit 2
        ;;
    *Flaky*)
        if [ "$SPICE_MUTATION_ATTEMPT" = "1" ]; then
            write_outcome "$caught"
            exit 0
        fi
        write_outcome "$survived"
        exit 2
        ;;
    *Timeout*)
        write_outcome "$timed_out"
        exit 3
        ;;
    *Incomplete*)
        write_outcome "$caught"
        printf '%s%s' \
            'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' \
            'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'
        exit 0
        ;;
    *Unviable*)
        write_outcome "$unviable"
        exit 0
        ;;
    *NoOutcome*)
        exit 0
        ;;
    *RemoveTool*)
        /bin/mv "$FAKE_PROGRAM" "$FAKE_PROGRAM.removed"
        exit 70
        ;;
    *) exit 70 ;;
esac
"#;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "spice-gate-kill-{label}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("fixture root");
            Self { root }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("fixture parent");
            }
            fs::write(path, contents).expect("fixture file");
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(inventory: &str) -> (Fixture, Fixture, PathBuf) {
        let source = Fixture::new("source");
        source.write("Cargo.toml", "[workspace]\nmembers = []\n");
        source.write("src/lib.rs", "original\n");
        source.write("src/z.rs", "also original\n");
        source.write("inventory.json", inventory);

        let tool = Fixture::new("tool");
        tool.write("cargo", FAKE_TOOL);
        tool.write("cargo-mutants", FAKE_TOOL);
        fs::set_permissions(tool.path("cargo"), fs::Permissions::from_mode(0o755))
            .expect("fixture Cargo executable");
        fs::set_permissions(
            tool.path("cargo-mutants"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("fixture executable");
        let log = tool.path("calls.log");
        (source, tool, log)
    }

    fn gate(tool: &Fixture, log: &Path, targets: &[&str], attempts: usize, capture: usize) -> Kill {
        let environment = Environment::new([
            (OsString::from("FAKE_LOG"), log.as_os_str().to_owned()),
            (
                OsString::from("FAKE_BAD_VERSION"),
                tool.path("bad-version").into_os_string(),
            ),
            (
                OsString::from("FAKE_PROGRAM"),
                tool.path("cargo-mutants").into_os_string(),
            ),
            (
                OsString::from("FAKE_DRIFT"),
                tool.path("drifted").into_os_string(),
            ),
            (
                OsString::from("FAKE_TOOL_DRIFT"),
                tool.path("tool-drift").into_os_string(),
            ),
        ])
        .expect("fixture environment");
        let tool = MutationTool::new(
            tool.path("cargo"),
            tool.path("cargo-mutants"),
            PINNED_CARGO_MUTANTS,
            environment,
        )
        .expect("pinned tool");
        let battery =
            MutationBattery::new(targets.iter().copied(), attempts, 41).expect("fixture battery");
        let bounds = KillBounds::new(
            "gate-kill-fixture",
            Deadlines::new(
                Duration::from_secs(10),
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
            .expect("fixture deadlines"),
            Duration::from_millis(20),
            capture,
        )
        .expect("fixture bounds");
        Kill::new(tool, battery, bounds)
    }

    fn empty_ignore() -> Ignore {
        Ignore::parse("").expect("empty ignore")
    }

    fn expected_inventory(tool: &MutationToolIdentity) -> String {
        format!(
            concat!(
                "tool cargo-mutants 27.1.0\n",
                "tool-declaration {}\n",
                "cargo-executable {}\n",
                "cargo-mutants-executable {}\n",
                "tool-identity {}\n",
                "version-argument mutants\n",
                "version-argument --version\n",
                "baseline-argument test\n",
                "baseline-argument --locked\n",
                "baseline-argument --workspace\n",
                "baseline-argument --all-targets\n",
                "inventory-argument mutants\n",
                "inventory-argument --no-config\n",
                "inventory-argument --no-shuffle\n",
                "inventory-argument --list\n",
                "inventory-argument --json\n",
                "attempt-argument mutants\n",
                "attempt-argument --no-config\n",
                "attempt-argument --no-shuffle\n",
                "attempt-argument --in-place\n",
                "attempt-argument --test-workspace=true\n",
                "attempt-argument --baseline=skip\n",
                "attempt-argument --colors=never\n",
                "attempt-argument --no-times\n",
                "attempt-argument --cargo-test-arg=--locked\n",
                "attempt-argument --cargo-test-arg=--all-targets\n",
                "attempt-argument --timeout=10\n",
                "attempt-argument --output\n",
                "attempt-argument .spice-gate-mutants\n",
                "attempts 2\n",
                "seed 41\n",
                "inventory-argument --file\n",
                "inventory-argument src/lib.rs\n",
                "attempt-argument --file\n",
                "attempt-argument src/lib.rs\n",
                "target src/lib.rs\n",
                "inventory-argument --file\n",
                "inventory-argument src/z.rs\n",
                "attempt-argument --file\n",
                "attempt-argument src/z.rs\n",
                "target src/z.rs\n",
                "mutant Killed\n",
                "mutant-source src/lib.rs\n",
                "mutant-argument --re\n",
                "mutant-argument ^Killed$\n",
                "mutant Survived\n",
                "mutant-source src/z.rs\n",
                "mutant-argument --re\n",
                "mutant-argument ^Survived$\n",
            ),
            tool.declaration(),
            tool.cargo(),
            tool.mutants(),
            tool.identity(),
        )
    }

    #[test]
    fn current_inventory_is_stable_pinned_and_baselined_before_scoring() {
        let (source, tool, log) = fixture(
            r#"[
                {"name":"Survived", "file":"src/z.rs", "extra":"ignored"},
                {"name":"Killed", "file":"src/lib.rs"}
            ]"#,
        );
        let gate = gate(&tool, &log, &["src/z.rs", "src/lib.rs"], 2, 4096);
        let floor = SurvivorRate::new(3, 4).expect("floor");

        let run = gate
            .run(&source.root, &empty_ignore(), Some(floor))
            .unwrap_or_else(|error| {
                panic!(
                    "confirmed battery: {error:?}; calls: {:?}",
                    fs::read_to_string(&log)
                )
            });
        assert_graph_projection(&run, floor);

        assert_eq!(run.inventory().targets(), ["src/lib.rs", "src/z.rs"]);
        assert_eq!(run.inventory().mutants(), ["Killed", "Survived"]);
        assert_eq!(
            run.inventory().render(),
            expected_inventory(run.inventory().tool())
        );

        assert_eq!(
            run.mutants()
                .iter()
                .map(|mutant| mutant.disposition)
                .collect::<Vec<_>>(),
            [
                MutantDisposition::ConfirmedKilled,
                MutantDisposition::ConfirmedSurvived,
            ]
        );
        for mutant in run.mutants() {
            assert_eq!(mutant.attempts.len(), 2);
            assert_eq!(
                mutant
                    .attempts
                    .iter()
                    .map(|attempt| (attempt.number, attempt.seed))
                    .collect::<Vec<_>>(),
                [(1, 41), (2, 41)]
            );
            assert!(
                mutant
                    .attempts
                    .iter()
                    .all(|attempt| attempt.process.outputs_complete())
            );
        }
        let measured = SurvivorRate::new(1, 2).expect("measured rate");
        assert_eq!(run.rate(), Some(measured));
        assert_eq!(run.movement(), MutationMovement::Shrank { floor, measured });
        assert_eq!(run.proposed_floor(), Some(measured));

        let report = run.report();
        assert_eq!(report.inventory().targets(), ["src/lib.rs", "src/z.rs"]);
        assert_eq!(report.inventory().mutants(), ["Killed", "Survived"]);
        assert_eq!(report.inventory().attempts(), 2);
        assert_eq!(report.inventory().seed(), 41);
        assert_eq!(report.rate(), Some(measured.into()));
        assert_eq!(report.proposed_floor(), Some(measured.into()));
        let encoded = report.encode().expect("encode current kill report");
        assert_eq!(
            crate::KillReport::decode(&encoded).expect("decode current kill report"),
            report
        );

        assert_eq!(
            fs::read_to_string(source.path("src/lib.rs")).expect("caller source"),
            "original\n",
            "the tool was handed only private snapshots"
        );
        let calls = fs::read_to_string(log).expect("tool calls");
        let lines: Vec<&str> = calls.lines().collect();
        let baseline = lines
            .iter()
            .position(|line| line.starts_with("test --locked"))
            .expect("baseline call");
        let first_mutant = lines
            .iter()
            .position(|line| line.contains("--re"))
            .expect("mutant call");
        assert!(baseline < first_mutant, "a mutant ran before the baseline");
        assert_eq!(
            lines.iter().filter(|line| line.contains("--list")).count(),
            2,
            "inventory must be re-read after baseline"
        );
    }

    fn assert_graph_projection(run: &KillRun, floor: SurvivorRate) {
        let graph_evidence = crate::test_evidence::MutationExecution::from_kill(run)
            .expect("completed KillRun projects typed graph evidence");
        assert_eq!(graph_evidence.source(), run.source());
        assert_eq!(graph_evidence.floor(), Some(floor));
        assert_eq!(
            graph_evidence
                .mutants()
                .iter()
                .map(crate::test_evidence::MutationObservation::source)
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/z.rs"]
        );
        let tests = crate::CanonicalTestExecution::new(
            run.source().clone(),
            vec!["src/lib.rs".to_owned()],
            vec!["gate::lib".to_owned()],
            Vec::new(),
            crate::CoverageCompleteness::unknown(
                "fixture has no authoritative per-test coverage denominator",
            )
            .expect("coverage reason"),
        )
        .expect("canonical test evidence");
        let report = crate::TestEvidenceGraph::from_kill(tests, run, Vec::new())
            .expect("one graph joins the canonical kill run")
            .report();
        assert_eq!(report.source(), run.source());
        assert_eq!(report.mutation().mutants(), graph_evidence.mutants());
        let foreign_tests = crate::CanonicalTestExecution::new(
            Digest::compute(Domain::GateTree, b"foreign workspace-test source"),
            vec!["src/lib.rs".to_owned()],
            vec!["gate::lib".to_owned()],
            Vec::new(),
            crate::CoverageCompleteness::unknown(
                "fixture has no authoritative per-test coverage denominator",
            )
            .expect("coverage reason"),
        )
        .expect("foreign canonical test evidence");
        let mismatch = crate::TestEvidenceGraph::from_kill(foreign_tests, run, Vec::new())
            .expect_err("a canonical KillRun cannot cross selected-source identities");
        assert!(mismatch.detail().contains("different selected sources"));
    }

    #[test]
    fn multi_target_inventory_requires_one_declared_structured_source_per_mutant() {
        for inventory in [
            r#"[{"name":"MissingSource"}]"#,
            r#"[{"name":"OutsideSource","file":"src/outside.rs"}]"#,
        ] {
            let (source, tool, log) = fixture(inventory);
            let error = gate(&tool, &log, &["src/lib.rs", "src/z.rs"], 1, 4096)
                .run(&source.root, &empty_ignore(), None)
                .expect_err("multi-target inventory source must be exact and declared");
            assert!(matches!(error, KillError::InvalidMutantSource { .. }));
        }
    }

    #[test]
    fn a_non_green_baseline_refuses_before_the_first_mutant() {
        let (source, tool, log) = fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        source.write("baseline-fails", "yes\n");
        let gate = gate(&tool, &log, &["src/lib.rs"], 2, 4096);

        let refused = gate
            .run(&source.root, &empty_ignore(), None)
            .expect_err("failed baseline refuses");

        assert!(matches!(refused, KillError::BaselineNotGreen(_)));
        let calls = fs::read_to_string(log).expect("tool calls");
        assert!(!calls.lines().any(|line| line.contains("--re")));
    }

    #[test]
    fn immutable_selected_sources_are_writable_only_inside_private_attempts() {
        let (source, tool, log) = fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        fs::set_permissions(source.path("src/lib.rs"), fs::Permissions::from_mode(0o400))
            .expect("make selected source immutable");

        let run = gate(&tool, &log, &["src/lib.rs"], 1, 4096)
            .run(&source.root, &empty_ignore(), None)
            .expect("private attempt makes only its own target writable");

        assert_eq!(
            run.mutants()[0].disposition,
            MutantDisposition::ConfirmedKilled
        );
        assert_eq!(
            fs::metadata(source.path("src/lib.rs"))
                .expect("caller target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            fs::read_to_string(source.path("src/lib.rs")).expect("caller target bytes"),
            "original\n"
        );
    }

    #[test]
    fn flaky_and_timed_out_mutants_keep_ordered_evidence_without_moving_the_floor() {
        let (source, tool, log) = fixture(
            r#"[{"name":"Timeout","file":"src/lib.rs"},{"name":"Flaky","file":"src/lib.rs"}]"#,
        );
        let gate = gate(&tool, &log, &["src/lib.rs"], 2, 4096);
        let floor = SurvivorRate::new(1, 3).expect("floor");

        let run = gate
            .run(&source.root, &empty_ignore(), Some(floor))
            .expect("unconfirmed attempts remain evidence");

        assert_eq!(run.inventory().mutants(), ["Flaky", "Timeout"]);
        assert_eq!(run.rate(), None);
        assert_eq!(
            run.movement(),
            MutationMovement::Unconfirmed {
                confirmed: 0,
                total: 2
            }
        );
        assert_eq!(run.proposed_floor(), None);
        assert_eq!(
            run.mutants()[0]
                .attempts
                .iter()
                .map(|attempt| attempt.kind.clone())
                .collect::<Vec<_>>(),
            [AttemptKind::Killed, AttemptKind::Survived]
        );
        assert_eq!(
            run.mutants()[1]
                .attempts
                .iter()
                .map(|attempt| attempt.kind.clone())
                .collect::<Vec<_>>(),
            [AttemptKind::TimedOut, AttemptKind::TimedOut]
        );
        assert!(
            run.mutants()
                .iter()
                .all(|mutant| mutant.disposition == MutantDisposition::Unconfirmed)
        );
    }

    #[test]
    fn unviable_and_incomplete_attempts_never_masquerade_as_kills() {
        let (source, tool, log) = fixture(r#"[{"name":"Incomplete","file":"src/lib.rs"}]"#);
        let incomplete_gate = gate(&tool, &log, &["src/lib.rs"], 2, 64);

        let run = incomplete_gate
            .run(&source.root, &empty_ignore(), None)
            .expect("incomplete attempt remains evidence");

        let mutant = &run.mutants()[0];
        assert_eq!(mutant.disposition, MutantDisposition::Unconfirmed);
        assert!(
            mutant
                .attempts
                .iter()
                .all(|attempt| attempt.kind == AttemptKind::Undecodable)
        );
        assert!(mutant.attempts.iter().all(|attempt| {
            attempt
                .process
                .stdout
                .as_ref()
                .is_some_and(|stdout| stdout.truncated && stdout.complete)
        }));

        let (source, tool, log) = fixture(r#"[{"name":"Unviable","file":"src/lib.rs"}]"#);
        let run = gate(&tool, &log, &["src/lib.rs"], 2, 4096)
            .run(&source.root, &empty_ignore(), None)
            .expect("unviable attempt remains evidence");
        let mutant = &run.mutants()[0];
        assert_eq!(mutant.disposition, MutantDisposition::Unconfirmed);
        assert!(
            mutant
                .attempts
                .iter()
                .all(|attempt| attempt.kind == AttemptKind::Undecodable)
        );
        assert!(
            mutant
                .attempts
                .iter()
                .all(|attempt| matches!(attempt.outcome, OutcomeEvidence::Present(_)))
        );
    }

    #[test]
    fn mutation_tool_removal_refuses_the_completed_run_identity() {
        let (source, tool, log) = fixture(r#"[{"name":"RemoveTool","file":"src/lib.rs"}]"#);
        let refused = gate(&tool, &log, &["src/lib.rs"], 2, 4096)
            .run(&source.root, &empty_ignore(), None)
            .expect_err("removed executable cannot retain its earlier identity");
        assert!(matches!(
            refused,
            KillError::ToolRead { role: "cargo-mutants", source, .. }
                if source.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn a_stale_caller_outcome_cannot_masquerade_as_current_evidence() {
        let (source, tool, log) = fixture(r#"[{"name":"NoOutcome","file":"src/lib.rs"}]"#);
        source.write(
            ".spice-gate-mutants/mutants.out/outcomes.json",
            concat!(
                "{\"cargo_mutants_version\":\"27.1.0\",\"total_mutants\":1,",
                "\"missed\":0,\"caught\":1,\"timeout\":0,\"unviable\":0,",
                "\"outcomes\":[{\"summary\":\"CaughtMutant\"}]}\n"
            ),
        );
        let run = gate(&tool, &log, &["src/lib.rs"], 2, 4096)
            .run(&source.root, &empty_ignore(), None)
            .expect("stale outcome is discarded in each private snapshot");

        let mutant = &run.mutants()[0];
        assert_eq!(mutant.disposition, MutantDisposition::Unconfirmed);
        assert!(mutant.attempts.iter().all(|attempt| {
            attempt.kind == AttemptKind::Undecodable && attempt.outcome == OutcomeEvidence::Absent
        }));
        assert_eq!(run.rate(), None);
        assert_eq!(run.proposed_floor(), None);
        assert!(
            source
                .path(".spice-gate-mutants/mutants.out/outcomes.json")
                .is_file()
        );
    }

    #[test]
    fn a_confirmed_survivor_regresses_survivor_debt_and_cannot_raise_the_floor() {
        let (source, tool, log) = fixture(r#"[{"name":"Survived","file":"src/lib.rs"}]"#);
        let gate = gate(&tool, &log, &["src/lib.rs"], 2, 4096);
        let floor = SurvivorRate::new(0, 1).expect("zero floor");

        let run = gate
            .run(&source.root, &empty_ignore(), Some(floor))
            .expect("confirmed survivor scores");

        let measured = SurvivorRate::new(1, 1).expect("survivor rate");
        assert_eq!(run.rate(), Some(measured));
        assert_eq!(
            run.movement(),
            MutationMovement::Regressed { floor, measured }
        );
        assert_eq!(run.proposed_floor(), None);
    }

    #[test]
    fn empty_drifted_and_unavailable_batteries_refuse_before_scoring() {
        let (empty_source, empty_tool, empty_log) = fixture("[]");
        let empty = gate(&empty_tool, &empty_log, &["src/lib.rs"], 1, 4096)
            .run(&empty_source.root, &empty_ignore(), None)
            .expect_err("empty inventory refuses");
        assert!(
            matches!(empty, KillError::EmptyInventory),
            "unexpected empty-inventory refusal: {empty:?}"
        );

        let (drift_source, drift_tool, drift_log) =
            fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        drift_source.write("drift-after-baseline", "yes\n");
        let drift = gate(&drift_tool, &drift_log, &["src/lib.rs"], 1, 4096)
            .run(&drift_source.root, &empty_ignore(), None)
            .expect_err("drifted inventory refuses");
        assert!(matches!(drift, KillError::InventoryDrift { .. }));

        let (missing_mutants_source, missing_mutants_tool, missing_mutants_log) =
            fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        let environment = Environment::empty();
        let tool = MutationTool::new(
            missing_mutants_tool.path("cargo"),
            missing_mutants_tool.path("absent-cargo-mutants"),
            PINNED_CARGO_MUTANTS,
            environment,
        )
        .expect("tool declaration");
        let battery = MutationBattery::new(["src/lib.rs"], 1, 1).expect("battery");
        let bounds = KillBounds::new(
            "unavailable-fixture",
            Deadlines::new(
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("deadlines"),
            Duration::from_millis(10),
            1024,
        )
        .expect("bounds");
        let unavailable = Kill::new(tool, battery, bounds)
            .run(&missing_mutants_source.root, &empty_ignore(), None)
            .expect_err("unavailable tool refuses");
        assert!(
            matches!(
                &unavailable,
                KillError::ToolRead { role: "cargo-mutants", source, .. }
                    if source.kind() == std::io::ErrorKind::NotFound
            ),
            "unexpected missing cargo-mutants refusal: {unavailable:?}"
        );
        assert!(
            !missing_mutants_log.exists(),
            "missing cargo-mutants reached process attestation"
        );

        let (missing_cargo_source, missing_cargo_tool, missing_cargo_log) =
            fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        let mut missing_cargo = gate(
            &missing_cargo_tool,
            &missing_cargo_log,
            &["src/lib.rs"],
            1,
            4096,
        );
        missing_cargo.tool.cargo_program = missing_cargo_tool.path("absent-cargo");
        let refused = missing_cargo
            .run(&missing_cargo_source.root, &empty_ignore(), None)
            .expect_err("missing direct Cargo refuses before attestation");
        assert!(matches!(
            refused,
            KillError::ToolRead { role: "cargo", ref source, .. }
                if source.kind() == std::io::ErrorKind::NotFound
        ));
        assert!(
            !missing_cargo_log.exists(),
            "missing Cargo reached process attestation"
        );

        let (drifted_source, drifted_tool, drifted_log) =
            fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        drifted_tool.write("bad-version", "yes\n");
        let drifted = gate(&drifted_tool, &drifted_log, &["src/lib.rs"], 1, 4096)
            .run(&drifted_source.root, &empty_ignore(), None)
            .expect_err("drifted tool refuses");
        assert!(matches!(drifted, KillError::ToolDrift { .. }));
    }

    #[test]
    fn executable_content_identities_are_stable_and_same_version_bytes_move_them() {
        let (source, tool, log) = fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        let gate = gate(&tool, &log, &["src/lib.rs"], 1, 4096);
        let first = gate
            .run(&source.root, &empty_ignore(), None)
            .expect("first identified run");
        let second = gate
            .run(&source.root, &empty_ignore(), None)
            .expect("unchanged identified run");
        assert_eq!(first.inventory().tool(), second.inventory().tool());

        tool.write(
            "cargo-mutants",
            &(FAKE_TOOL.to_owned() + "\n# same semantic version, different bytes\n"),
        );
        let changed = gate
            .run(&source.root, &empty_ignore(), None)
            .expect("same-version replacement remains operational");
        assert_eq!(
            first.inventory().tool().expected(),
            changed.inventory().tool().expected()
        );
        assert_ne!(
            first.inventory().tool().mutants(),
            changed.inventory().tool().mutants()
        );
        assert_ne!(
            first.inventory().tool().identity(),
            changed.inventory().tool().identity()
        );
        assert_ne!(first.report().render(), changed.report().render());
    }

    #[test]
    fn executable_byte_drift_during_a_run_refuses_the_run() {
        let (source, tool, log) = fixture(r#"[{"name":"Killed","file":"src/lib.rs"}]"#);
        tool.write("tool-drift", "armed\n");
        let refused = gate(&tool, &log, &["src/lib.rs"], 1, 4096)
            .run(&source.root, &empty_ignore(), None)
            .expect_err("mid-run executable drift refuses");
        assert!(matches!(refused, KillError::ToolIdentityDrift { .. }));
    }

    #[test]
    fn rates_are_exact_and_configuration_refuses_ambiguous_batteries() {
        assert_eq!(
            SurvivorRate::new(2, 4).expect("reduced rate"),
            SurvivorRate::new(1, 2).expect("canonical rate")
        );
        assert!(matches!(
            SurvivorRate::new(2, 1),
            Err(RateError::SurvivorsExceedConfirmed { .. })
        ));
        assert!(matches!(
            MutationBattery::new(Vec::<String>::new(), 1, 0),
            Err(RateError::EmptyTargets)
        ));
        assert!(matches!(
            MutationBattery::new(["src/lib.rs"], 0, 0),
            Err(RateError::ZeroAttempts)
        ));
        assert!(matches!(
            MutationBattery::new(["src/lib.rs", "src/lib.rs"], 1, 0),
            Err(RateError::DuplicateTarget { .. })
        ));
        assert!(matches!(
            MutationTool::new(
                "cargo",
                "/absolute/cargo-mutants",
                PINNED_CARGO_MUTANTS,
                Environment::empty()
            ),
            Err(RateError::NonAbsoluteProgram { .. })
        ));
        assert!(matches!(
            MutationTool::new(
                "/absolute/cargo",
                "/absolute/cargo-mutants",
                "foreign\nidentity",
                Environment::empty()
            ),
            Err(RateError::InvalidToolIdentity { .. })
        ));
        for key in [
            CARGO_ENVIRONMENT,
            CARGO_BUILD_DIRECTORY_ENVIRONMENT,
            CARGO_TARGET_DIRECTORY_ENVIRONMENT,
        ] {
            let reserved =
                Environment::new([(key, "/alternate")]).expect("one reserved environment entry");
            assert!(matches!(
                MutationTool::new(
                    "/absolute/cargo",
                    "/absolute/cargo-mutants",
                    PINNED_CARGO_MUTANTS,
                    reserved
                ),
                Err(RateError::ReservedEnvironment { key: ref observed }) if observed == key
            ));
        }

        let tool = MutationTool::new(
            "/absolute/cargo",
            "/absolute/cargo-mutants",
            PINNED_CARGO_MUTANTS,
            Environment::empty(),
        )
        .expect("empty base environment is valid");
        let root = Path::new("/private/selected-snapshot");
        let environment = tool
            .environment(root, 41, Some(2))
            .expect("gate-owned environment resolves");
        let value = |key: &str| {
            environment
                .entries()
                .iter()
                .find_map(|(name, value)| (name == key).then_some(value.as_os_str()))
                .unwrap_or_else(|| panic!("missing gate-owned environment {key}"))
        };
        assert_eq!(
            value(CARGO_BUILD_DIRECTORY_ENVIRONMENT),
            root.join(MUTATION_BUILD_DIRECTORY).as_os_str()
        );
        assert_eq!(
            value(CARGO_TARGET_DIRECTORY_ENVIRONMENT),
            root.join(MUTATION_TARGET_DIRECTORY).as_os_str()
        );
    }

    #[test]
    fn exact_mutant_filter_escapes_every_regex_metacharacter() {
        assert_eq!(
            exact_regex("a[b].*(c)+d?{e}|f^g$h\\i"),
            r"^a\[b\]\.\*\(c\)\+d\?\{e\}\|f\^g\$h\\i$"
        );
    }

    #[test]
    fn sill_deadline_completion_is_a_timed_out_attempt_without_an_artifact() {
        let process = ProcessEvidence {
            completion: ProcessCompletion::TimedOut(None),
            stdout: Some(StreamEvidence {
                bytes: Vec::new(),
                truncated: false,
                complete: true,
            }),
            stderr: Some(StreamEvidence {
                bytes: Vec::new(),
                truncated: false,
                complete: true,
            }),
        };
        assert_eq!(
            decode_attempt(&process, &OutcomeEvidence::Absent),
            AttemptKind::TimedOut
        );
    }

    #[test]
    fn fake_mutation_tool_is_valid_posix_shell() {
        let (_source, tool, _log) = fixture("[]");
        let status = std::process::Command::new("/bin/sh")
            .arg("-n")
            .arg(tool.path("cargo"))
            .status()
            .expect("validate fixture shell");
        assert!(status.success());
    }
}

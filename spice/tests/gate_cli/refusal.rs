use super::*;

#[test]
fn every_gate_leaf_uses_one_rust_vector_without_interpreter_or_taskwarrior_processes() {
    // GOV-45 / CASE 250: ordinary `gate` and `gate pre-commit` remain direct
    // observation/enforcement paths; only the explicit `gate findings` leaf
    // enters board dispatch.
    let repository = oversized_refusal_repository();
    let (tripwires, marker) = process_tripwires();
    let report = report_body(&invoke_with_path(
        &["gate"],
        repository.path(),
        Some(tripwires.path()),
    ));
    let fold = assert_oversized_json_refusal(&repository, &tripwires, &report);
    assert_oversized_text_refusal(&repository, &tripwires, &fold);
    assert_canonical_receipt(&repository, &tripwires, &report);
    assert!(
        !marker.exists(),
        "a forbidden interpreter or Taskwarrior process crossed the Rust rim: {}",
        marker.display()
    );
}

fn oversized_refusal_repository() -> GateRepository {
    let repository = GateRepository::new();
    repository.write_markdown_case_mismatches(512);
    repository.add("docs");
    git(
        repository.path(),
        &["commit", "-qm", "Plant oversized canonical gate evidence"],
    );
    repository.write_source(2_000);
    repository.add("src/lib.rs");
    repository
}

fn process_tripwires() -> (Scratch, PathBuf) {
    let tripwires = Scratch::new("tripwires");
    let marker = tripwires.path().join("invoked");
    for program in ["python", "python3", "taskwarrior", "task"] {
        let executable = tripwires.path().join(program);
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}' >> '{}'\nexit 97\n",
                program,
                marker.display()
            ),
        )
        .expect("write process tripwire");
        let mut permissions = fs::metadata(&executable)
            .expect("read process tripwire mode")
            .permissions();
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make process tripwire executable");
    }
    std::os::unix::fs::symlink(env!("CARGO"), tripwires.path().join("cargo"))
        .expect("make canonical test runner Cargo available inside the process tripwire path");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO")).with_file_name("rustc"),
        tripwires.path().join("rustc"),
    )
    .expect("make Cargo's exact Rust compiler available inside the process tripwire path");
    for program in ["cc", "xcrun"] {
        let executable = Path::new("/usr/bin").join(program);
        if executable.exists() {
            std::os::unix::fs::symlink(&executable, tripwires.path().join(program)).unwrap_or_else(
                |error| panic!("make {program} available to the selected target: {error}"),
            );
        }
    }
    (tripwires, marker)
}

fn assert_oversized_json_refusal(
    repository: &GateRepository,
    tripwires: &Scratch,
    report: &BatteryReport,
) -> String {
    let canonical = report.encode().expect("canonical battery report encodes");
    assert!(
        canonical.len() > gate::BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "fixture must expose the former full-report refusal flood: {} bytes",
        canonical.len(),
    );
    let pre_commit = invoke_with_path(
        &["--json", "gate", "pre-commit"],
        repository.path(),
        Some(tripwires.path()),
    );
    assert_eq!(pre_commit.code, Some(REFUSAL_EXIT));
    assert!(pre_commit.out.is_empty());
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&pre_commit.err).expect("typed pre-commit refusal");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("non-admitting tripwire vector must refuse: {outcome:?}");
    };
    assert_eq!(
        refusal.reason(),
        "gate::change-gate-regressed",
        "the fixture must reach the selected-change refusal mapper: {refusal:?}",
    );
    let Refusal::Evidence {
        evidence, summary, ..
    } = &refusal
    else {
        panic!("pre-commit gate report must use evidence refusal: {refusal:?}")
    };
    assert!(summary.contains("explicit full verifier"));
    let (_, change_report) = assert_concise_pre_commit_refusal(&refusal);
    let fold = change_report.refusal_fold();
    assert!(evidence.starts_with(fold.as_str()));
    assert!(
        evidence
            .lines()
            .any(|line| line.starts_with("full-report: \"/")),
        "the refusal does not name its absolute canonical report artifact: {evidence}",
    );
    assert!(evidence.contains("gate=file-shape state=failed"));
    assert!(
        change_report.policy().len() < gate::GateId::BATTERY.len(),
        "a known Rust change unexpectedly executed the complete battery",
    );
    let admitted = change_report
        .policy()
        .iter()
        .find(|result| result.admits())
        .expect("the oversized fixture retains at least one admitting arm");
    assert!(
        !evidence.contains(&format!("gate={} state=", admitted.gate_id())),
        "passing-arm evidence leaked into the refusal: {evidence}",
    );
    assert!(
        pre_commit.err.len() <= gate::BATTERY_REFUSAL_FOLD_MAX_BYTES * 2 + 2_048,
        "typed JSON refusal is not structurally bounded: {} bytes",
        pre_commit.err.len(),
    );
    fold
}

fn assert_oversized_text_refusal(repository: &GateRepository, tripwires: &Scratch, fold: &str) {
    let pre_commit = invoke_with_path(
        &["gate", "pre-commit"],
        repository.path(),
        Some(tripwires.path()),
    );
    assert_eq!(pre_commit.code, Some(REFUSAL_EXIT));
    assert!(pre_commit.out.is_empty());
    let text = std::str::from_utf8(&pre_commit.err).expect("text refusal is UTF-8");
    let (report_path, _, report) = read_report_artifact(text);
    assert_eq!(report.refusal_fold(), fold);
    assert!(text.starts_with(fold));
    let encoded_path = serde_json::to_string(
        report_path
            .to_str()
            .expect("text report artifact path remains UTF-8"),
    )
    .expect("text report artifact path encodes");
    assert!(
        pre_commit.err.len() <= gate::BATTERY_REFUSAL_FOLD_MAX_BYTES + encoded_path.len() + 2_048,
        "text refusal is not structurally bounded: {} bytes",
        pre_commit.err.len(),
    );
}

fn assert_canonical_receipt(
    repository: &GateRepository,
    tripwires: &Scratch,
    report: &BatteryReport,
) {
    let receipt = receipt_body(&invoke_with_path(
        &["gate", "receipt"],
        repository.path(),
        Some(tripwires.path()),
    ));
    assert_eq!(receipt.tree(), report.context().source());
    assert_eq!(
        receipt.manifest(),
        &Digest::compute(
            Domain::GateBatteryManifest,
            gate::CANONICAL_BATTERY_MANIFEST.as_bytes(),
        )
    );
    assert_eq!(receipt.gate_set(), report.gate_set());
    assert_equivalent_except_rust_elapsed(receipt.results(), report.results());
    assert_eq!(gate::GateId::BATTERY.len(), 27);
    assert_eq!(report.results().len(), 27);
    assert_eq!(
        report
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        gate::GateId::BATTERY
    );
}

#[test]
fn pre_commit_refusal_publishes_one_failed_result_behind_a_private_report_path() {
    let repository = GateRepository::new();
    repository.write_source(2_000);
    repository.add("src/lib.rs");
    let pre_commit = invoke(&["--json", "gate", "pre-commit"], repository.path());
    assert_eq!(pre_commit.code, Some(REFUSAL_EXIT));
    assert!(pre_commit.out.is_empty());
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&pre_commit.err).expect("typed pre-commit refusal");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("one typed regression must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "gate::change-gate-regressed");
    assert_eq!(
        assert_concise_pre_commit_refusal(&refusal).0,
        1,
        "the focused artifact fixture must contain exactly one refusing result",
    );
}

fn assert_concise_pre_commit_refusal(refusal: &Refusal) -> (usize, ChangeGateReport) {
    let Refusal::Evidence {
        evidence, summary, ..
    } = refusal
    else {
        panic!("pre-commit gate report must use evidence refusal: {refusal:?}")
    };
    assert!(summary.contains("explicit full verifier"));
    let (report_path, report_bytes, pre_commit_report) = read_report_artifact(evidence);
    let encoded_report_path = serde_json::to_string(
        report_path
            .to_str()
            .expect("report artifact path remains UTF-8"),
    )
    .expect("report artifact path encodes");
    assert_eq!(
        evidence.as_str(),
        format!(
            "{}full-report: {}\n",
            pre_commit_report.refusal_fold().as_str(),
            encoded_report_path
        ),
        "the terminal refusal contains only the failure fold and audit path",
    );
    assert!(
        evidence.len()
            <= BATTERY_REFUSAL_FOLD_MAX_BYTES
                + encoded_report_path.len()
                + "full-report: ".len()
                + 1,
        "the refusal stays within the fold bound plus its artifact line: {} bytes",
        evidence.len(),
    );
    assert!(
        evidence.len() < report_bytes.len(),
        "many admitted metrics must remain in the artifact instead of bloating the refusal: refusal={} report={}",
        evidence.len(),
        report_bytes.len(),
    );
    (
        assert_refusing_results_are_focused(evidence, &pre_commit_report),
        pre_commit_report,
    )
}

fn read_report_artifact(evidence: &str) -> (PathBuf, Vec<u8>, ChangeGateReport) {
    let report_path = evidence
        .lines()
        .find_map(|line| line.strip_prefix("full-report: "))
        .map(|encoded| {
            serde_json::from_str::<String>(encoded)
                .map(PathBuf::from)
                .expect("pre-commit report path is one JSON string")
        })
        .expect("pre-commit refusal names its full report artifact");
    assert!(report_path.is_absolute(), "artifact path is absolute");
    assert_eq!(
        fs::metadata(&report_path)
            .expect("report artifact metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "the audit report is private",
    );
    let report_bytes = fs::read(&report_path).expect("read full report artifact");
    let pre_commit_report = ChangeGateReport::decode(&report_bytes).unwrap_or_else(|error| {
        panic!(
            "pre-commit artifact carries a canonical report: {error}; path={}",
            report_path.display()
        )
    });
    fs::remove_file(&report_path).expect("remove test report artifact");
    (report_path, report_bytes, pre_commit_report)
}

fn assert_refusing_results_are_focused(evidence: &str, report: &ChangeGateReport) -> usize {
    let admitted = report
        .policy()
        .iter()
        .filter(|result| result.admits())
        .count();
    let refusing = report.policy().len() - admitted;
    assert!(
        admitted > refusing,
        "the concise-refusal fixture needs many more passing arms than refusals; admitted={admitted} refusing={refusing}"
    );
    for report in report.policy() {
        let result = report.result();
        let arm = format!("gate={} state=", report.gate_id());
        if report.admits() {
            assert!(
                !evidence.contains(&arm),
                "passing arm {} leaked into the terminal refusal",
                report.gate_id()
            );
        } else {
            assert!(
                evidence.contains(&arm),
                "failing arm {} is absent from the terminal refusal",
                report.gate_id()
            );
            assert!(
                evidence.contains(&format!("reason={}", result.reason())),
                "typed summary for {} is absent from the terminal refusal",
                report.gate_id()
            );
        }
    }
    refusing
}

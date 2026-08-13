use super::*;
use gate::metrics::{EvidenceState, MetricId, MetricKey};
use gate::{ArmEvidence, BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryArmResult, FocusedArmReport};

#[test]
fn verifier_process_projects_one_bounded_actionable_file_shape_refusal() {
    let fixture = Fixture::new();
    let programs = install_full_verifier_programs(&fixture);

    install_file_shape_overrun(&fixture);
    let selected_result = run_selected_gate(
        &fixture,
        &programs.directory,
        &programs.cargo_log,
        &programs.browser_log,
    );
    let refused_result = run_refusing_selected_gate(&fixture, &programs.directory);

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs.directory)
        .output()
        .expect("launch the real verifier process");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("battery refused publication"),
        "{}",
        streams(&output)
    );
    assert!(
        !programs.forbidden_marker.exists(),
        "a forbidden program crossed the verifier rim: {}",
        programs.forbidden_marker.display()
    );
    assert_full_verifier_action_trace(&programs, &fixture.repository, &output);

    assert_bounded_refusal(&output, &refused_result);
    assert_eq!(selected_result.gate_id(), GateId::EnvNameLedger);
}

fn run_selected_gate(
    fixture: &Fixture,
    programs: &Path,
    cargo_log: &Path,
    browser_log: &Path,
) -> BatteryArmResult {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "env-name-ledger"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch one real gate arm");
    assert!(output.status.success(), "{}", streams(&output));
    let result: BatteryArmResult =
        serde_json::from_slice(&output.stdout).expect("selected arm emits one JSON result");
    assert_eq!(result.gate_id(), GateId::EnvNameLedger);
    assert!(!cargo_log.exists(), "selected arm launched a Cargo stage");
    assert!(
        !browser_log.exists(),
        "selected arm launched the browser stage"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("[verify]"));
    result
}

fn run_refusing_selected_gate(fixture: &Fixture, programs: &Path) -> BatteryArmResult {
    let human = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "file-shape"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch the refusing file-shape gate arm");
    assert_eq!(human.status.code(), Some(1), "{}", streams(&human));
    let diagnostic = String::from_utf8_lossy(&human.stdout);
    assert!(
        diagnostic.starts_with("arm position=18 gate=file-shape state=failed\n"),
        "the human refusal does not lead with the selected arm: {}",
        streams(&human)
    );
    assert!(
        diagnostic
            .contains("finding gate=file-shape metric=file.lines key=path=\"src/overrun.rs\""),
        "the bounded refusal does not name its actionable breach: {}",
        streams(&human)
    );
    assert!(
        diagnostic.contains("full-evidence: cargo xtask gate --json file-shape"),
        "the bounded refusal does not name its machine recovery path: {}",
        streams(&human)
    );
    assert!(
        diagnostic.ends_with("omitted_arms=0 omitted_findings=0\n"),
        "the bounded refusal does not close with exact omission counts: {}",
        streams(&human)
    );
    assert!(human.stdout.len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES);
    assert!(!diagnostic.contains("[verify]"));

    let machine = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "--json", "file-shape"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch the canonical file-shape gate report");
    assert_eq!(machine.status.code(), Some(1), "{}", streams(&machine));
    let canonical = machine
        .stdout
        .strip_suffix(b"\n")
        .expect("machine focused report carries one terminal newline");
    let report = FocusedArmReport::decode(canonical)
        .unwrap_or_else(|error| panic!("machine path emits a canonical focused report: {error}"));
    assert_eq!(report.gate_id(), GateId::FileShape);
    assert!(!report.admits());
    assert!(machine.stdout.len() > human.stdout.len());
    report.into_result()
}

fn assert_bounded_refusal(output: &std::process::Output, selected: &BatteryArmResult) {
    let stdout = String::from_utf8(output.stdout.clone()).expect("verifier stdout is UTF-8");
    let first_arm = format!(
        "arm position={} gate={} state={}",
        selected.position(),
        selected.gate_id(),
        selected.state().as_str(),
    );
    let fold_start = stdout
        .find(&first_arm)
        .unwrap_or_else(|| panic!("verifier emits the refusal fold: {}", streams(output)));
    let summary_start = stdout
        .rfind("battery refusal summary:")
        .expect("verifier closes its refusal fold with a summary");
    let summary_end = stdout[summary_start..]
        .find('\n')
        .map_or(stdout.len(), |end| summary_start + end + 1);
    let fold = &stdout.as_bytes()[fold_start..summary_end];
    assert!(
        fold.len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "battery diagnostic is {} bytes, limit is {}",
        fold.len(),
        BATTERY_REFUSAL_FOLD_MAX_BYTES
    );
    assert!(
        stdout.contains("full-evidence: cargo xtask gate --json file-shape"),
        "refusal omits the exact selected-arm recovery command: {}",
        streams(output)
    );
    assert!(
        !stdout.contains("[verify] contract landing"),
        "a refused battery crossed contract landing: {}",
        streams(output)
    );

    let ArmEvidence::Metrics { report } = selected.evidence() else {
        panic!("the selected file-shape arm carries structural metrics")
    };
    let metric = report
        .metrics()
        .iter()
        .find(|metric| {
            metric.metric() == MetricId::FileLines
                && metric.key() == &MetricKey::path("src/overrun.rs").expect("fixture overrun path")
        })
        .expect("selected file-shape evidence names the overrun path");
    let EvidenceState::Measured { measurement } = metric.state() else {
        panic!("the overrun is a measured file-lines value")
    };
    let latch = measurement
        .latch()
        .expect("file lines carry their active latch");
    let exact_finding = format!(
        concat!(
            "finding gate=file-shape metric=file.lines key=path=\"src/overrun.rs\" ",
            "state=failed unit=source-lines value={} base_limit={} ",
            "configured_ratchet_limit={} ratchet_limit={} active_limit={} ",
            "latch_transition={}"
        ),
        measurement.value(),
        latch.base_limit(),
        latch.configured_ratchet_limit(),
        latch.ratchet_limit(),
        latch.active_limit(),
        latch.transition(),
    );
    assert!(
        stdout.contains(&exact_finding),
        "refusal omits the exact actionable file-shape finding {exact_finding:?}: {}",
        streams(output)
    );

    let reports = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| BatteryReport::decode(line).ok())
        .collect::<Vec<_>>();
    assert!(
        reports.is_empty(),
        "a refusing human verifier emitted a complete canonical BatteryReport: {}",
        streams(output)
    );
    let lifecycle = String::from_utf8_lossy(&output.stdout);
    for identity in [
        "canonical Rust gate battery",
        "npm run test:canvas (BROWSER-1kKSWhZk guard)",
    ] {
        assert_eq!(
            lifecycle
                .lines()
                .filter(|line| {
                    line.contains("stage=start") && line.contains(&format!("identity={identity:?}"))
                })
                .count(),
            1,
            "action {identity:?} did not start exactly once: {}",
            streams(output)
        );
    }
}

fn install_file_shape_overrun(fixture: &Fixture) {
    const OVERLONG_LINES: usize = 4_000;
    let path = fixture.repository.join("src/overrun.rs");
    fs::write(&path, "// measured overrun\n".repeat(OVERLONG_LINES))
        .expect("write real file-shape overrun");
    git(&fixture.repository, &["add", "src/overrun.rs"]);
}

#[test]
fn command_help_enumerates_every_single_gate_arm_spelling() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("help")
        .output()
        .expect("launch xtask help");

    assert!(output.status.success(), "{}", streams(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(concat!(
            "cargo xtask <help|verify [--package PACKAGE]|",
            "verify-action <selection|battery>|gate [--json] ARM|"
        )),
        "{}",
        streams(&output)
    );
    assert!(stdout.contains("Neither is a completion verifier"));
    for gate_id in GateId::BATTERY {
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("  {}", gate_id.as_str())),
            "help omits {}:\n{}",
            gate_id.as_str(),
            streams(&output)
        );
    }
}

#[test]
fn unknown_gate_arm_points_to_the_canonical_inventory() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "not-a-gate"])
        .output()
        .expect("launch unknown gate arm");

    assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("run `cargo xtask help` for accepted gate arms"),
        "{}",
        streams(&output)
    );
}

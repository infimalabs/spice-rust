use gate::metrics::{EvidenceState, MetricId, MetricKey};
use gate::{ArmEvidence, BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryReport};

use super::*;

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
    assert!(
        !programs.cargo_log.exists(),
        "a structural refusal launched Cargo: {}",
        streams(&output)
    );
    assert!(
        !programs.browser_log.exists(),
        "a structural refusal launched the browser: {}",
        streams(&output)
    );

    assert_bounded_refusal(&output, &refused_result);
    assert_eq!(selected_result.gate_id(), GateId::EnvNameLedger);
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
        stdout.contains("full-evidence: cargo xtask gate file-shape"),
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
    assert_eq!(
        lifecycle
            .lines()
            .filter(|line| {
                line.contains("stage=setup identity=")
                    && line.contains("selected file-shape and routine-complexity gate preflight")
            })
            .count(),
        1,
        "structural preflight did not start exactly once: {}",
        streams(output)
    );
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
            0,
            "structural refusal launched action {identity:?}: {}",
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

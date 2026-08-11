use super::*;

use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct FocusedDispatch {
    pub(super) body: GateFindings,
    pub(super) findings: Vec<FindingRow>,
}

pub(super) fn focused_dispatch(
    root: &Path,
    marker: &str,
    finding_names: &[&str],
) -> FocusedDispatch {
    let report = focused_file_shape_report(root, marker, finding_names);
    let findings = report
        .findings()
        .into_iter()
        .map(FindingRow::from)
        .collect::<Vec<_>>();
    assert_eq!(findings.len(), finding_names.len());
    assert_eq!(report.admits(), findings.is_empty());
    let (report_digest, evidence) = crate::gate_command::focused_report_evidence(&report)
        .unwrap_or_else(|outcome| {
            panic!("focused report did not cross the CLI evidence encoder: {outcome:?}")
        });
    assert_eq!(
        gate::FocusedArmReport::decode(evidence.as_bytes())
            .expect("CLI evidence is a canonical focused report"),
        report
    );
    FocusedDispatch {
        body: GateFindings {
            report: report_digest,
            evidence,
        },
        findings,
    }
}

fn focused_file_shape_report(
    root: &Path,
    marker: &str,
    finding_names: &[&str],
) -> gate::FocusedArmReport {
    static NEXT_REPORT: AtomicU64 = AtomicU64::new(1);
    let repository = root.join(format!(
        "focused-report-{}",
        NEXT_REPORT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repository.join("src")).expect("focused repository directories exist");
    gate_git(&repository, &["init", "-q"]);
    gate_git(
        &repository,
        &["config", "user.email", "gate-findings@example.invalid"],
    );
    gate_git(
        &repository,
        &["config", "user.name", "Gate Findings Fixture"],
    );

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice crate has a workspace parent");
    for document in ["spice-rust.toml", "baseline.toml"] {
        fs::copy(workspace.join(document), repository.join(document))
            .unwrap_or_else(|error| panic!("copy focused fixture {document}: {error}"));
    }
    fs::write(repository.join("src/base.rs"), "pub fn base() {}\n")
        .expect("write focused fixture baseline source");
    gate_git(&repository, &["add", "--all"]);
    gate_git(&repository, &["commit", "-qm", "focused report baseline"]);

    let marker_path = "src/report_marker.rs";
    fs::write(
        repository.join(marker_path),
        format!("pub const REPORT_MARKER: &str = {marker:?};\n"),
    )
    .expect("write focused report marker");
    let mut changes = vec![marker_path.to_owned()];
    let oversized = "//\n".repeat(2_000);
    for name in finding_names {
        let path = format!("src/{name}.rs");
        fs::write(repository.join(&path), &oversized)
            .unwrap_or_else(|error| panic!("write focused finding {path}: {error}"));
        changes.push(path);
    }
    changes.sort();
    gate_git(&repository, &["add", "--all"]);

    let rim = gate::RimCapture::new(
        repository.clone(),
        Some(OsString::from("/usr/bin:/bin")),
        changes,
    )
    .expect("focused fixture rim is absolute");
    let actor = gate::ActorCapture::Acting {
        actor: "gate-findings-fixture".to_owned(),
        bound_claims: Vec::new(),
    };
    let mut capture = gate::BatteryCapture::capture_arm(
        &repository,
        &gate::Selector::staged(),
        gate::ClaimCapture::Idle,
        actor,
        rim,
        gate::HygieneConfiguration::current(),
        gate::GateId::FileShape,
    )
    .expect("capture production focused file-shape report");
    let report = capture
        .execute_arm_report(gate::GateId::FileShape)
        .expect("execute production focused file-shape report");
    assert_eq!(report.gate_id(), gate::GateId::FileShape);
    report
}

fn gate_git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .output()
        .unwrap_or_else(|error| panic!("run focused fixture Git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "focused fixture Git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

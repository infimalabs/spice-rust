use super::*;

use std::ffi::OsString;
use std::process::Command;

pub(super) struct FocusedDispatch {
    pub(super) body: GateFindings,
    pub(super) findings: Vec<FindingRow>,
    pub(super) repository: Repository,
}

pub(super) fn focused_dispatch(
    root: &Path,
    marker: &str,
    finding_names: &[&str],
) -> FocusedDispatch {
    focused_dispatch_in(root, "focused-repository", marker, finding_names)
}

pub(super) fn focused_dispatch_in(
    root: &Path,
    repository_name: &str,
    marker: &str,
    finding_names: &[&str],
) -> FocusedDispatch {
    let repository = focused_repository(root, repository_name);
    gate_git(repository.worktree_root(), &["reset", "--hard", "HEAD"]);
    gate_git(repository.worktree_root(), &["clean", "-fd"]);
    for entry in fs::read_dir(repository.worktree_root().join("src"))
        .expect("read focused fixture source inventory")
    {
        let path = entry.expect("read focused fixture source entry").path();
        if path.file_name().is_some_and(|name| name != "base.rs") {
            fs::remove_file(&path).expect("remove prior focused fixture source");
        }
    }
    let marker_path = "src/report_marker.rs";
    fs::write(
        repository.worktree_root().join(marker_path),
        format!("pub const REPORT_MARKER: &str = {marker:?};\n"),
    )
    .expect("write focused report marker");
    let mut changes = vec![marker_path.to_owned()];
    let oversized = "//\n".repeat(2_000);
    for name in finding_names {
        let path = format!("src/{name}.rs");
        fs::write(repository.worktree_root().join(&path), &oversized)
            .unwrap_or_else(|error| panic!("write focused finding {path}: {error}"));
        changes.push(path);
    }
    changes.sort();
    gate_git(repository.worktree_root(), &["add", "--all"]);
    focused_dispatch_for_selector(
        repository,
        gate::Selector::staged(),
        &changes,
        finding_names,
    )
}

pub(super) fn focused_revision_dispatch(
    repository: &Repository,
    revision: &str,
) -> FocusedDispatch {
    focused_dispatch_for_selector(
        repository.clone(),
        gate::Selector::revision(revision),
        &[],
        &[],
    )
}

pub(super) fn commit_focused(dispatch: &FocusedDispatch, message: &str) {
    gate_git(
        dispatch.repository.worktree_root(),
        &["commit", "-qm", message],
    );
}

pub(super) fn unrelated_focused_revision(
    dispatch: &FocusedDispatch,
    finding_names: &[&str],
) -> FocusedDispatch {
    let tree = gate_git_output(dispatch.repository.worktree_root(), &["write-tree"]);
    let revision = gate_git_output(
        dispatch.repository.worktree_root(),
        &["commit-tree", &tree, "-m", "unrelated focused root"],
    );
    focused_dispatch_for_selector(
        dispatch.repository.clone(),
        gate::Selector::revision(revision),
        &[],
        finding_names,
    )
}

#[test]
fn lineage_refuses_descendant_revision_with_the_same_tree_as_recovery() {
    let (root, bus, _, _) = fixture("gate-findings-same-tree-recovery", 1);
    let finding = focused_dispatch(&root, "same-tree-finding", &["same-tree"]);
    commit_focused(&finding, "commit unchanged standing finding tree");
    let clean_revision = spice::gate::capture_gate_finding_anchor(
        &finding.repository,
        &gate::Selector::revision("HEAD"),
    )
    .expect("capture the committed descendant")
    .bind(finding.body.source.tree.clone());
    assert_ne!(finding.body.source.anchor, clean_revision.anchor);
    assert_eq!(finding.body.source.tree, clean_revision.tree);

    let error = spice::gate::require_gate_finding_source_forward(
        &finding.repository,
        &finding.body.source,
        &clean_revision,
        true,
    )
    .expect_err("unchanged source cannot prove recovery through an empty descendant");

    assert_eq!(error.reason(), "gate::finding-recovery-unproven");
    clean(&root, bus);
}

fn focused_dispatch_for_selector(
    repository: Repository,
    selector: gate::Selector,
    changes: &[String],
    finding_names: &[&str],
) -> FocusedDispatch {
    let before = spice::gate::capture_gate_finding_anchor(&repository, &selector)
        .expect("capture focused fixture source before report");
    let report = focused_file_shape_report(&repository, selector.clone(), changes);
    let after = spice::gate::capture_gate_finding_anchor(&repository, &selector)
        .expect("capture focused fixture source after report");
    assert_eq!(before, after, "focused fixture source moved during report");
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
            source: before.bind(report.context().source().clone()),
        },
        findings,
        repository,
    }
}

fn focused_file_shape_report(
    repository: &Repository,
    selector: gate::Selector,
    changes: &[String],
) -> gate::FocusedArmReport {
    let rim = gate::RimCapture::new(
        repository.worktree_root().to_path_buf(),
        Some(OsString::from("/usr/bin:/bin")),
        changes.to_vec(),
    )
    .expect("focused fixture rim is absolute");
    let actor = gate::ActorCapture::Acting {
        actor: "gate-findings-fixture".to_owned(),
        bound_claims: Vec::new(),
    };
    let mut capture = gate::BatteryCapture::capture_arm(
        repository.worktree_root(),
        &selector,
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

fn focused_repository(root: &Path, name: &str) -> Repository {
    let repository = root.join(name);
    if !repository.join(".git").exists() {
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
    }
    Repository::discover(&repository).expect("discover focused fixture repository")
}

fn gate_git(root: &Path, arguments: &[&str]) {
    let _ = gate_git_output(root, arguments);
}

fn gate_git_output(root: &Path, arguments: &[&str]) -> String {
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
    String::from_utf8(output.stdout)
        .expect("focused fixture Git output is UTF-8")
        .trim()
        .to_owned()
}

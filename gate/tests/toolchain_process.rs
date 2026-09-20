use gate::{ArmOutcome, GateId, measure_toolchain};

mod support;

use support::git_commit;
use support::toolchain::{acquired, arm, canvas_nonzero_directory, scanning_repository};

#[test]
fn canvas_public_acquisition_retains_unparsed_nonzero_without_running_trunk() {
    let repository = scanning_repository(
        "toolchain-canvas-nonzero",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write("app/index.html", "<main></main>\n");
    git_commit(&repository, "canvas entrypoint");
    let stubs = canvas_nonzero_directory("toolchain-canvas-tools");
    let observations = acquired(
        &repository,
        Some(stubs.root.clone().into_os_string()),
        &["app/index.html"],
    );

    let observation = arm(&observations, GateId::CanvasWebTypecheck);
    assert_eq!(observation.outcome, ArmOutcome::NonzeroAnalysis);
    assert_eq!(observation.exit_code, Some(17));
    assert!(observation.findings.is_empty());
    assert!(observation.reason.contains("compiling fixture"));
    assert!(
        observation
            .reason
            .contains("error: lint configuration rejected")
    );
    assert!(observation.reason.contains("invalid lint level"));
    assert!(
        !stubs.path("trunk-analysis-ran").exists(),
        "trunk ran after the cargo analysis refused"
    );

    measure_toolchain(observations).expect("canvas nonzero observation is admitted");
    repository.remove();
    stubs.remove();
}

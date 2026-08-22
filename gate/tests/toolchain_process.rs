use gate::{ArmOutcome, GateId, measure_toolchain};

mod support;

use support::git_commit;
use support::toolchain::{
    UNPARSABLE_INCLUDED, acquired, arm, canvas_nonzero_directory, including_repository,
    scanning_repository, toolchain_directory,
};

/// A refusal the formatter cannot explain is retained with the code it gave.
///
/// [`measure_toolchain`] admits an exit code only beside `NonzeroAnalysis` and
/// demands one there, so an analysis that reports the outcome without the code
/// is refused at admission rather than at the call. The captured tree here
/// carries a source the formatter cannot parse, which it reports on its error
/// stream, so it exits nonzero having named no differing file at all.
///
/// That unreadable source is reached only by `include!`, which is the failure
/// mode the formatter gap was made of: the arm concluding a tree is formatted
/// because nothing told it otherwise. Findings alone would read exactly like a
/// canonical tree, so the refusal is what has to survive rather than the count.
///
/// The code ships twice, once as a field and once inside the sentence, and both
/// spellings are held to each other. A reader shown a sentence naming one code
/// beside a field carrying another has been handed an invented number.
#[test]
fn a_formatter_refusal_naming_no_file_carries_the_code_it_refused_with() {
    let repository = including_repository("toolchain-unparsable", UNPARSABLE_INCLUDED);
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );

    let observation = arm(&observations, GateId::Formatters);
    assert_eq!(
        observation.outcome,
        ArmOutcome::NonzeroAnalysis,
        "the formatter arm reported this over a file it could not read: {}",
        observation.reason
    );
    let code = observation
        .exit_code
        .expect("a refusal carries the code it refused with");
    assert_ne!(code, 0, "zero is not a code that explains a refusal");
    assert!(
        observation
            .reason
            .contains(&format!("refused with {code} ")),
        "the field carries {code} and the sentence names another: {}",
        observation.reason
    );
    assert!(
        observation.findings.is_empty(),
        "an unexplained refusal invented {:?}",
        observation.findings
    );

    measure_toolchain(observations).expect("an explained refusal is admitted");
}

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
    assert!(
        observation
            .reason
            .contains("cargo canvas-wasm-typecheck refused with 17")
    );
    assert!(
        !stubs.path("trunk-analysis-ran").exists(),
        "trunk ran after the cargo analysis refused"
    );

    measure_toolchain(observations).expect("canvas nonzero observation is admitted");
    repository.remove();
    stubs.remove();
}

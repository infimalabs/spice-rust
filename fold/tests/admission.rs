#[allow(dead_code)]
#[path = "../build/admission.rs"]
mod admission;
#[allow(dead_code)]
#[path = "../build/registry.rs"]
mod registry;

use admission::{AnswerContent, FoldRegistration, RingRegistration, Source};
use wire::FactKind;

const WITNESS_SOURCE: &[Source] = &[Source::Head {
    variant: "Witness",
    name: "witness",
}];
const UNKNOWN_FACT_FAMILY_SOURCE: &[Source] = &[Source::Facts {
    plane: "Fixture",
    pattern: "fixture::*",
}];
const UNKNOWN_HEAD_SOURCE: &[Source] = &[Source::Head {
    variant: "Fixture",
    name: "fixture",
}];
const UNKNOWN_FOLD_SOURCE: &[Source] = &[Source::Fold {
    variant: "FixtureFold",
    name: "fixture::fold",
}];
const LATER_FOLD_SOURCE: &[Source] = &[Source::Fold {
    variant: "ConsentQueue",
    name: "consent::queue",
}];
const UNKNOWN_RING_SOURCE: &[Source] = &[Source::Ring {
    variant: "FixtureRing",
    name: "fixture::ring",
}];
const UNKNOWN_CONFIG_SOURCE: &[Source] = &[Source::Config {
    variant: "Fixture",
    name: "fixture",
}];
const UNKNOWN_TAPE_SOURCE: &[Source] = &[Source::Tape {
    variant: "Fixture",
    name: "fixture",
}];
const UNKNOWN_COMMAND_RESULTS_SOURCE: &[Source] = &[Source::CommandResults("fixture-results")];

const UNKNOWN_FACT_FAMILY_CONTENT: &[AnswerContent] = &[AnswerContent::Facts {
    plane: "Fixture",
    pattern: "fixture::*",
}];
const UNKNOWN_HEAD_CONTENT: &[AnswerContent] = &[AnswerContent::Head {
    variant: "Fixture",
    name: "fixture",
}];
const UNSUPPLIED_FACT_CONTENT: &[AnswerContent] = &[AnswerContent::Fact(FactKind::TaskAdded)];
const UNSUPPLIED_FACTS_CONTENT: &[AnswerContent] = &[AnswerContent::Facts {
    plane: "Task",
    pattern: "task::*",
}];
const UNSUPPLIED_HEAD_CONTENT: &[AnswerContent] = &[AnswerContent::Head {
    variant: "Task",
    name: "task",
}];

fn folds() -> Vec<FoldRegistration> {
    registry::FOLDS.to_vec()
}

fn rings() -> Vec<RingRegistration> {
    registry::RINGS.to_vec()
}

fn fold_mut<'a>(folds: &'a mut [FoldRegistration], name: &str) -> &'a mut FoldRegistration {
    folds
        .iter_mut()
        .find(|fold| fold.name == name)
        .expect("fixture mutates a shipped registry row")
}

fn assert_refusal(folds: &[FoldRegistration], rings: &[RingRegistration], expected: &str) {
    let refusal = admission::validate(FactKind::ALL, folds, rings)
        .expect_err("mutated registry must be refused");
    assert_eq!(refusal.to_string(), expected);
}

#[test]
fn the_shipped_registry_is_admitted() {
    admission::validate(FactKind::ALL, registry::FOLDS, registry::RINGS).expect("shipped registry");
}

#[test]
fn a_duplicate_contract_row_is_refused_by_name() {
    let mut folds = folds();
    folds.push(folds[0]);

    assert_refusal(
        &folds,
        &rings(),
        "fold registry row steer::inbox is declared more than once",
    );
}

#[test]
fn a_missing_contract_row_is_refused_by_name() {
    let mut folds = folds();
    folds.remove(0);

    assert_refusal(
        &folds,
        &rings(),
        "fold registry is missing contract row steer::inbox",
    );
}

#[test]
fn an_unknown_contract_row_is_refused_by_name() {
    let mut folds = folds();
    let mut extra = folds[0];
    extra.variant = "FixtureFold";
    extra.name = "fixture::fold";
    folds.push(extra);

    assert_refusal(
        &folds,
        &rings(),
        "fold registry contains unknown contract row fixture::fold",
    );
}

#[test]
fn a_fold_without_an_answer_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").answer = "";

    assert_refusal(&folds, &rings(), "fold steer::inbox has no declared answer");
}

#[test]
fn a_fold_without_answer_content_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = &[];

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer declares no content",
    );
}

#[test]
fn an_unknown_beyond_policy_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").beyond = "Fixture";

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox has unknown beyond policy Fixture",
    );
}

#[test]
fn a_bounded_ring_with_recovery_is_refused_by_name() {
    let folds = folds();
    let mut rings = rings();
    rings[0].recovery = "Replay";

    assert_refusal(
        &folds,
        &rings,
        "bounded ring bus::refusals must declare recovery None",
    );
}

#[test]
fn an_unknown_fact_family_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_FACT_FAMILY_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown fact family Fixture => fixture::*",
    );
}

#[test]
fn an_unknown_head_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_HEAD_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown head source fixture",
    );
}

#[test]
fn an_unknown_fold_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_FOLD_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown source fold fixture::fold",
    );
}

#[test]
fn a_fold_must_follow_its_source_fold() {
    let mut folds = folds();
    let dependent = fold_mut(&mut folds, "steer::inbox");
    let mut sources = dependent.sources.to_vec();
    sources.extend_from_slice(LATER_FOLD_SOURCE);
    dependent.sources = Box::leak(sources.into_boxed_slice());

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox must follow source fold consent::queue",
    );

    let dependent = folds
        .iter()
        .position(|fold| fold.name == "steer::inbox")
        .expect("fixture contains the dependent fold");
    let source = folds
        .iter()
        .position(|fold| fold.name == "consent::queue")
        .expect("fixture contains the source fold");
    folds.swap(dependent, source);

    admission::validate(FactKind::ALL, &folds, &rings())
        .expect("dependency-safe order must be admitted");
}

#[test]
fn an_unknown_ring_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_RING_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown source ring fixture::ring",
    );
}

#[test]
fn an_unknown_config_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_CONFIG_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown config source fixture",
    );
}

#[test]
fn an_unknown_tape_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_TAPE_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown tape source fixture",
    );
}

#[test]
fn an_unknown_command_result_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").sources = UNKNOWN_COMMAND_RESULTS_SOURCE;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox names unknown command-result source fixture-results",
    );
}

#[test]
fn an_unconsumed_typed_fact_is_refused_with_its_producer() {
    let mut folds = folds();
    for fold in &mut folds {
        fold.sources = WITNESS_SOURCE;
    }

    assert_refusal(
        &folds,
        &rings(),
        "fact kind task::added from producer command-door has no consuming fold",
    );
}

#[test]
fn answer_content_from_an_unknown_fact_family_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = UNKNOWN_FACT_FAMILY_CONTENT;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer names fact content fixture::*, but no admitted kind can supply it",
    );
}

#[test]
fn answer_content_from_an_unknown_head_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = UNKNOWN_HEAD_CONTENT;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer names head content fixture, but no admitted head can supply it",
    );
}

#[test]
fn exact_fact_content_without_a_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = UNSUPPLIED_FACT_CONTENT;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer content task::added is not supplied by its admitted sources",
    );
}

#[test]
fn fact_family_content_without_a_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = UNSUPPLIED_FACTS_CONTENT;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer content task::* is not supplied by its admitted sources",
    );
}

#[test]
fn head_content_without_a_source_is_refused_by_name() {
    let mut folds = folds();
    fold_mut(&mut folds, "steer::inbox").content = UNSUPPLIED_HEAD_CONTENT;

    assert_refusal(
        &folds,
        &rings(),
        "fold steer::inbox answer content task is not supplied by its admitted sources",
    );
}

/// Memberships a plane keeps although the glob it is admitted under cannot
/// spell them, each named with the reason it outlives its own spelling.
///
/// Named one at a time rather than permitted as a class: each is a decision
/// someone made, and removing one should cost the same argument that making it
/// did. A fourth arriving here is the review, not the lint.
const SPELLING_EXCEPTIONS: &[(&str, &str, &str)] = &[
    (
        "Steering",
        "taste::reminded",
        "a reminder is a live directive and the taste evidence justifying it in one \
         row, so the steering reduction has to see a kind spelled under taste",
    ),
    (
        "Lane",
        "lineage::configured",
        "lineage spells its own stem and is admitted under no glob at all, and the \
         lane reduction is what rebuilds occupant lineage",
    ),
    (
        "Lane",
        "lineage::arranged",
        "the other half of that lineage pair, claimed for the same reason",
    ),
];

/// Plane membership is written twice, and the two copies are one set.
///
/// [`FACT_PLANES`](admission::FACT_PLANES) declares each plane as a wire
/// spelling, so a kind is admitted as consumed the moment it is declared under
/// an existing stem. `FactPlane::contains` writes that same membership a second
/// time as hand-edited predicates, and until this law nothing made the two
/// agree. They had already drifted by four kinds -- `task::deleted`,
/// `lane::arranged`, `lane::imported` and `lane::killed` -- every one of them
/// admitted by its glob while no predicate claimed it, which is what left them
/// unreadable from any plane and so absent from every reduction that
/// enumerates one.
///
/// This is the per-plane half of that guard and is strictly stronger than the
/// kind-first law in `fold/src/tests.rs`, which sees only the total orphan. A
/// kind claimed by the wrong plane, or dropped from the right one while some
/// other plane still claims it, is invisible there and reds here.
#[test]
fn each_plane_claims_exactly_the_kinds_its_admitted_glob_spells() {
    let mut diverged: Vec<(&str, &str)> = Vec::new();
    for plane in fold::FactPlane::ALL {
        let declared = format!("{plane:?}");
        let (name, glob) = admission::FACT_PLANES
            .iter()
            .find(|(name, _)| *name == declared)
            .unwrap_or_else(|| panic!("plane {declared} is admitted under no wire glob"));
        for kind in FactKind::ALL {
            if admission::pattern_matches(glob, kind.as_str()) != plane.contains(*kind) {
                diverged.push((name, kind.as_str()));
            }
        }
    }
    diverged.sort_unstable();

    let mut permitted: Vec<(&str, &str)> = SPELLING_EXCEPTIONS
        .iter()
        .map(|(plane, kind, _)| (*plane, *kind))
        .collect();
    permitted.sort_unstable();

    assert_eq!(
        diverged, permitted,
        "every plane claims exactly the kinds its glob spells, apart from the \
         memberships named in SPELLING_EXCEPTIONS",
    );
}

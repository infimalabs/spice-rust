#[allow(dead_code)]
#[path = "../build/admission.rs"]
mod admission;
#[allow(dead_code)]
#[path = "../build/registry.rs"]
mod registry;

use admission::{FoldRegistration, Source};
use wire::FactKind;

const UNKNOWN_FACT_FAMILY_SOURCE: &[Source] = &[Source::Facts {
    plane: "Fixture",
    pattern: "fixture::*",
}];

fn folds() -> Vec<FoldRegistration> {
    registry::FOLDS.to_vec()
}

fn fold_mut(folds: &mut [FoldRegistration]) -> &mut FoldRegistration {
    folds.first_mut().expect("fixture has task::effort")
}

fn assert_refusal(folds: &[FoldRegistration], expected: &str) {
    let refusal =
        admission::validate(FactKind::ALL, folds).expect_err("mutated registry must be refused");
    assert_eq!(refusal.to_string(), expected);
}

#[test]
fn the_shipped_registry_is_admitted() {
    admission::validate(FactKind::ALL, registry::FOLDS).expect("shipped registry");
    admission::validate_registry_source(FactKind::ALL, include_str!("../src/registry.rs"))
        .expect("shared registry source");
    let refusal = admission::validate_registry_source(FactKind::ALL, "fold_registry! {}\n")
        .expect_err("changed registry source must be refused");
    assert_eq!(
        refusal.to_string(),
        "fold registry source differs from its build admission contract",
    );
}

#[test]
fn duplicate_missing_and_unknown_rows_are_refused_by_name() {
    let mut duplicate = folds();
    duplicate.push(duplicate[0]);
    assert_refusal(
        &duplicate,
        "fold registry row task::effort is declared more than once",
    );

    assert_refusal(&[], "fold registry is missing contract row task::effort");

    let mut unknown = folds();
    let mut extra = unknown[0];
    extra.variant = "FixtureFold";
    extra.name = "fixture::fold";
    unknown.push(extra);
    assert_refusal(
        &unknown,
        "fold registry contains unknown contract row fixture::fold",
    );
}

#[test]
fn incomplete_operational_contracts_are_refused() {
    let mut no_answer = folds();
    fold_mut(&mut no_answer).answer = "";
    assert_refusal(&no_answer, "fold task::effort has no declared answer");

    let mut no_sources = folds();
    fold_mut(&mut no_sources).sources = &[];
    assert_refusal(&no_sources, "fold task::effort declares no sources");

    let mut unknown_policy = folds();
    fold_mut(&mut unknown_policy).beyond = "Wait";
    assert_refusal(
        &unknown_policy,
        "fold task::effort has unknown beyond policy Wait",
    );
}

#[test]
fn only_admitted_fact_families_can_be_sources() {
    let mut unknown_source = folds();
    fold_mut(&mut unknown_source).sources = UNKNOWN_FACT_FAMILY_SOURCE;
    assert_refusal(
        &unknown_source,
        "fold task::effort names unknown fact family Fixture => fixture::*",
    );
}

#[test]
fn generated_fact_plane_patterns_match_the_runtime_predicates() {
    for &(variant, pattern) in admission::FACT_PLANES {
        let plane = match variant {
            "Task" => fold::FactPlane::Task,
            "Claim" => fold::FactPlane::Claim,
            "Lane" => fold::FactPlane::Lane,
            "Lineage" => fold::FactPlane::Lineage,
            "Journal" => fold::FactPlane::Journal,
            other => panic!("unhandled generated fact plane {other}"),
        };
        for kind in FactKind::ALL {
            assert_eq!(
                admission::pattern_matches(pattern, kind.as_str()),
                plane.contains(*kind),
                "generated pattern and runtime predicate disagree for {variant} / {kind}",
            );
        }
    }
}

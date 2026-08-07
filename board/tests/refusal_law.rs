//! Every refusal variant this crate declares is bound to a test that asserts it.
//!
//! `board::lapse::Refusal` and `board::claim::ClaimRefusal` were each closed by
//! hand, one task per enum, and nothing measured either. An eighteenth variant
//! would reopen the gap in silence: it lands, the `Display` impl forces one
//! formatting arm, the compiler is satisfied, and no test ever names it.
//!
//! The law is the two matches below. They are exhaustive and wildcard-free, so a
//! new variant does not compile until it is named here, and a renamed one moves
//! its arm instead of leaving a count intact. Each arm answers with the test that
//! asserts that variant, and the tests read the suite text to check the named
//! test exists and does name it, so pointing a new variant at some existing test
//! is not a way through either.
//!
//! The enumeration is recovered from this file's own arms rather than kept as a
//! second list beside them. A second list would not be forced to grow: a variant
//! plus its arm compiles, and a hand-kept array of seventeen would stay at
//! seventeen and keep passing. The arms are the one thing the compiler makes the
//! next author write.
//!
//! `Refusal` in `lane/src/renew.rs` carries the same hazard, and all four of its
//! variants -- `Dead`, `Stale`, `Displaced` and `Regressive` -- are asserted
//! today, in that module's own test block rather than in a `tests/` suite, which
//! is the text a law over there would have to read. It is deliberately not
//! covered here: `board` does not depend on `lane`
//! and must not, because `lane` depends on `board`, so no test binary in this
//! crate can name that enum. Covering it means a law of this shape inside the
//! `lane` crate, reading that declaration.
//!
//! Declaration sites are named by file and item rather than by line, so that a
//! doc inserted above one of them does not silently retarget the citation.

use board::claim::ClaimRefusal;
use board::lapse::Refusal;

/// This file as text, so the law can enumerate the arms the compiler checked.
const LAW: &str = include_str!("refusal_law.rs");

/// The module declaring each enum under law.
const LAPSE_DECLARATION: &str = include_str!("../src/lapse.rs");
const CLAIM_DECLARATION: &str = include_str!("../src/claim.rs");

/// Where a lapse assertion may live: the suite, and the in-file test module.
///
/// `Changed` is asserted only in the second, so a law that read `tests/` alone
/// would report it uncovered and invite a redundant test.
const LAPSE_SUITE: [&str; 2] = [include_str!("lapse.rs"), LAPSE_DECLARATION];

/// Where a claim assertion may live.
const CLAIM_SUITE: [&str; 1] = [include_str!("coordination.rs")];

/// The test that asserts each `Refusal`.
///
/// Wildcard-free on purpose: the missing arm is the point of the law.
fn lapse_covering_test(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::MissingTask(..) => "timer_and_authority_refusals_leave_the_journal_unchanged",
        Refusal::MissingClaim => {
            "a_task_that_was_never_claimed_refuses_and_claiming_it_makes_it_lapse"
        }
        Refusal::MissingLease => "a_claim_without_its_lease_is_a_typed_zero_write_refusal",
        Refusal::Inactive(..) => "timer_and_authority_refusals_leave_the_journal_unchanged",
        Refusal::ClaimLeaseEpoch { .. } => {
            "a_lease_epoch_that_left_the_claim_behind_is_named_before_the_request_epoch"
        }
        Refusal::ClaimEpoch { .. } => "timer_and_authority_refusals_leave_the_journal_unchanged",
        Refusal::ClaimOccupant { .. } => "timer_and_authority_refusals_leave_the_journal_unchanged",
        Refusal::IncoherentLease => {
            "an_expiry_past_the_durable_maximum_refuses_and_one_inside_it_lapses"
        }
        Refusal::StaleExpiry { .. } => {
            "a_superseded_expiry_is_refused_and_the_renewed_one_still_lapses"
        }
        Refusal::Early { .. } => "timer_and_authority_refusals_leave_the_journal_unchanged",
        Refusal::MissingResidency => {
            "a_board_that_seated_no_residency_refuses_and_a_seated_one_lapses"
        }
        Refusal::ResidentState(..) => {
            "residency_refusals_name_the_seat_they_read_and_write_nothing"
        }
        Refusal::StaleResidentEpoch { .. } => {
            "residency_refusals_name_the_seat_they_read_and_write_nothing"
        }
        Refusal::MissingLane(..) => "residency_refusals_name_the_seat_they_read_and_write_nothing",
        Refusal::LaneDisposition { .. } => {
            "residency_refusals_name_the_seat_they_read_and_write_nothing"
        }
        Refusal::Displaced { .. } => "residency_refusals_name_the_seat_they_read_and_write_nothing",
        Refusal::Changed(..) => "expiry_changed_after_observation_refuses_before_a_second_fact",
    }
}

/// The test that asserts each `ClaimRefusal`, on the same terms.
fn claim_covering_test(refusal: &ClaimRefusal) -> &'static str {
    match refusal {
        ClaimRefusal::MissingClaim => {
            "active_claim_is_only_admitted_from_matching_coherent_live_heads"
        }
        ClaimRefusal::MissingLease => {
            "active_claim_is_only_admitted_from_matching_coherent_live_heads"
        }
        ClaimRefusal::Inactive(..) => {
            "active_claim_is_only_admitted_from_matching_coherent_live_heads"
        }
        ClaimRefusal::WrongActor { .. } => {
            "active_claim_is_only_admitted_from_matching_coherent_live_heads"
        }
        ClaimRefusal::Epoch { .. } => {
            "active_claim_refuses_a_split_epoch_and_each_out_of_order_lease_bound"
        }
        ClaimRefusal::IncoherentLease => {
            "active_claim_refuses_a_split_epoch_and_each_out_of_order_lease_bound"
        }
        ClaimRefusal::NotStarted { .. } => {
            "active_claim_refuses_a_lease_whose_start_has_not_arrived"
        }
        ClaimRefusal::Expired { .. } => {
            "active_claim_is_only_admitted_from_matching_coherent_live_heads"
        }
    }
}

/// Every arm of one match above, as the variant it names and the test it answers.
///
/// The marker is assembled here rather than written as a literal, so that reading
/// this file cannot find the search text inside the searching code.
fn arms(declared: &str) -> Vec<(String, String)> {
    let marker = format!("\n{}{declared}::", " ".repeat(8));
    let mut found = Vec::new();
    let mut rest = LAW;
    while let Some(at) = rest.find(&marker) {
        let arm = &rest[at + marker.len()..];
        let variant: String = arm
            .chars()
            .take_while(|letter| letter.is_alphanumeric() || *letter == '_')
            .collect();
        let opening = arm.find('"').expect("every arm answers with a test name");
        let closing = arm[opening + 1..]
            .find('"')
            .expect("every arm's test name is closed");
        found.push((variant, arm[opening + 1..opening + 1 + closing].to_owned()));
        rest = &arm[opening + 1 + closing..];
    }
    found
}

/// Whether `test` is declared somewhere in `suite` and names `refusal` in its body.
///
/// The body ends at the first closing brace in the column the routine itself
/// begins in -- column zero in the suite, column four inside the in-file test
/// module -- so a match found past the end of one test is not credited to it. Any
/// shallower rule cuts the body short: these tests close a loop or an array of
/// cases partway through, and the refusal they name can be the one after it.
fn asserts(suite: &[&str], test: &str, refusal: &str) -> bool {
    suite.iter().any(|source| {
        let Some(start) = source.find(&format!("fn {test}(")) else {
            return false;
        };
        let opened = source[..start].rfind('\n').map_or(0, |line| line + 1);
        let closing = format!("\n{}}}\n", " ".repeat(start - opened));
        let body = &source[start..];
        body[..body.find(&closing).unwrap_or(body.len())].contains(refusal)
    })
}

/// Every variant `declared` lists, read from the declaration rather than counted.
///
/// This is what makes the arms provably complete instead of merely present. The
/// compiler forces one arm per variant, but nothing forces the parse above to
/// find every arm that was written, and a parse that recovered three of seventeen
/// would leave fourteen bindings unchecked and report success.
fn declared_variants(source: &str, declared: &str) -> Vec<String> {
    let at = source
        .find(&format!("pub enum {declared} {{"))
        .expect("the enum under law is declared by the module it is read from");
    let body = &source[at..];
    let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
    body.lines()
        .filter_map(|line| {
            let variant = line.strip_prefix("    ")?;
            variant.chars().next().filter(char::is_ascii_uppercase)?;
            Some(
                variant
                    .chars()
                    .take_while(|letter| letter.is_alphanumeric() || *letter == '_')
                    .collect(),
            )
        })
        .collect()
}

/// Check one enum's arms against the suite that is supposed to assert them.
///
/// The control is the first assertion, not a count: it holds the parsed arms
/// against an answer the compiled match returned for a real value, so a parse
/// that silently found nothing, or found the wrong text, cannot report success.
fn law(declared: &str, declaration: &str, control: (&str, &str), suite: &[&str]) {
    let bound = arms(declared);
    assert_eq!(
        bound
            .iter()
            .find(|(variant, _)| variant == control.0)
            .map(|(_, test)| test.as_str()),
        Some(control.1),
        "{declared}::{} is bound to {} by the compiled match, and the arms read \
         back out of this file must say the same",
        control.0,
        control.1
    );
    for variant in declared_variants(declaration, declared) {
        assert!(
            bound.iter().any(|(named, _)| *named == variant),
            "{declared} declares {variant}, and the arms read back out of this \
             file must reach it; a variant the parse above misses is a variant \
             whose binding below is never checked"
        );
    }
    for (variant, test) in &bound {
        assert!(
            asserts(suite, test, &format!("{declared}::{variant}")),
            "{declared}::{variant} names {test}, which must be a test that exists \
             and that names this variant; naming a test that asserts something \
             else leaves the variant unasserted"
        );
    }
}

#[test]
fn every_lapse_refusal_variant_is_bound_to_a_test_that_asserts_it() {
    law(
        "Refusal",
        LAPSE_DECLARATION,
        ("MissingClaim", lapse_covering_test(&Refusal::MissingClaim)),
        &LAPSE_SUITE,
    );
}

#[test]
fn every_claim_refusal_variant_is_bound_to_a_test_that_asserts_it() {
    law(
        "ClaimRefusal",
        CLAIM_DECLARATION,
        (
            "IncoherentLease",
            claim_covering_test(&ClaimRefusal::IncoherentLease),
        ),
        &CLAIM_SUITE,
    );
}

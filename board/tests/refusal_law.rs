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
//! What the text check settles and what it leaves open is stated on `asserts`
//! rather than promised here: it holds the target to a `#[test]` naming that
//! exact path, and it does not read the mention. A variant named in a comment
//! passes. The law narrows where an unasserted variant can hide; it is not a
//! proof that the value was compared against anything.
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

/// Whether `test` is a `#[test]` in `suite` whose body names `refusal`.
///
/// The body ends at the first closing brace in the column the routine itself
/// begins in -- column zero in the suite, column four inside the in-file test
/// module -- so a match found past the end of one test is not credited to it. Any
/// shallower rule cuts the body short: these tests close a loop or an array of
/// cases partway through, and the refusal they name can be the one after it.
///
/// What this does not do is read the mention. A variant named in a comment, or
/// constructed and never compared, satisfies it, so the arms above are worth
/// reading rather than trusted. The bound is deliberate: telling an assertion
/// from a mention means parsing the test, and a parse that got it wrong would
/// fail closed on honest bindings. The two ways a mention can be counterfeit
/// without any parse are refused here instead.
fn asserts(suite: &[&str], test: &str, refusal: &str) -> bool {
    suite.iter().any(|source| {
        let Some(start) = source.find(&format!("fn {test}(")) else {
            return false;
        };
        let opened = source[..start].rfind('\n').map_or(0, |line| line + 1);
        if !is_test(&source[..opened]) {
            return false;
        }
        let closing = format!("\n{}}}\n", " ".repeat(start - opened));
        let body = &source[start..];
        names(&body[..body.find(&closing).unwrap_or(body.len())], refusal)
    })
}

/// Whether the routine following `preceding` carries the `#[test]` attribute.
///
/// `LAPSE_SUITE` has to include the declaring module, because `Changed` is
/// asserted only in that module's own test block. The side effect is that every
/// production function in it became an eligible target, and one of them,
/// `map_commit_error`, names the very variant it maps. Binding `Changed` to that
/// function satisfied a law whose diagnostic promised a test.
///
/// The walk goes back over the attributes, doc lines and blank lines that may
/// stand between `#[test]` and the `fn`, and stops at the first line that is
/// none of those -- the brace closing whatever came before -- so the attribute
/// found belongs to this routine and not to the one above it.
fn is_test(preceding: &str) -> bool {
    preceding
        .lines()
        .rev()
        .map(str::trim)
        .take_while(|line| line.is_empty() || line.starts_with('#') || line.starts_with("//"))
        .any(|line| line == "#[test]")
}

/// Whether `text` names `refusal` whole, rather than inside a longer name.
///
/// A longer name can swallow a shorter one from either end, so both ends are
/// held to the same rule: the character on each side must be absent or a
/// non-identifier one. `ClaimRefusal::Inactive` contains `Refusal::Inactive` and
/// four names are shared between the two enums, which is the left end; a variant
/// whose name is a prefix of a sibling's contains it too, so `Refusal::Changed`
/// must not be answered for by `Refusal::ChangedElsewhere`, which is the right.
/// Neither end costs an honest binding: every real occurrence opens on `(`, `&`
/// or a space and closes on `(`, `)`, `,` or the end of its line.
fn names(text: &str, refusal: &str) -> bool {
    let whole = |letter: char| !letter.is_alphanumeric() && letter != '_';
    text.match_indices(refusal).any(|(at, _)| {
        text[..at].chars().next_back().is_none_or(whole)
            && text[at + refusal.len()..].chars().next().is_none_or(whole)
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
            "{declared}::{variant} names {test}, which must be a #[test] that \
             exists and whose body writes {declared}::{variant} whole, with a \
             non-identifier character or nothing on each side of it; a routine \
             that is no test, a mention buried in a longer name from either \
             end, or a test that never names the variant at all, leaves the \
             variant unasserted"
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

/// The three counterfeits `asserts` refuses, on text written here rather than found.
///
/// Each path is assembled instead of spelled, on the same terms as `arms`: a
/// literal at arm indentation would be read back out of this file as an arm of
/// the matches above, binding a variant the compiler never checked.
///
/// Written against `asserts` directly rather than by mutating the real arms,
/// because every counterfeit has to fail for the same variant that succeeds when
/// honest, and the suite holds no body naming only a sibling of either kind: no
/// lapse test names the claim enum, and no variant name is a prefix of another.
#[test]
fn only_a_test_naming_that_very_enum_is_credited_with_asserting_a_variant() {
    let lapse = format!("{}::{}", "Refusal", "MissingClaim");
    let sibling = format!("Claim{lapse}");
    let longer = format!("{lapse}Elsewhere");
    let asserted = format!("#[test]\nfn bound() {{\n    refuses(&{lapse});\n}}\n");
    let production = format!("/// Map one.\nfn bound() {{\n    Err({lapse})\n}}\n");
    let shadowed = format!("#[test]\nfn bound() {{\n    refuses(&{sibling});\n}}\n");
    let extended = format!("#[test]\nfn bound() {{\n    refuses(&{longer});\n}}\n");

    assert!(
        asserts(&[&asserted], "bound", &lapse),
        "a test whose body names {lapse} is the honest case, and refusing it \
         would make the two refusals below vacuous"
    );
    assert!(
        !asserts(&[&production], "bound", &lapse),
        "{lapse} is named by a routine carrying no #[test], which is how a \
         production function in the declaring module answered for a variant"
    );
    assert!(
        !asserts(&[&shadowed], "bound", &lapse),
        "the only mention is {sibling}, which contains {lapse} as a substring \
         and must not satisfy the lapse binding"
    );
    assert!(
        !asserts(&[&extended], "bound", &lapse),
        "the only mention is {longer}, which contains {lapse} from the other \
         end, and a variant must not be credited to a sibling that merely \
         begins with its name"
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

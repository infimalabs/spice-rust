//! A refused submission names its reason instead of doing nothing quietly.
//!
//! The forms these decisions serve live in a wasm-only module, so nothing here
//! can press their buttons. What it can do is hold every refusal to the same
//! bar the operator holds it to: a sentence naming the field and what an
//! accepted value looks like. Asserting only that a refusal happened would
//! pass against the defect this file exists to pin, which was a refusal that
//! happened perfectly well and said nothing.

use app::compose::{self, Notice, Refusal};

/// Every field the seat form offers, filled with a value that is accepted.
///
/// Each test below spoils exactly one of these, so a refusal it observes can
/// only have come from the field it spoiled. A helper that spoiled two at once
/// would pass whichever check ran first and prove nothing about the other.
const WORKTREE: &str = "/tmp/lane";
const VENDOR: &str = "claude";
const LIFE: &str = "steer";

#[test]
fn a_complete_seat_form_is_accepted_with_its_fields_intact() {
    let seat = compose::seat(WORKTREE, VENDOR, LIFE).expect("a complete form seats");
    assert_eq!(seat.worktree, WORKTREE);
    assert_eq!(seat.life, "steer");
    assert_eq!(seat.vendor.as_str(), "claude");
}

/// A capitalised vendor is folded rather than refused.
///
/// This is the trip the operator actually hits: the field is free text with no
/// datalist, the vendors have proper names, and `wire::Name` takes only
/// lower-case words. Typing Claude the way it is written everywhere else once
/// produced no request and no message at all.
#[test]
fn a_capitalised_vendor_is_folded_to_the_lower_case_name_the_wire_takes() {
    let seat = compose::seat(WORKTREE, "Claude", LIFE).expect("a capitalised vendor seats");
    assert_eq!(seat.vendor.as_str(), "claude");

    let shouted = compose::seat(WORKTREE, "CODEX", LIFE).expect("a shouted vendor seats");
    assert_eq!(shouted.vendor.as_str(), "codex");
}

/// Folding case is not forgiving everything else.
///
/// Lower-casing makes the common trip work, and it would be easy for that to
/// become a vendor field that accepts anything and reports its refusal from
/// the resident a network round trip later, or not at all.
#[test]
fn a_vendor_that_is_not_one_word_is_refused_and_quoted_back_as_written() {
    for written in ["my agent", "gpt-5", "clau.de", "claudé", "claude2"] {
        let refusal = compose::seat(WORKTREE, written, LIFE)
            .expect_err("a vendor that is not one lower-case word is refused");
        assert_eq!(
            refusal,
            Refusal::Vendor {
                written: written.to_owned()
            },
            "{written:?}"
        );
        let sentence = refusal.to_string();
        assert!(sentence.contains("lower-case word"), "{sentence}");
        assert!(sentence.contains(&format!("{written:?}")), "{sentence}");
    }
}

/// A vendor is trimmed like every other field before it is judged.
///
/// Otherwise a trailing space picked up from a paste would be reported as a
/// vendor that is not one word, quoting a value that looks correct to read.
#[test]
fn a_vendor_is_trimmed_before_it_is_judged() {
    let seat = compose::seat(WORKTREE, "  Claude\n", LIFE).expect("a padded vendor seats");
    assert_eq!(seat.vendor.as_str(), "claude");
}

#[test]
fn a_blank_vendor_is_refused_by_name_rather_than_quoted_as_nothing() {
    let refusal = compose::seat(WORKTREE, "   ", LIFE).expect_err("a blank vendor refuses");
    assert_eq!(
        refusal,
        Refusal::Missing {
            field: "Vendor",
            hint: "one lower-case word such as claude or codex",
        },
        "a blank vendor is a missing field, not a malformed one"
    );
}

#[test]
fn a_blank_worktree_is_refused_by_name_with_the_shape_of_a_good_one() {
    for written in ["", "   ", "\t"] {
        let refusal = compose::seat(written, VENDOR, LIFE).expect_err("a blank worktree refuses");
        let sentence = refusal.to_string();
        assert!(sentence.starts_with("Worktree is required"), "{sentence}");
        assert!(sentence.contains("absolute path"), "{sentence}");
    }
}

#[test]
fn a_blank_life_is_refused_by_name_and_lists_the_lives_on_offer() {
    let refusal = compose::seat(WORKTREE, VENDOR, "  ").expect_err("a blank life refuses");
    let sentence = refusal.to_string();
    assert!(sentence.starts_with("Life is required"), "{sentence}");
    for life in ["steer", "drive", "drain"] {
        assert!(sentence.contains(life), "{sentence} omits {life}");
    }
}

/// Refusals arrive in the order the form lists its fields.
///
/// Order is a decision, not an accident: the operator fills this form top to
/// bottom, and a refusal that skips to the last field they touched sends them
/// back up past ones they had already finished. The form renders Worktree,
/// then Vendor, then Life, and these three calls each leave every later field
/// broken too, so any other precedence changes which one is named.
#[test]
fn refusals_arrive_in_the_order_the_form_lists_its_fields() {
    let named = |worktree, vendor, life| match compose::seat(worktree, vendor, life) {
        Err(Refusal::Missing { field, .. }) => field,
        Err(Refusal::Vendor { .. }) => "Vendor",
        other => panic!("a broken form refuses: {other:?}"),
    };
    assert_eq!(named("", "My Agent", ""), "Worktree");
    assert_eq!(named(WORKTREE, "My Agent", ""), "Vendor");
    assert_eq!(named(WORKTREE, VENDOR, ""), "Life");
}

#[test]
fn steering_a_seated_lane_returns_the_seat_beside_the_trimmed_message() {
    let (seat, text) =
        compose::steer("  ship it  ", Some(("lane", "occupant"))).expect("a seated lane steers");
    assert_eq!(seat, ("lane", "occupant"));
    assert_eq!(text, "ship it");
}

/// An unseated selection is refused before the message is even read.
///
/// The seat travels inside the accepted result, so this ordering is what makes
/// the caller unable to hold a message without the lane it goes to. Were the
/// message checked first, a blank message on an empty seat would report the
/// wrong problem and the caller would need its own second check.
#[test]
fn steering_with_no_seated_lane_is_refused_before_the_message_is_read() {
    let refusal =
        compose::steer::<(&str, &str)>("ship it", None).expect_err("an unseated selection refuses");
    assert_eq!(refusal, Refusal::Unseated);
    assert_eq!(
        refusal.to_string(),
        "Select a seated agent before sending direction."
    );

    let blank =
        compose::steer::<(&str, &str)>("", None).expect_err("an unseated selection refuses");
    assert_eq!(blank, Refusal::Unseated, "the seat outranks the message");
}

#[test]
fn steering_a_blank_message_is_refused_by_name() {
    for written in ["", " ", "\n\t "] {
        let refusal =
            compose::steer(written, Some("lane")).expect_err("a blank message refuses to send");
        let sentence = refusal.to_string();
        assert!(sentence.starts_with("Message is required"), "{sentence}");
        assert!(sentence.contains("direction to send"), "{sentence}");
    }
}

/// A notice shows a refusal and shows nothing otherwise.
///
/// The view asks `is_some` before rendering anything, so a notice that carried
/// no refusal but still claimed to be showing one would put an empty alert
/// under the form on every keystroke.
#[test]
fn a_notice_carries_one_refusal_and_is_empty_without_one() {
    let empty = Notice::none();
    assert!(!empty.is_some());
    assert_eq!(empty.refusal(), None);
    assert_eq!(empty.to_string(), "");
    assert_eq!(empty, Notice::default());

    let notice = Notice::from(Refusal::Unseated);
    assert!(notice.is_some());
    assert_eq!(notice.refusal(), Some(&Refusal::Unseated));
    assert_eq!(notice.to_string(), Refusal::Unseated.to_string());
    assert_ne!(notice, empty);
}

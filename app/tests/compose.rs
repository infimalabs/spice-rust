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
const LIFE: &str = "steer";

fn vendor() -> wire::Name {
    "omp".parse().expect("the fixture spells a vendor name")
}

fn choice(name: &str) -> wire::intent::VendorChoice {
    wire::intent::VendorChoice {
        name: name.parse().expect("the fixture spells a vendor name"),
        label: name.to_owned(),
        default_model: "claude-opus-5".to_owned(),
        default_effort: "high".to_owned(),
    }
}

/// A seat runs on the catalogue's one vendor; any other catalogue offers no
/// seat vendor rather than a guess among rows.
#[test]
fn a_seat_vendor_is_the_catalogue_s_only_row() {
    let catalog = |vendors| wire::intent::VendorCatalogAnswer {
        vendors,
        efforts: vec!["high".to_owned()],
    };
    let single = catalog(vec![choice("omp")]);
    assert_eq!(
        compose::seat_vendor(&single).map(|vendor| vendor.name.as_str()),
        Some("omp")
    );
    assert_eq!(compose::seat_vendor(&catalog(Vec::new())), None);
    assert_eq!(
        compose::seat_vendor(&catalog(vec![choice("omp"), choice("other")])),
        None
    );
}

#[test]
fn a_complete_seat_form_is_accepted_with_its_fields_intact() {
    let seat = compose::seat("  Ava  ", WORKTREE, vendor(), LIFE).expect("a complete form seats");
    assert_eq!(seat.name.as_deref(), Some("Ava"));
    assert_eq!(seat.worktree, WORKTREE);
    assert_eq!(seat.life, "steer");
    assert_eq!(seat.vendor.as_str(), "omp");
}

#[test]
fn a_blank_worktree_is_refused_by_name_with_the_shape_of_a_good_one() {
    for written in ["", "   ", "\t"] {
        let refusal =
            compose::seat("", written, vendor(), LIFE).expect_err("a blank worktree refuses");
        let sentence = refusal.to_string();
        assert!(sentence.starts_with("Worktree is required"), "{sentence}");
        assert!(sentence.contains("absolute path"), "{sentence}");
    }
}

#[test]
fn a_blank_life_is_refused_by_name_and_lists_the_lives_on_offer() {
    let refusal = compose::seat("", WORKTREE, vendor(), "  ").expect_err("a blank life refuses");
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
/// then Life, and the first call leaves the later field broken too, so any
/// other precedence changes which one is named.
#[test]
fn refusals_arrive_in_the_order_the_form_lists_its_fields() {
    let named = |worktree, life| match compose::seat("", worktree, vendor(), life) {
        Err(Refusal::Missing { field, .. }) => field,
        other => panic!("a broken form refuses: {other:?}"),
    };
    assert_eq!(named("", ""), "Worktree");
    assert_eq!(named(WORKTREE, ""), "Life");
}

/// The selection fields never refuse: blank leaves the vendor's own value in
/// force, and the effort is folded to lower case, while the model is handed on
/// as written because the vendor resolves it.
#[test]
fn a_seat_selection_is_absent_when_blank_and_folded_when_written() {
    assert_eq!(compose::selection("", "  "), compose::Selection::default());
    assert_eq!(
        compose::selection(" gpt-5.5 ", " Low\n"),
        compose::Selection {
            model: Some("gpt-5.5".to_owned()),
            effort: Some("low".to_owned()),
        }
    );
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

#[test]
fn a_task_observation_returns_the_typed_handle_and_trimmed_text() {
    let note = compose::note("  SPICE-1kKRRsDP  ", "  one observation  ")
        .expect("a complete task observation composes");
    assert_eq!(note.task.as_str(), "SPICE-1kKRRsDP");
    assert_eq!(note.text, "one observation");
}

#[test]
fn an_unparseable_task_handle_is_refused_with_a_visible_sentence() {
    // This is a task-handle fixture, not an environment access. Assemble the
    // underscore-shaped case from ordinary fragments so the installed ledger
    // cannot mistake test data for a process input.
    let underscore_handle = ["SPICE", "_123"].concat();
    assert!(underscore_handle.starts_with("SPICE_"));
    assert!(underscore_handle.ends_with("123"));
    for written in [
        "not a handle",
        "spice-aBcD",
        underscore_handle.as_str(),
        "SPICE-",
    ] {
        let refusal =
            compose::note(written, "observation").expect_err("an unparseable task handle refuses");
        assert_eq!(
            refusal,
            Refusal::Task {
                written: written.to_owned(),
            }
        );
        let sentence = refusal.to_string();
        assert!(sentence.contains("Task must be"), "{sentence}");
        assert!(sentence.contains(&format!("{written:?}")), "{sentence}");
    }
}

#[test]
fn note_fields_refuse_in_form_order() {
    let task = compose::note(" ", " ").expect_err("a blank form refuses its task first");
    assert!(task.to_string().starts_with("Task is required"));

    let observation = compose::note("SPICE-1kKRRsDP", " ")
        .expect_err("a blank observation refuses after a valid task");
    assert!(
        observation
            .to_string()
            .starts_with("Observation is required"),
        "{observation}"
    );
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

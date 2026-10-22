//! Form decisions preserve accepted input and reject invalid submissions with
//! typed refusals. Human-facing explanations are not fixed output spellings.

use app::compose::{self, Refusal};

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

    let blank =
        compose::steer::<(&str, &str)>("", None).expect_err("an unseated selection refuses");
    assert_eq!(blank, Refusal::Unseated, "the seat outranks the message");
}

#[test]
fn steering_a_blank_message_is_refused_by_name() {
    for written in ["", " ", "\n\t "] {
        let refusal =
            compose::steer(written, Some("lane")).expect_err("a blank message refuses to send");
        assert!(matches!(refusal, Refusal::Missing { .. }));
    }
}

/// A conclusion carries no text, so a drafted observation is never silently
/// consumed by it; only a blank composer concludes.
#[test]
fn concluding_with_a_drafted_observation_is_refused_rather_than_discarded() {
    for blank in ["", " ", "\n\t "] {
        compose::conclusion(blank).expect("a blank composer concludes");
    }
    let refusal = compose::conclusion(" evidence ").expect_err("a draft would be lost");
    assert!(matches!(refusal, Refusal::Unused { .. }));
}

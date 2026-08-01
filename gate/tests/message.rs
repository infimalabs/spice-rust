use gate::{OriginTrailer, validate_commit_message};

const VALID: &str = "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD";

#[test]
fn v1_round_trips_byte_identically() {
    let parsed = VALID.parse::<OriginTrailer>().expect("valid v1 trailer");

    assert_eq!(parsed.to_string(), VALID);
    assert_eq!(
        parsed.repo().to_string(),
        "018f82d4-6f59-7d06-92e3-bce9131163c7"
    );
    assert_eq!(parsed.task().as_str(), "SPICE-aBcD");
}

#[test]
fn reordered_extra_missing_and_other_spellings_are_refused() {
    let malformed = [
        "Spice-Origin: repo=018f82d4-6f59-7d06-92e3-bce9131163c7 v1 task=SPICE-aBcD",
        "Spice-Origin: v1 task=SPICE-aBcD repo=018f82d4-6f59-7d06-92e3-bce9131163c7",
        "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD actor=agent",
        "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7",
        "Spice-Origin: v1 task=SPICE-aBcD",
        "Spice-Origin: v2 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD",
        "spice-origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD",
        "Spice-Origin: v1 repo=018F82D4-6F59-7D06-92E3-BCE9131163C7 task=SPICE-aBcD",
        "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=spice-aBcD",
        "Spice-Origin:  v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD",
        "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7  task=SPICE-aBcD",
        "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD ",
    ];

    for line in malformed {
        let error = line
            .parse::<OriginTrailer>()
            .expect_err("non-v1 spelling must be refused");
        assert_eq!(error.evidence(), line);
    }
}

#[test]
fn commit_gate_names_the_exact_commit_and_preserves_unresolved_evidence() {
    let historical = "Spice-Origin-v0: repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD";
    let message = format!("Record provenance\n\n{historical}\n");

    let error = validate_commit_message("8ac31e7", &message)
        .expect_err("historical origin evidence must remain unresolved");

    assert_eq!(error.commit(), "8ac31e7");
    assert_eq!(error.line(), 3);
    assert_eq!(error.evidence(), historical);
    assert_eq!(message.lines().nth(2), Some(historical));
    assert!(error.to_string().starts_with("commit 8ac31e7 "));
}

#[test]
fn commit_gate_validates_every_origin_trailer_and_ignores_unrelated_lines() {
    let message = format!(
        "Record provenance\n\nBody: prose\n\n{VALID}\nReviewed-By: Agent <agent@example.test>\nSpice-Originator: retained\n{VALID}\n"
    );

    let trailers = validate_commit_message("f29d16c", &message).expect("valid commit message");

    assert_eq!(trailers.len(), 2);
    assert!(trailers.iter().all(|trailer| trailer.to_string() == VALID));
}

#[test]
fn commit_gate_refuses_spacing_that_only_normalization_would_accept() {
    let malformed = " Spice-Origin : v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=SPICE-aBcD";

    let error = validate_commit_message("d431f90", malformed)
        .expect_err("the gate must not trim a malformed trailer");

    assert_eq!(error.commit(), "d431f90");
    assert_eq!(error.evidence(), malformed);
}

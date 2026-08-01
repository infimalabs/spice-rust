//! Spelling fixtures for the canonical identity types.
//!
//! `uuid` and `jiff` both accept a family of spellings. WIRE admits exactly
//! one of each, so these fixtures pin the refusals: every spelling a looser
//! parser would silently repair must fail here instead.

use wire::{CanonicalUuid, IdentError, Instant};

const CANONICAL: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";

#[test]
fn the_canonical_spelling_is_admitted_and_renders_unchanged() {
    let parsed: CanonicalUuid = CANONICAL.parse().expect("canonical spelling");
    assert_eq!(parsed.to_string(), CANONICAL);
}

#[test]
fn only_the_canonical_uuid_spelling_is_admitted() {
    let refused = [
        ("braces", "{018f82d4-6f59-7d06-92e3-bce9131163c2}"),
        ("upper case", "018F82D4-6F59-7D06-92E3-BCE9131163C2"),
        ("mixed case", "018f82d4-6F59-7d06-92E3-bce9131163c2"),
        ("compact", "018f82d46f597d0692e3bce9131163c2"),
        ("urn", "urn:uuid:018f82d4-6f59-7d06-92e3-bce9131163c2"),
        ("truncated", "018f82d4-6f59-7d06-92e3-bce9131163c"),
        ("overlong", "018f82d4-6f59-7d06-92e3-bce9131163c22"),
        ("shifted separators", "018f82d4-6f597-d06-92e3-bce9131163c2"),
        ("wrong separator", "018f82d4_6f59-7d06-92e3-bce9131163c2"),
        ("non-hex digit", "018f82d4-6f59-7d06-92e3-bce9131163cg"),
        ("leading space", " 018f82d4-6f59-7d06-92e3-bce9131163c2"),
        ("empty", ""),
    ];

    for (spelling, text) in refused {
        let outcome = text.parse::<CanonicalUuid>();
        assert_eq!(
            outcome,
            Err(IdentError::Uuid(text.to_owned())),
            "the {spelling} spelling must be refused, not repaired"
        );
    }
}

#[test]
fn a_non_canonical_uuid_is_refused_by_the_decoder_too() {
    // The refusal has to sit in the type, not in a caller-side check, or a
    // decoded frame could smuggle a spelling the parser would have rejected.
    let admitted = wire::decode::<CanonicalUuid>(format!("\"{CANONICAL}\"").as_bytes());
    let refused =
        wire::decode::<CanonicalUuid>(b"\"018F82D4-6F59-7D06-92E3-BCE9131163C2\"".as_slice());

    assert!(admitted.is_ok(), "the canonical spelling decodes");
    assert!(refused.is_err(), "upper-case text is not a canonical uuid");
}

#[test]
fn only_millisecond_utc_instants_are_admitted() {
    let canonical = "2030-04-12T18:44:09.271Z";
    let parsed: Instant = canonical.parse().expect("canonical instant");
    assert_eq!(parsed.to_string(), canonical);

    let refused = [
        ("numeric offset", "2030-04-12T18:44:09.271+00:00"),
        ("lower-case zulu", "2030-04-12T18:44:09.271z"),
        ("no fraction", "2030-04-12T18:44:09Z"),
        ("second precision", "2030-04-12T18:44:09.2Z"),
        ("microsecond precision", "2030-04-12T18:44:09.271828Z"),
        ("space separator", "2030-04-12 18:44:09.271Z"),
        ("compact date", "20300412T18:44:09.271Z"),
        ("trailing space", "2030-04-12T18:44:09.271Z "),
        ("empty", ""),
    ];

    for (spelling, text) in refused {
        let outcome = text.parse::<Instant>();
        assert_eq!(
            outcome,
            Err(IdentError::Instant(text.to_owned())),
            "the {spelling} spelling must be refused, not repaired"
        );
    }
}

#[test]
fn instants_render_at_millisecond_precision_regardless_of_input_resolution() {
    // A timestamp built elsewhere in the workspace can carry more resolution
    // than the wire spells. Rendering truncates rather than widening the shape.
    let sharper: jiff::Timestamp = "2030-04-12T18:44:09.271828Z".parse().expect("timestamp");
    let instant = Instant::new(sharper);

    assert_eq!(instant.to_string(), "2030-04-12T18:44:09.271Z");
    assert_eq!(
        wire::encode(&instant).expect("encodes"),
        b"\"2030-04-12T18:44:09.271Z\"",
    );
}

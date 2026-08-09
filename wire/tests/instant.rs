//! Construction, spelling, and identity fixtures for wire instants.

use std::cmp::Ordering;

use wire::{IdentError, Instant};

#[test]
fn unix_epoch_has_the_canonical_wire_spelling() {
    assert_eq!(Instant::UNIX_EPOCH.to_string(), "1970-01-01T00:00:00.000Z");
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
    // than the wire spells. The surplus is dropped at construction, so the
    // stored value is the one the spelling denotes.
    let sharper: jiff::Timestamp = "2030-04-12T18:44:09.271828Z".parse().expect("timestamp");
    let instant = Instant::new(sharper).expect("reading is in the wire year range");

    assert_eq!(instant.to_string(), "2030-04-12T18:44:09.271Z");
    assert_eq!(
        wire::encode(&instant).expect("encodes"),
        b"\"2030-04-12T18:44:09.271Z\"",
    );
    assert_eq!(
        instant.get().subsec_nanosecond() % 1_000_000,
        0,
        "the surplus resolution must not survive construction",
    );
}

#[test]
fn instants_encoding_to_the_same_bytes_are_the_same_value() {
    // The defect this pins: deriving Eq over the full-resolution timestamp
    // while rendering at millisecond precision made two values that encode
    // byte-identically compare unequal.
    let sharper: jiff::Timestamp = "2030-04-12T18:44:09.271828Z".parse().expect("timestamp");
    let narrowed = Instant::new(sharper).expect("reading is in the wire year range");
    let spelled: Instant = "2030-04-12T18:44:09.271Z".parse().expect("canonical");

    assert_eq!(
        wire::encode(&narrowed).expect("encodes"),
        wire::encode(&spelled).expect("encodes"),
        "the two values must encode identically for this test to mean anything",
    );
    assert_eq!(
        narrowed, spelled,
        "equal wire bytes must imply equal values"
    );
}

#[test]
fn instants_round_trip_through_the_wire_by_identity() {
    // Every constructible instant survives encode-then-decode unchanged,
    // including ones built from sub-millisecond and pre-epoch readings.
    let readings = [
        "2030-04-12T18:44:09.271828Z",
        "2030-04-12T18:44:09.271999Z",
        "1969-12-31T23:59:59.999999Z",
        "1950-01-01T00:00:00.000001Z",
        "1970-01-01T00:00:00.000000Z",
    ];

    for reading in readings {
        let original = Instant::new(reading.parse::<jiff::Timestamp>().expect("timestamp"))
            .expect("reading is in the wire year range");
        let encoded = wire::encode(&original).expect("encodes");
        let decoded: Instant = wire::decode(&encoded).expect("decodes");

        assert_eq!(
            decoded, original,
            "{reading} must decode back to the value it encoded from",
        );
        assert_eq!(decoded.to_string(), original.to_string());
    }
}

#[test]
fn instant_ordering_and_hashing_agree_with_the_rendered_spelling() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash_of(instant: Instant) -> u64 {
        let mut hasher = DefaultHasher::new();
        instant.hash(&mut hasher);
        hasher.finish()
    }

    let coarse = Instant::new("2030-04-12T18:44:09.271000Z".parse().expect("timestamp"))
        .expect("reading is in the wire year range");
    let sharper = Instant::new("2030-04-12T18:44:09.271828Z".parse().expect("timestamp"))
        .expect("reading is in the wire year range");
    let later = Instant::new("2030-04-12T18:44:09.272000Z".parse().expect("timestamp"))
        .expect("reading is in the wire year range");

    assert_eq!(coarse.to_string(), sharper.to_string());
    assert_eq!(
        coarse.cmp(&sharper),
        Ordering::Equal,
        "same spelling, same rank"
    );
    assert_eq!(
        hash_of(coarse),
        hash_of(sharper),
        "same spelling, same hash"
    );
    assert_ne!(coarse.to_string(), later.to_string());
    assert_eq!(
        coarse.cmp(&later),
        Ordering::Less,
        "distinct spellings keep their order"
    );
}

#[test]
fn instant_construction_matches_the_wire_year_range() {
    let minimum_timestamp: jiff::Timestamp =
        "0000-01-01T00:00:00Z".parse().expect("minimum wire year");
    let minimum = Instant::new(minimum_timestamp).expect("minimum wire instant");
    let maximum = Instant::new(jiff::Timestamp::MAX).expect("maximum wire instant");

    for bound in [minimum, maximum] {
        let encoded = wire::encode(&bound).expect("boundary encodes");
        let decoded: Instant = wire::decode(&encoded).expect("boundary decodes");
        assert_eq!(decoded, bound, "a constructible boundary round trips");
    }

    assert!(
        matches!(
            Instant::new(jiff::Timestamp::MIN),
            Err(IdentError::Instant(spelling)) if spelling.starts_with('-')
        ),
        "an extended negative year has no admitted wire spelling",
    );
}

#[test]
fn instants_floor_rather_than_truncate_toward_zero() {
    // Before 1970 the two disagree: truncating toward zero moves the value
    // later and renders a different spelling than the one it was built from.
    let reading: jiff::Timestamp = "1969-12-31T23:59:59.999999Z".parse().expect("timestamp");
    let narrowed = Instant::new(reading).expect("reading is in the wire year range");

    assert_eq!(narrowed.to_string(), "1969-12-31T23:59:59.999Z");
    assert!(
        narrowed.get() <= reading,
        "flooring must not round a pre-epoch reading up to the next millisecond",
    );
}

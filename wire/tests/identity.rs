//! Spelling fixtures for the canonical identity types.
//!
//! `uuid` and `jiff` both accept a family of spellings. WIRE admits exactly
//! one of each, so these fixtures pin the refusals: every spelling a looser
//! parser would silently repair must fail here instead.

use std::cmp::Ordering;

use wire::{CanonicalUuid, Digest, Domain, IdentError, Instant};

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

/// Where each class sits in the sweep below.
///
/// The sweep is a hand-written array, and on its own an array goes stale in
/// silence: add a variant, forget the line, and every domain test keeps passing
/// while covering one class less. This match is the half the compiler checks. A
/// new variant fails to compile here first, and the arm it forces names the
/// index the sweep then has to fill.
const fn position(domain: Domain) -> usize {
    match domain {
        Domain::DistTargetPayload => 0,
        Domain::DistArtifact => 1,
        Domain::DistPins => 2,
        Domain::DistInventory => 3,
        Domain::GateFile => 4,
        Domain::GateTree => 5,
        Domain::GatePolicy => 6,
        Domain::GateInventory => 7,
        Domain::GateMeasurement => 8,
    }
}

#[test]
fn the_sweep_names_every_domain_exactly_once() {
    for (index, domain) in DOMAINS.iter().enumerate() {
        assert_eq!(
            position(*domain),
            index,
            "the domain sweep and the class list disagree about {domain:?}"
        );
    }
}

/// Every domain, so a new class cannot be added without being swept here.
const DOMAINS: [Domain; 9] = [
    Domain::DistTargetPayload,
    Domain::DistArtifact,
    Domain::DistPins,
    Domain::DistInventory,
    Domain::GateFile,
    Domain::GateTree,
    Domain::GatePolicy,
    Domain::GateInventory,
    Domain::GateMeasurement,
];

#[test]
fn every_class_leads_with_the_plane_its_tag_names() {
    for domain in DOMAINS {
        let plane = domain
            .tag()
            .split('.')
            .nth(1)
            .expect("a tag names the plane its class belongs to");
        let spelled = format!("{domain:?}");

        assert!(
            spelled.len() > plane.len() && spelled[..plane.len()].eq_ignore_ascii_case(plane),
            "{spelled} is tagged {} but does not lead with the plane {plane}",
            domain.tag()
        );
        assert!(
            spelled[plane.len()..].starts_with(|letter: char| letter.is_ascii_uppercase()),
            "{spelled} must begin a new word after the plane {plane}"
        );
    }
}

#[test]
fn the_same_bytes_under_two_domains_never_collide() {
    for bytes in [b"".as_slice(), b"\x00", b"payload", b"\xff\xfe\x00\x01"] {
        let computed: Vec<Digest> = DOMAINS
            .iter()
            .map(|domain| Digest::compute(*domain, bytes))
            .collect();

        for (index, digest) in computed.iter().enumerate() {
            for other in &computed[index + 1..] {
                assert_ne!(
                    digest, other,
                    "the same bytes under two domains produced one identity"
                );
            }
        }
    }
}

#[test]
fn a_domain_tag_is_frozen_and_distinct_from_every_other() {
    // The tags are the durable half of the identity: changing one silently
    // re-points every digest already published under it.
    let tags: Vec<&str> = DOMAINS.iter().map(|domain| domain.tag()).collect();
    assert_eq!(
        tags,
        [
            "spice.dist.target-payload.v1",
            "spice.dist.artifact.v1",
            "spice.dist.pins.v1",
            "spice.dist.inventory.v1",
            "spice.gate.file.v1",
            "spice.gate.tree.v1",
            "spice.gate.policy.v1",
            "spice.gate.inventory.v1",
            "spice.gate.measurement.v1",
        ],
        "a domain tag changed; that re-points digests already published under it"
    );

    for (index, tag) in tags.iter().enumerate() {
        assert!(
            !tag.contains('\0'),
            "{tag} contains a NUL, so the domain framing is no longer injective"
        );
        for other in &tags[index + 1..] {
            assert_ne!(tag, other, "two domains share one tag");
        }
    }
}

#[test]
fn a_computed_digest_is_already_the_one_admitted_spelling() {
    // compute and FromStr share one predicate; this proves they agree rather
    // than assuming it, so a mint can never produce what a decode would refuse.
    for domain in DOMAINS {
        let computed = Digest::compute(domain, b"payload");
        let reparsed: Digest = computed
            .as_str()
            .parse()
            .expect("a computed digest is a canonical spelling");

        assert_eq!(computed, reparsed, "minting and decoding disagree");
        assert_eq!(computed.as_str().len(), 64);
        assert!(
            computed
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "a computed digest must render lower-case hex"
        );
    }
}

#[test]
fn the_domain_framing_separates_the_tag_from_the_bytes() {
    // Without the separator, tag+bytes concatenation would let one domain's
    // long tag and another's leading bytes hash identically. Stating the exact
    // framing here pins the construction against a silent change.
    let mut expected = blake3::Hasher::new();
    expected.update(Domain::DistArtifact.tag().as_bytes());
    expected.update(&[0]);
    expected.update(b"payload");

    assert_eq!(
        Digest::compute(Domain::DistArtifact, b"payload").as_str(),
        expected.finalize().to_hex().as_str(),
        "the digest is BLAKE3 over tag, NUL, then bytes"
    );
}

#[test]
fn only_the_canonical_digest_spelling_is_admitted() {
    let canonical = Digest::compute(Domain::DistPins, b"pins")
        .as_str()
        .to_owned();
    let upper = canonical.to_ascii_uppercase();

    for spelling in [
        "",
        "abc",
        upper.as_str(),
        &canonical[..63],
        &format!("{canonical}0"),
        &format!("sha256:{canonical}"),
        &canonical.replace('a', "g"),
    ] {
        assert_eq!(
            spelling.parse::<Digest>(),
            Err(IdentError::Digest(spelling.to_owned())),
            "the {spelling:?} spelling must be refused, not repaired"
        );
    }

    assert!(
        canonical.parse::<Digest>().is_ok(),
        "the canonical spelling is admitted"
    );
}

#[test]
fn a_non_canonical_digest_is_refused_by_the_decoder_too() {
    // Same reason as the uuid decoder fixture: a caller-side check would let a
    // decoded frame carry a spelling the parser rejects.
    let canonical = Digest::compute(Domain::DistInventory, b"inventory")
        .as_str()
        .to_owned();

    let admitted = wire::decode::<Digest>(format!("\"{canonical}\"").as_bytes());
    let refused =
        wire::decode::<Digest>(format!("\"{}\"", canonical.to_ascii_uppercase()).as_bytes());

    assert!(admitted.is_ok(), "the canonical spelling decodes");
    assert!(refused.is_err(), "upper-case hex is not a canonical digest");
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
    let instant = Instant::new(sharper);

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
    let narrowed = Instant::new(sharper);
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
        let original = Instant::new(reading.parse::<jiff::Timestamp>().expect("timestamp"));
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

    let coarse = Instant::new("2030-04-12T18:44:09.271000Z".parse().expect("timestamp"));
    let sharper = Instant::new("2030-04-12T18:44:09.271828Z".parse().expect("timestamp"));
    let later = Instant::new("2030-04-12T18:44:09.272000Z".parse().expect("timestamp"));

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
fn instant_narrowing_is_total_over_the_timestamp_range() {
    // Instant::new floors rather than returning a Result, which is only honest
    // if flooring cannot leave the representable range at either extreme.
    for bound in [jiff::Timestamp::MIN, jiff::Timestamp::MAX] {
        let narrowed = Instant::new(bound);
        assert_eq!(narrowed.get().subsec_nanosecond() % 1_000_000, 0);
        assert!(
            narrowed.get() <= bound,
            "flooring never moves an instant later"
        );
    }
}

#[test]
fn instants_floor_rather_than_truncate_toward_zero() {
    // Before 1970 the two disagree: truncating toward zero moves the value
    // later and renders a different spelling than the one it was built from.
    let reading: jiff::Timestamp = "1969-12-31T23:59:59.999999Z".parse().expect("timestamp");
    let narrowed = Instant::new(reading);

    assert_eq!(narrowed.to_string(), "1969-12-31T23:59:59.999Z");
    assert!(
        narrowed.get() <= reading,
        "flooring must not round a pre-epoch reading up to the next millisecond",
    );
}

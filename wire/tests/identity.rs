//! Spelling fixtures for the canonical identity types.
//!
//! `uuid` and `jiff` both accept a family of spellings. WIRE admits exactly
//! one of each, so these fixtures pin the refusals: every spelling a looser
//! parser would silently repair must fail here instead.

use wire::{CanonicalUuid, Digest, Domain, IdentError};

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

/// Every domain, so a new class cannot be added without being swept here.
const DOMAINS: [Domain; 4] = [
    Domain::TargetPayload,
    Domain::Artifact,
    Domain::Pins,
    Domain::Inventory,
];

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
    expected.update(Domain::Artifact.tag().as_bytes());
    expected.update(&[0]);
    expected.update(b"payload");

    assert_eq!(
        Digest::compute(Domain::Artifact, b"payload").as_str(),
        expected.finalize().to_hex().as_str(),
        "the digest is BLAKE3 over tag, NUL, then bytes"
    );
}

#[test]
fn only_the_canonical_digest_spelling_is_admitted() {
    let canonical = Digest::compute(Domain::Pins, b"pins").as_str().to_owned();
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
    let canonical = Digest::compute(Domain::Inventory, b"inventory")
        .as_str()
        .to_owned();

    let admitted = wire::decode::<Digest>(format!("\"{canonical}\"").as_bytes());
    let refused =
        wire::decode::<Digest>(format!("\"{}\"", canonical.to_ascii_uppercase()).as_bytes());

    assert!(admitted.is_ok(), "the canonical spelling decodes");
    assert!(refused.is_err(), "upper-case hex is not a canonical digest");
}

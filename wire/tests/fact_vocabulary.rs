//! The fact vocabulary is closed, and every kind in it is spelled exactly once.
//!
//! `FactKind` declares itself closed: a spelling absent from the enum is not a
//! fact. A closed set earns nothing unless something enforces it, so this
//! fixture enforces it from two sides.
//!
//! The compiler holds one side. [`plane`] matches `FactKind` exhaustively from
//! outside the defining crate, which is only possible because the enum is not
//! `#[non_exhaustive]`. Adding a variant fails the build naming the variant it
//! did not cover, before any test runs.
//!
//! The source holds the other side. The spellings are read out of
//! `wire/src/fact.rs` rather than restated here, so a kind cannot be added,
//! renamed, or left without a `serde` rename without this suite saying which
//! one. Restating the list would only pin a copy against a copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use wire::FactKind;

/// The plane a fact belongs to, decided by the compiler rather than by text.
///
/// The arms group by plane instead of restating each spelling, so this stays a
/// completeness check and not a second copy of the vocabulary. The tests below
/// tie every answer back to the spelling's own prefix, which is what stops a
/// new variant from being swept into a neighbouring plane to silence the build.
fn plane(kind: FactKind) -> &'static str {
    match kind {
        FactKind::TaskAdded
        | FactKind::TaskPhased
        | FactKind::TaskDone
        | FactKind::TaskReviewed
        | FactKind::TaskAssigned
        | FactKind::TaskNoted
        | FactKind::TaskDeferred
        | FactKind::TaskWoken
        | FactKind::TaskEdited => "task",
        FactKind::ClaimWon
        | FactKind::ClaimCarried
        | FactKind::ClaimFreed
        | FactKind::ClaimLapsed
        | FactKind::ClaimRenewed => "claim",
        FactKind::DocApplied => "doc",
        FactKind::SteerSent
        | FactKind::SteerResent
        | FactKind::SteerAcked
        | FactKind::SteerDeclined
        | FactKind::SteerMissed
        | FactKind::SteerLapsed
        | FactKind::SteerCanceled => "steer",
        FactKind::ConsentAsked
        | FactKind::ConsentGranted
        | FactKind::ConsentDenied
        | FactKind::ConsentQueued
        | FactKind::ConsentFloored => "consent",
        FactKind::LaneSeated
        | FactKind::LaneStarted
        | FactKind::LaneExited
        | FactKind::LaneCompacted
        | FactKind::LaneRenewed
        | FactKind::LaneDrained
        | FactKind::LaneDegraded
        | FactKind::LaneHeld
        | FactKind::LaneResumed => "lane",
        FactKind::TasteConfirmed | FactKind::TasteCleared => "taste",
        FactKind::CanaryAlarmed | FactKind::CanaryContradicted => "canary",
        FactKind::VendorArmed => "vendor",
        FactKind::TrustPended | FactKind::TrustGranted | FactKind::TrustRevoked => "trust",
        FactKind::CommitLanded => "commit",
        FactKind::GateFound | FactKind::GateCleared => "gate",
        FactKind::JournalMigrated | FactKind::JournalBacked | FactKind::JournalRestored => {
            "journal"
        }
        FactKind::UpSeated
        | FactKind::UpQuiescing
        | FactKind::UpDraining
        | FactKind::UpTerminating
        | FactKind::UpKilling
        | FactKind::UpVerifying
        | FactKind::UpDark => "up",
        FactKind::ProofSealed => "proof",
        FactKind::ReleasePublished => "release",
    }
}

/// One declared kind, as `wire/src/fact.rs` writes it.
struct Declared {
    variant: String,
    spelling: Option<String>,
}

/// Read the `FactKind` variants and their spellings out of the source.
///
/// Reading the enum rather than restating it is what lets an added, renamed, or
/// unspelled variant fail here by name.
fn declared_kinds() -> Vec<Declared> {
    let source = fs::read_to_string(fact_source()).expect("wire/src/fact.rs is readable");
    let open = source
        .find("pub enum FactKind {")
        .expect("fact.rs declares FactKind");
    let body = &source[open..];
    let close = body.find("\n}").expect("the FactKind body closes");
    let body = &body[..close];

    let mut declared = Vec::new();
    let mut pending: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#[serde(rename = \"") {
            let spelling = rest
                .split('"')
                .next()
                .expect("a rename attribute closes its string");
            pending = Some(spelling.to_owned());
            continue;
        }
        if line.starts_with("//") || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(variant) = line.strip_suffix(',') {
            if variant.starts_with(char::is_uppercase) {
                declared.push(Declared {
                    variant: variant.to_owned(),
                    spelling: pending.take(),
                });
            }
        }
    }

    assert!(
        declared.len() > 1,
        "no FactKind variants parsed from wire/src/fact.rs; the parser has drifted from the source",
    );
    declared
}

fn fact_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("fact.rs")
}

#[test]
fn every_fact_kind_carries_an_explicit_spelling() {
    // A variant without a rename still serializes: serde falls back to the Rust
    // name, so `CommitLanded` would cross as "CommitLanded" and pass any test
    // that only checks a round trip. The spelling law has to be asserted where
    // it is declared, not inferred from whatever serde happens to emit.
    let unspelled: Vec<String> = declared_kinds()
        .into_iter()
        .filter(|kind| kind.spelling.is_none())
        .map(|kind| kind.variant)
        .collect();

    assert!(
        unspelled.is_empty(),
        "these fact kinds carry no #[serde(rename)], so they would cross under \
         their Rust names instead of a plane::verb spelling: {unspelled:?}",
    );
}

#[test]
fn every_declared_spelling_crosses_as_itself() {
    for kind in declared_kinds() {
        let spelling = kind.spelling.expect("every kind is spelled");
        let quoted = format!("\"{spelling}\"");

        let decoded: FactKind = wire::decode(quoted.as_bytes())
            .unwrap_or_else(|_| panic!("{spelling} is a fact kind the decoder admits"));
        let encoded = wire::encode(&decoded).expect("a fact kind encodes");

        assert_eq!(
            String::from_utf8(encoded).expect("a spelling is text"),
            quoted,
            "{} does not cross as the spelling it declares",
            kind.variant,
        );
    }
}

#[test]
fn no_two_fact_kinds_share_a_spelling() {
    // Two kinds sharing a spelling would decode to whichever serde saw last,
    // silently collapsing two facts into one.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for kind in declared_kinds() {
        let spelling = kind.spelling.expect("every kind is spelled");
        if let Some(first) = seen.insert(spelling.clone(), kind.variant.clone()) {
            panic!("{first} and {} both spell {spelling}", kind.variant);
        }
    }
}

#[test]
fn every_spelling_names_the_plane_the_compiler_assigns() {
    // Ties the exhaustive match to the spellings. Without this, a new variant
    // could be appended to any arm to quiet the build and would still pass.
    for kind in declared_kinds() {
        let spelling = kind.spelling.expect("every kind is spelled");
        let (declared_plane, verb) = spelling
            .split_once("::")
            .unwrap_or_else(|| panic!("{spelling} is not a plane::verb path"));
        assert!(!verb.is_empty(), "{spelling} names no verb");

        let quoted = format!("\"{spelling}\"");
        let decoded: FactKind = wire::decode(quoted.as_bytes()).expect("a declared kind decodes");

        assert_eq!(
            plane(decoded),
            declared_plane,
            "{} is spelled on the {declared_plane} plane but matched as {}",
            kind.variant,
            plane(decoded),
        );
    }
}

#[test]
fn the_vocabulary_spans_every_plane_the_match_names() {
    // The reverse direction: an arm naming a plane no spelling uses would mean
    // the match and the vocabulary have drifted apart.
    let spelled: BTreeSet<String> = declared_kinds()
        .into_iter()
        .filter_map(|kind| kind.spelling)
        .filter_map(|spelling| spelling.split("::").next().map(str::to_owned))
        .collect();

    for expected in [
        "task", "claim", "doc", "steer", "consent", "lane", "taste", "canary", "vendor", "trust",
        "commit", "gate", "journal", "up", "proof", "release",
    ] {
        assert!(
            spelled.contains(expected),
            "the {expected} plane has an arm in plane() but no spelling in FactKind",
        );
    }
}

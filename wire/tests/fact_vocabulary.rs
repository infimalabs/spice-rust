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
//! renamed, or left unspelled without this suite saying which one. Restating
//! the list would only pin a copy against a copy.
//!
//! What is read is the `vocabulary!` table, because that table is where a kind
//! is declared: one row names the variant, its spelling, and the producer class
//! that mints it, and the enum, the spellings, and the body registry are all
//! expanded from it. Reading the expansion instead would read this crate's own
//! output back to itself, which is why the parser below looks for the table and
//! not for `pub enum FactKind` — that enum is now generated, so a parser aimed
//! at it would find no variants and prove nothing about all 68 of them.
//!
//! [`PINNED`] holds the third side, and the one neither of the others can
//! supply. Both of those read the crate under test: the match covers its enum
//! and the parser reads its source, so a renamed spelling moves the enum, the
//! source, and every expectation drawn from either of them at once, and nothing
//! fails. An oracle taken from the artifact it judges detects absence, never
//! drift. `PINNED` is written out by hand here, so a spelling that changes in
//! `wire/src/fact.rs` alone is a spelling that disagrees with an independent
//! statement of what it was agreed to be.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use wire::FactKind;

/// Every kind's wire spelling, written out here rather than read from the crate
/// under test.
///
/// The parser answers "what does `fact.rs` say today". This answers "what were
/// these spellings agreed to be", and only a deliberate edit here can change
/// that answer. Renaming `taste::confirmed` to `taste::vindicated` in the
/// vocabulary table used to pass the whole workspace suite in silence, because
/// every expectation was drawn from the file being renamed.
///
/// Silence is the expensive part. These spellings cross the socket, rest in the
/// journal, and compile into the browser app, so a rename nobody is forced to
/// notice does not fail at a version boundary — it stops matching journal rows
/// already written under the old word, which is a decoder error against durable
/// history rather than a build error against a diff.
///
/// So editing this table is the deliberate act, and the tests below exist to
/// make it impossible to take by accident.
const PINNED: &[(&str, &str)] = &[
    ("TaskAdded", "task::added"),
    ("TaskPhased", "task::phased"),
    ("TaskDone", "task::done"),
    ("TaskReviewed", "task::reviewed"),
    ("TaskAssigned", "task::assigned"),
    ("TaskNoted", "task::noted"),
    ("TaskDeferred", "task::deferred"),
    ("TaskWoken", "task::woken"),
    ("TaskEdited", "task::edited"),
    ("TaskDeleted", "task::deleted"),
    ("ClaimWon", "claim::won"),
    ("ClaimCarried", "claim::carried"),
    ("ClaimFreed", "claim::freed"),
    ("ClaimLapsed", "claim::lapsed"),
    ("ClaimRenewed", "claim::renewed"),
    ("DocApplied", "doc::applied"),
    ("SteerSent", "steer::sent"),
    ("SteerResent", "steer::resent"),
    ("SteerRecovered", "steer::recovered"),
    ("SteerAcked", "steer::acked"),
    ("SteerDeclined", "steer::declined"),
    ("SteerMissed", "steer::missed"),
    ("SteerLapsed", "steer::lapsed"),
    ("SteerDeadLettered", "steer::deadlettered"),
    ("SteerCanceled", "steer::canceled"),
    ("ConsentAsked", "consent::asked"),
    ("ConsentGranted", "consent::granted"),
    ("ConsentDenied", "consent::denied"),
    ("ConsentQueued", "consent::queued"),
    ("ConsentFloored", "consent::floored"),
    ("LaneSeated", "lane::seated"),
    ("LaneArranged", "lane::arranged"),
    ("LineageConfigured", "lineage::configured"),
    ("LineageArranged", "lineage::arranged"),
    ("LaneSessionDeclared", "lane::declared"),
    ("LaneStarted", "lane::started"),
    ("LaneImported", "lane::imported"),
    ("LaneExited", "lane::exited"),
    ("LaneCompacted", "lane::compacted"),
    ("LaneRenewed", "lane::renewed"),
    ("LaneDrained", "lane::drained"),
    ("LaneKilled", "lane::killed"),
    ("LaneDegraded", "lane::degraded"),
    ("LaneHeld", "lane::held"),
    ("LaneResumed", "lane::resumed"),
    ("TasteConfirmed", "taste::confirmed"),
    ("TasteCleared", "taste::cleared"),
    ("TasteReminded", "taste::reminded"),
    ("CanaryAlarmed", "canary::alarmed"),
    ("CanaryContradicted", "canary::contradicted"),
    ("VendorArmed", "vendor::armed"),
    ("TrustPended", "trust::pended"),
    ("TrustGranted", "trust::granted"),
    ("TrustRevoked", "trust::revoked"),
    ("CommitLanded", "commit::landed"),
    ("GateFound", "gate::found"),
    ("GateCleared", "gate::cleared"),
    ("JournalMigrated", "journal::migrated"),
    ("JournalBacked", "journal::backed"),
    ("JournalRestored", "journal::restored"),
    ("JournalIngested", "journal::ingested"),
    ("UpSeated", "up::seated"),
    ("UpQuiescing", "up::quiescing"),
    ("UpDraining", "up::draining"),
    ("UpTerminating", "up::terminating"),
    ("UpKilling", "up::killing"),
    ("UpVerifying", "up::verifying"),
    ("UpDark", "up::dark"),
    ("ProofSealed", "proof::sealed"),
    ("ReleasePublished", "release::published"),
];

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
        | FactKind::TaskEdited
        | FactKind::TaskDeleted => "task",
        FactKind::ClaimWon
        | FactKind::ClaimCarried
        | FactKind::ClaimFreed
        | FactKind::ClaimLapsed
        | FactKind::ClaimRenewed => "claim",
        FactKind::DocApplied => "doc",
        FactKind::SteerSent
        | FactKind::SteerResent
        | FactKind::SteerRecovered
        | FactKind::SteerAcked
        | FactKind::SteerDeclined
        | FactKind::SteerMissed
        | FactKind::SteerLapsed
        | FactKind::SteerDeadLettered
        | FactKind::SteerCanceled => "steer",
        FactKind::ConsentAsked
        | FactKind::ConsentGranted
        | FactKind::ConsentDenied
        | FactKind::ConsentQueued
        | FactKind::ConsentFloored => "consent",
        FactKind::LaneSeated
        | FactKind::LaneArranged
        | FactKind::LaneSessionDeclared
        | FactKind::LaneStarted
        | FactKind::LaneImported
        | FactKind::LaneExited
        | FactKind::LaneCompacted
        | FactKind::LaneRenewed
        | FactKind::LaneDrained
        | FactKind::LaneKilled
        | FactKind::LaneDegraded
        | FactKind::LaneHeld
        | FactKind::LaneResumed => "lane",
        FactKind::LineageConfigured | FactKind::LineageArranged => "lineage",
        FactKind::TasteConfirmed | FactKind::TasteCleared | FactKind::TasteReminded => "taste",
        FactKind::CanaryAlarmed | FactKind::CanaryContradicted => "canary",
        FactKind::VendorArmed => "vendor",
        FactKind::TrustPended | FactKind::TrustGranted | FactKind::TrustRevoked => "trust",
        FactKind::CommitLanded => "commit",
        FactKind::GateFound | FactKind::GateCleared => "gate",
        FactKind::JournalMigrated
        | FactKind::JournalBacked
        | FactKind::JournalRestored
        | FactKind::JournalIngested => "journal",
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

/// Read the declared kinds and their spellings out of the vocabulary table.
///
/// Reading the table rather than restating it is what lets an added, renamed,
/// or unspelled variant fail here by name. A row is `Variant = "spelling" by
/// Producer;`; the spelling stays optional here so a row that stops carrying
/// one is reported rather than skipped.
fn declared_kinds() -> Vec<Declared> {
    let source = fs::read_to_string(fact_source()).expect("wire/src/fact.rs is readable");
    let open = source
        .find("vocabulary! {")
        .expect("fact.rs declares the vocabulary table");
    let body = &source[open..];
    let close = body.find("\n}").expect("the vocabulary table closes");
    let body = &body[..close];

    let mut declared = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with(char::is_uppercase) {
            continue;
        }
        let (variant, rest) = line.split_once(' ').unwrap_or((line, ""));
        let spelling = rest
            .split_once('"')
            .and_then(|(_, quoted)| quoted.split_once('"'))
            .map(|(spelling, _)| spelling.to_owned());
        declared.push(Declared {
            variant: variant.to_owned(),
            spelling,
        });
    }

    assert!(
        declared.len() > 1,
        "no kinds parsed from the vocabulary table in wire/src/fact.rs; the \
         parser has drifted from the source",
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
    //
    // The macro grammar forecloses that today — a row is `Variant = "spelling"
    // by Producer;` and dropping the spelling fails expansion rather than this
    // assertion. Two live drifts still land here: a grammar loosened to make
    // the spelling optional, and a parser that accepts a row it cannot read a
    // spelling out of.
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

/// What the compiled vocabulary actually crosses as, keyed by variant.
///
/// The key comes from `Debug` and the value from serde, so the two halves of a
/// row are read through different derives and a rename cannot move both.
fn crossing_spellings() -> BTreeMap<String, String> {
    FactKind::ALL
        .iter()
        .map(|kind| {
            let encoded = wire::encode(kind).expect("a fact kind encodes");
            let quoted = String::from_utf8(encoded).expect("a spelling is text");
            let spelling = quoted
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("a fact kind crosses as a JSON string")
                .to_owned();
            (format!("{kind:?}"), spelling)
        })
        .collect()
}

/// One row of either table, spelled the same way so the two can be differenced.
fn rows<K: AsRef<str>, V: AsRef<str>>(pairs: impl IntoIterator<Item = (K, V)>) -> BTreeSet<String> {
    pairs
        .into_iter()
        .map(|(variant, spelling)| format!("{} = {:?}", variant.as_ref(), spelling.as_ref()))
        .collect()
}

#[test]
fn every_kind_crosses_as_the_spelling_this_table_pins() {
    let pinned: BTreeMap<&str, &str> = PINNED.iter().copied().collect();
    assert_eq!(
        pinned.len(),
        PINNED.len(),
        "this table names some variant twice, so one of its rows is unreachable",
    );

    let crossing = crossing_spellings();
    let mut drift = Vec::new();
    for (variant, crossed) in &crossing {
        match pinned.get(variant.as_str()) {
            Some(expected) if *expected == crossed.as_str() => {}
            Some(expected) => drift.push(format!(
                "{variant} was pinned as {expected:?} but now crosses as {crossed:?}"
            )),
            None => drift.push(format!(
                "{variant} crosses as {crossed:?} but this table pins no spelling for it"
            )),
        }
    }
    for variant in pinned.keys() {
        assert!(
            crossing.contains_key(*variant),
            "{variant} is pinned here but is no longer a fact kind",
        );
    }

    assert!(
        drift.is_empty(),
        "the crossing spellings no longer match the ones pinned in this file; \
         change the table only if stored facts should be read under the new \
         word:\n{}",
        drift.join("\n"),
    );
}

#[test]
fn every_pinned_spelling_still_decodes_to_its_kind() {
    // The direction durable history depends on. A journal row written under a
    // pinned spelling has to keep decoding as the kind it was written as, and a
    // rename breaks that here rather than at a read of real bytes.
    for (variant, spelling) in PINNED {
        let quoted = format!("\"{spelling}\"");
        let decoded: FactKind = wire::decode(quoted.as_bytes()).unwrap_or_else(|_| {
            panic!("{spelling:?} was pinned for {variant} but no longer decodes as any kind")
        });

        assert_eq!(
            format!("{decoded:?}"),
            *variant,
            "{spelling:?} was pinned for {variant} but now decodes as {decoded:?}",
        );
    }
}

#[test]
fn the_pinned_table_and_the_declaring_source_name_the_same_kinds() {
    // Neither list may move alone. This also hardens the parser above, whose
    // own guard only asserts that it read more than one row. A parser that
    // silently stopped reading part of the table would clear that guard, and
    // this names every row it stopped seeing. The plane check catches such a
    // drift only when the lost rows empty a whole plane, so a few rows dropped
    // from the middle of one are visible here and nowhere else.
    let declared = rows(
        declared_kinds()
            .into_iter()
            .filter_map(|kind| kind.spelling.map(|spelling| (kind.variant, spelling))),
    );
    let pinned = rows(PINNED.iter().copied());

    let only_declared: Vec<&String> = declared.difference(&pinned).collect();
    let only_pinned: Vec<&String> = pinned.difference(&declared).collect();

    assert!(
        only_declared.is_empty() && only_pinned.is_empty(),
        "the vocabulary table in wire/src/fact.rs and the pinned table in this \
         file disagree, so one was edited alone:\n  only in fact.rs: \
         {only_declared:?}\n  only in this file: {only_pinned:?}",
    );
}

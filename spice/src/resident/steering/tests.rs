use super::directive_creation;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const DIRECTIVE: &str = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d1";
const EXPIRES: &str = "2026-08-01T07:07:19.000Z";

/// Every kind that opens a lineage has to be findable as a creating fact.
///
/// `directive_creation` ends in `expect`, which reads a missing creating fact as
/// impossible rather than as absent. `pending_directive` calls it for every
/// pending directive while rendering the inbox, so a reminder the ledger admits
/// but this cannot find is not a blank field -- it takes the resident down on
/// publication, and again on cancel and recover.
///
/// This is the falsifier for that: it fails by panic, not by assertion, if the
/// `taste::reminded` arm is dropped.
#[test]
fn a_reminder_is_findable_as_the_fact_that_created_its_directive() {
    let snapshot = snapshot_of(&[reminded_bytes(1, "1kJK6N3R")]);
    let directive: wire::CanonicalUuid = DIRECTIVE.parse().expect("fixture directive");

    let envelope = directive_creation(&snapshot, directive);

    assert_eq!(envelope.seq.get(), 1);
    assert_eq!(envelope.fact.kind(), wire::FactKind::TasteReminded);
}

/// The lookup is keyed on identity, not merely on carrying the right kind.
#[test]
fn another_reminders_identity_is_not_mistaken_for_this_one() {
    let snapshot = snapshot_of(&[reminded_bytes(1, "1kJK6N3R"), reminded_bytes(2, "1kJK6M0V")]);
    let second: wire::CanonicalUuid = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d2"
        .parse()
        .expect("fixture directive");

    let envelope = directive_creation(&snapshot, second);

    assert_eq!(envelope.seq.get(), 2);
}

fn snapshot_of(facts: &[String]) -> journal::FactSnapshot {
    let decoded = facts
        .iter()
        .map(|text| wire::decode(text.as_bytes()).expect("registered envelope"))
        .collect();
    journal::FactSnapshot::from_facts(decoded).expect("fixture snapshot")
}

/// One `taste::reminded` envelope as the journal stores it.
fn reminded_bytes(seq: u64, key: &str) -> String {
    let content = wire::Digest::compute(wire::Domain::TasteProse, b"we will poll until it settles");
    format!(
        concat!(
            r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"spice","#,
            r#""origin":"resident:e2e9f1f2-19b2-4ddc-83fa-340b02dee2c4","#,
            r#""fact":{{"kind":"taste::reminded","#,
            r#""directive":"a0ae31a7-eb68-463a-b3c0-272bfeaaa5d{}","#,
            r#""key":"{}","lane":"{}","taste":"polling","content":"{}","#,
            r#""occupant":"{}","compaction":0,"#,
            r#""locus":{{"member":1,"offset":40,"length":80,"event":0,"line":3,"column":1}},"#,
            r#""text":"polling is not a completion event","expires":"{}"}}}}"#
        ),
        seq,
        seq,
        seq,
        key,
        LANE,
        content.as_str(),
        OCCUPANT,
        EXPIRES
    )
}

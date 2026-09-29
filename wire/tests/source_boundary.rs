//! The forward migration that gives a stored lifecycle fact a source boundary.
//!
//! The journal is append-only, so a fact already written can never grow the
//! field and decode is the only place the migration can happen. That makes the
//! decode arm load-bearing rather than incidental: every lifecycle fact already
//! in the fleet passes through it. Each carrier is asserted separately, because
//! the attribute is hand-applied per field and one carrier omitting it would
//! still leave the others green.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::{Actor, CanonicalUuid, Instant, TaskStamp, TranscriptSourceHead, body};

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("fixture uuid")
}

fn stamp() -> TaskStamp {
    "1kKRRsDP".parse().expect("fixture task stamp")
}

fn instant(text: &str) -> Instant {
    Instant::new(text.parse().expect("fixture timestamp")).expect("fixture instant")
}

/// A proven boundary carrying one accounted head.
///
/// Deliberately not an empty `Proven`: an empty one is byte-identical for every
/// lane, so a carrier that dropped its stored boundary entirely would still
/// decode equal to this fixture and the loss would not show.
fn proven() -> body::SourceBoundary {
    body::SourceBoundary::Proven {
        heads: vec![TranscriptSourceHead {
            lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            vendor: "omp".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid("018f82d4-6f59-7d06-92e3-bce913116303"),
            next: body::TranscriptPosition {
                member: 4,
                offset: 512,
            },
        }],
    }
}

fn claim_won() -> body::ClaimWon {
    body::ClaimWon {
        task: stamp(),
        actor: Actor::Operator,
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116305"),
        cell: Some(wire::ClaimCell {
            project: "task.delivery".to_owned(),
            phase: "todo".parse().expect("phase"),
        }),
        until: instant("2030-04-12T18:44:09.271828Z"),
        git: None,
        recovery: None,
        source: proven(),
    }
}

fn task_done() -> body::TaskDone {
    body::TaskDone {
        task: stamp(),
        validation: "the fixture validated".to_owned(),
        terminal: None,
        git: None,
        source: proven(),
    }
}

fn task_reviewed() -> body::TaskReviewed {
    body::TaskReviewed {
        task: stamp(),
        reviewer: Actor::Operator,
        verdict: body::Verdict::Clean,
        finding: None,
        git: None,
        source: proven(),
    }
}

fn claim_carried() -> body::ClaimCarried {
    body::ClaimCarried {
        task: stamp(),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        ancestor: uuid("018f82d4-6f59-7d06-92e3-bce913116306"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116305"),
        source: proven(),
    }
}

/// Bodies exactly as a store written before boundaries existed holds them.
///
/// These are literal, not pruned from a live fixture: they are the durable
/// shape the migration exists to keep readable, and a fixture derived from
/// today's types would follow the types if the field were ever renamed away.
const CLAIM_WON: &str = r#"{
  "kind": "claim::won",
  "task": "1kKRRsDP",
  "actor": "operator",
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "occupant": "018f82d4-6f59-7d06-92e3-bce913116304",
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116305",
  "until": "2030-04-12T18:44:09.271Z"
}"#;

const TASK_DONE: &str = r#"{
  "kind": "task::done",
  "task": "1kKRRsDP",
  "validation": "the fixture validated"
}"#;

const TASK_REVIEWED: &str = r#"{
  "kind": "task::reviewed",
  "task": "1kKRRsDP",
  "reviewer": "operator",
  "verdict": "clean"
}"#;

const CLAIM_CARRIED: &str = r#"{
  "kind": "claim::carried",
  "task": "1kKRRsDP",
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "occupant": "018f82d4-6f59-7d06-92e3-bce913116304",
  "ancestor": "018f82d4-6f59-7d06-92e3-bce913116306",
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116305"
}"#;

const LANE_ARRANGED: &str = r#"{
  "kind": "lane::arranged",
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "from_team": "018f82d4-6f59-7d06-92e3-bce913116307",
  "team": "018f82d4-6f59-7d06-92e3-bce913116308",
  "before": "018f82d4-6f59-7d06-92e3-bce913116304"
}"#;

const TEAM_ARRANGED: &str = r#"{
  "kind": "team::arranged",
  "from": [],
  "teams": []
}"#;

/// Read a stored body back through the registry the journal decodes with.
fn registered<F: wire::Body>(document: &str) -> F {
    let fact: wire::AnyFact =
        wire::decode(document.as_bytes()).expect("a fact stored before boundaries is admitted");
    fact.read().expect("the fixture names its own kind")
}

/// Decode a fact from the bytes it would have carried before the field existed.
///
/// Pruning the key out of the encoded fixture rather than hand-writing a legacy
/// document keeps the fixture bound to the live shape: every other field stays
/// exactly what a producer writes today, so the one difference under test is the
/// absence the migration exists for. The removal is asserted because a fixture
/// that never carried the key would prove nothing by losing it.
fn stored_before_boundaries<T: Serialize + DeserializeOwned>(fact: &T) -> T {
    let mut document: serde_json::Value =
        serde_json::from_slice(&wire::encode(fact).expect("fixture encodes"))
            .expect("encoded fixture is json");
    assert!(
        document
            .as_object_mut()
            .expect("fact is a json object")
            .remove("source")
            .is_some(),
        "the fixture carried no source key, so removing it proved nothing"
    );
    wire::decode(&serde_json::to_vec(&document).expect("pruned fact encodes"))
        .expect("a fact stored before boundaries decodes")
}

/// Each carrier is named on its own, so one field losing the attribute reds.
///
/// The expected value is built from the literal reason rather than from
/// [`body::SourceBoundary::unrecorded`], because comparing the migration against
/// itself passes for whatever the migration happens to produce -- including an
/// empty `Proven`, which is the one answer it must never give.
#[test]
fn every_carrier_stored_before_boundaries_decodes_to_the_named_migration() {
    let migrated = body::SourceBoundary::Incomplete {
        reason: "recorded before task lifecycle facts carried source boundaries".to_owned(),
    };
    assert_eq!(
        body::SourceBoundary::unrecorded(),
        migrated,
        "the migrated reason no longer names the shape it was decoded from"
    );

    assert_eq!(stored_before_boundaries(&claim_won()).source, migrated);
    assert_eq!(stored_before_boundaries(&task_done()).source, migrated);
    assert_eq!(stored_before_boundaries(&task_reviewed()).source, migrated);
    assert_eq!(stored_before_boundaries(&claim_carried()).source, migrated);
}

/// The migration fires on absence, not on every decode.
///
/// Without this the assertion above would hold just as well for a decoder that
/// threw away whatever was stored, which is the opposite of a migration.
#[test]
fn a_carrier_that_stored_a_boundary_keeps_it_instead_of_migrating() {
    let decoded: body::ClaimWon =
        wire::decode(&wire::encode(&claim_won()).expect("fixture encodes")).expect("fact decodes");

    assert_eq!(decoded.source, proven());
    assert_ne!(decoded.source, body::SourceBoundary::unrecorded());
}

/// The two states a reader must never confuse do not encode to the same bytes.
///
/// A lane whose transcript has not been accounted yet opens at zero heads, which
/// is a real boundary. A fact predating boundaries has no answer at all. Were
/// the migration to produce an empty `Proven`, a reducer would read *no usage*
/// where the truth is *unknown usage*, and every such fact would silently claim
/// its phase spent nothing.
#[test]
fn a_migrated_boundary_does_not_read_as_an_empty_proven_one() {
    let opened = body::ClaimWon {
        source: body::SourceBoundary::Proven { heads: Vec::new() },
        ..claim_won()
    };

    assert_ne!(
        wire::encode(&stored_before_boundaries(&claim_won())).expect("migrated fact encodes"),
        wire::encode(&opened).expect("opened fact encodes"),
        "a lane accounted at zero and a fact predating boundaries encoded alike"
    );
}

/// Every carrier already in a journal still decodes through the registered DTO.
///
/// This is the door the store actually uses: `journal` reads facts as
/// [`wire::AnyFact`], whose admission runs the registered body across the
/// supplied fields and refuses any body that does not re-encode to exactly
/// them. A migration that wrote the migrated value back out would satisfy the
/// direct decode above and still make every stored lifecycle fact undecodable
/// here, so asserting only on the body type would prove the wrong thing.
#[test]
fn every_carrier_stored_before_boundaries_still_reads_through_the_registry() {
    let migrated = body::SourceBoundary::unrecorded();

    assert_eq!(registered::<body::ClaimWon>(CLAIM_WON).source, migrated);
    let legacy_done = registered::<body::TaskDone>(TASK_DONE);
    assert_eq!(legacy_done.source, migrated);
    assert_eq!(legacy_done.terminal, None);
    assert_eq!(
        registered::<body::TaskReviewed>(TASK_REVIEWED).source,
        migrated
    );
    assert_eq!(
        registered::<body::ClaimCarried>(CLAIM_CARRIED).source,
        migrated
    );
}

/// A migrated fact encodes back to exactly the bytes it arrived as.
///
/// The journal is append-only, so the stored bytes are the only ones that will
/// ever exist for these facts. Re-encoding to anything else is what the registry
/// refuses, and it is also what would strand durable data behind a decoder that
/// can read it but never hand it on.
#[test]
fn a_migrated_boundary_encodes_back_to_the_bytes_it_arrived_as() {
    let supplied: serde_json::Value =
        serde_json::from_str(CLAIM_WON).expect("legacy fixture is json");
    let fact: wire::AnyFact = wire::decode(CLAIM_WON.as_bytes()).expect("legacy fact decodes");

    assert_eq!(
        serde_json::to_value(&fact).expect("decoded fact re-encodes"),
        supplied,
        "a stored fact did not survive a decode and re-encode unchanged"
    );
}

/// A membership transition stored before boundaries still reads, and says so.
///
/// These three carry a list rather than one boundary because a rearrangement
/// names no lane of its own, and the list is the same shape for all three so a
/// reducer reads one thing. An empty list is the migrated state and also the
/// live state when no claimed lane moved; both mean the transition splits no
/// attribution, so the reducer takes the same action either way.
#[test]
fn membership_transitions_stored_before_boundaries_read_with_no_lanes() {
    assert!(
        registered::<body::LaneArranged>(LANE_ARRANGED)
            .sources
            .is_empty()
    );
    assert!(
        registered::<body::TeamArranged>(TEAM_ARRANGED)
            .sources
            .is_empty()
    );
}

/// A recorded membership boundary survives the registry unchanged.
///
/// The empty case above passes for a field the registry silently dropped, so a
/// populated one has to prove the boundary makes the round trip.
#[test]
fn a_recorded_membership_boundary_survives_the_registry() {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce913116302");
    let arranged = body::LaneArranged {
        lane,
        from_team: uuid("018f82d4-6f59-7d06-92e3-bce913116307"),
        team: uuid("018f82d4-6f59-7d06-92e3-bce913116308"),
        before: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        sources: vec![body::LaneSourceBoundary {
            lane,
            source: proven(),
        }],
    };
    let fact = wire::Fact::new(arranged.clone());

    let decoded: wire::AnyFact =
        wire::decode(&wire::encode(&fact).expect("fact encodes")).expect("fact is admitted");

    assert_eq!(
        decoded
            .read::<body::LaneArranged>()
            .expect("the registered kind")
            .sources,
        arranged.sources
    );
}

/// Renewal extends a lease inside one interval, so it closes no boundary.
///
/// Asserting on the encoded fact rather than on the type keeps the claim
/// falsifiable: adding the field to a renewal later reds this, rather than
/// silently widening what a renewal is taken to have proven.
#[test]
fn a_renewal_records_no_boundary_at_all() {
    let document: serde_json::Value = serde_json::from_slice(
        &wire::encode(&body::ClaimRenewed {
            task: stamp(),
            epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116305"),
            until: instant("2030-04-12T19:44:09.271828Z"),
            maximum: instant("2030-04-12T23:44:09.271828Z"),
        })
        .expect("renewal encodes"),
    )
    .expect("encoded renewal is json");

    assert!(
        document.get("source").is_none(),
        "a renewal recorded a boundary it does not close"
    );
}

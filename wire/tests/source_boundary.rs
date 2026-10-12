//! Current task source boundaries preserve attributed transcript heads across wire encoding.

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

#[test]
fn a_claim_boundary_survives_the_current_wire_round_trip() {
    let decoded: body::ClaimWon =
        wire::decode(&wire::encode(&claim_won()).expect("fixture encodes")).expect("fact decodes");

    assert_eq!(decoded.source, proven());
    assert_ne!(decoded.source, body::SourceBoundary::unrecorded());
}

#[test]
fn a_membership_boundary_survives_the_current_registry_round_trip() {
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

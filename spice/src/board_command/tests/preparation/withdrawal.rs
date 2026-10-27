//! Paste-ready contribution sources, reference scope at candidate retention,
//! and the author's withdrawal of an undecided candidate.

use super::contribution::{contribute, contributed, contribution_body};
use super::*;

use wire::rooms::{CandidateWithdraw, CandidateWithdrawal, RoomContributionKind};

/// Retain `micros` under `controller` citing `sources`, witnessed as the
/// retain client witnesses it.
fn retain_citing(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    controller: &TaskHandle,
    output: &TaskHandle,
    micros: i64,
    sources: Vec<RoomSource>,
) -> Outcome<CommandAnswer> {
    let current = board(bus, epoch, Actor::Lane(seat.lane));
    let candidate = CandidateRef {
        controller: controller.born().expect("controller stamp"),
        candidate: RoomAuthorityStamp::for_micros(micros).expect("candidate identity"),
    };
    let mut expected = transition_witnesses(at(&current, controller));
    expected.extend(transition_witnesses(at(&current, output)));
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: None,
    });
    expected.push(residency(&current, seat));
    issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: DesignPassProposal {
                candidate,
                statement: at(&current, controller).statement.position,
                claim: Some(
                    at(&current, controller)
                        .claim
                        .as_ref()
                        .expect("Design is claimed before proposing a pass")
                        .position,
                ),
                recommendation: wire::rooms::DesignPassRecommendation::Work,
                governed_inputs: Vec::new(),
                outputs: vec![output.born().expect("output stamp")],
                justification: "the cited evidence supports this pass".to_owned(),
                references: sources
                    .into_iter()
                    .map(|source| CandidateReference {
                        relation: "evidence".parse().expect("relation"),
                        source,
                    })
                    .collect(),
            },
        },
    )
}

/// Withdraw `candidate` as `actor`, witnessed as the withdraw client
/// witnesses it.
pub(super) fn withdraw(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    controller: &TaskHandle,
    candidate: &CandidateRef,
    reason: &str,
) -> Outcome<CommandAnswer> {
    let current = board(bus, epoch, actor.clone());
    let withdrawal = ProposalRef::Withdrawal {
        candidate: candidate.clone(),
    };
    let head = bus
        .read({
            let withdrawal = withdrawal.clone();
            move |read| read.proposal_head(&withdrawal)
        })
        .expect("withdrawal head reads")
        .map(|head| head.position());
    let mut expected = transition_witnesses(at(&current, controller));
    expected.push(candidate_witness(bus, candidate));
    expected.push(Witness::Proposal {
        proposal: withdrawal,
        head,
    });
    issue(
        bus,
        epoch,
        actor,
        CommandKind::CandidateWithdraw,
        expected,
        &CandidateWithdraw {
            withdrawal: CandidateWithdrawal {
                candidate: candidate.clone(),
                reason: reason.to_owned(),
            },
        },
    )
}

fn refusal_of(outcome: &Outcome<CommandAnswer>) -> (&str, Option<&str>) {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    (
        refusal.reason(),
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
    )
}

#[test]
fn a_contribution_answer_is_the_source_retain_accepts_only_under_its_own_design() {
    let (root, bus, epoch, seats) = fixture("preparation-contribution-source", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(&bus, epoch, "Cited design", wire::TaskIntent::Design, None);
    capture_design(&bus, epoch, &seats[0], &design);
    let output = add_plan_output(&bus, epoch, &design, "Cited output");
    let other = add_controller(&bus, epoch, "Other design", wire::TaskIntent::Design, None);
    let evidence = |room: &TaskHandle, sources| {
        contributed(contribute(
            &bus,
            epoch,
            root.as_ref(),
            lane.clone(),
            contribution_body(room, RoomContributionKind::charge(1), "Measured.", sources),
        ))
    };
    let answer = evidence(&design, Vec::new());
    let foreign = evidence(&other, Vec::new());

    let encoded = serde_json::to_value(&answer).expect("answer encodes");
    let pasted: RoomSource =
        serde_json::from_value(encoded["source"].clone()).expect("the answer carries a source");
    assert_eq!(
        pasted,
        RoomSource::Contribution {
            room: design.born().expect("design stamp"),
            publication: answer.publication.clone(),
        }
    );
    let cited = evidence(&design, vec![pasted.clone()]);
    assert_ne!(cited.publication, answer.publication);
    let invented = RoomSource::Contribution {
        room: design.born().expect("design stamp"),
        publication: RoomAuthorityStamp::for_micros(1_760_000_000_000_999)
            .expect("unpublished identity"),
    };
    let unavailable = contribute(
        &bus,
        epoch,
        root.as_ref(),
        lane.clone(),
        contribution_body(
            &design,
            RoomContributionKind::charge(1),
            "Cites nothing real.",
            vec![invented.clone()],
        ),
    );
    assert_eq!(
        refusal_of(&unavailable).0,
        "room::contribution-source-unavailable"
    );

    let current = board(&bus, epoch, Actor::Operator);
    let artifact = RoomSource::Artifact {
        digest: "0".repeat(64).parse().expect("artifact digest"),
    };
    let missing = RoomSource::Journal {
        fact: wire::SequenceCode::new(1_000_000),
    };
    let cases = [
        (
            vec![foreign.source.clone()],
            "preparation::reference-out-of-scope",
        ),
        (
            vec![pasted.clone(), journal_source(&current, &other)],
            "preparation::reference-out-of-scope",
        ),
        (vec![missing], "preparation::reference-missing"),
        (vec![invented], "preparation::reference-missing"),
    ];
    for (index, (sources, reason)) in (1_760_000_000_000_010..).zip(cases) {
        let offending = serde_json::to_string(sources.last().expect("one offending source"))
            .expect("source encodes");
        let outcome = retain_citing(&bus, epoch, &seats[0], &design, &output, index, sources);
        assert_eq!(refusal_of(&outcome), (reason, Some(offending.as_str())));
    }
    assert_committed(retain_citing(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        1_760_000_000_000_020,
        vec![
            pasted,
            journal_source(&current, &design),
            journal_source(&current, &output),
            artifact,
        ],
    ));
    clean(&root, bus);
}

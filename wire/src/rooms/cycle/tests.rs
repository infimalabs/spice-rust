use super::*;
use crate::body::{DesignConclusion, TaskCompletionTerminal};
use crate::rooms::{
    CandidateReference, CandidateWithdrawal, DecisionRef, DesignPassProposal,
    DesignPassRecommendation, ExecutionAuthorization, OutputDecision, PreparationDecision,
    QuestionResultRef, RoomAuthorityStamp, RoomContributionKind, RoomRoute, RoomSource,
    TaskPreparation,
};
use crate::{Fact, Instant, Name, SequenceCode, TaskStamp};

fn task(micros: i64) -> TaskStamp {
    TaskStamp::for_micros(micros).expect("fixture task")
}

fn lane(last: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{last:012}")
        .parse()
        .expect("fixture lane")
}

fn authority(micros: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(1_760_000_000_000_000 + micros).expect("authority stamp")
}

fn instant() -> Instant {
    "2026-10-16T00:00:00.000Z".parse().expect("fixture instant")
}

fn any<F: crate::Body>(body: F) -> AnyFact {
    crate::decode(&crate::encode(&Fact::new(body)).expect("fixture encodes"))
        .expect("fixture decodes as a stored fact")
}

const DESIGN: i64 = 1;
const OTHER: i64 = 2;
const OUTPUT: i64 = 3;
const SECOND: i64 = 4;

fn design() -> TaskStamp {
    task(DESIGN)
}

fn candidate(micros: i64) -> CandidateRef {
    CandidateRef {
        controller: design(),
        candidate: authority(micros),
    }
}

fn contributed(room: i64, by: u8, kind: RoomContributionKind) -> AnyFact {
    any(RoomContributed {
        room: task(room),
        publication: authority(1_000 + i64::from(by)),
        lane: lane(by),
        contribution: kind,
        statement: None,
        text: "Substantive contribution.".to_owned(),
        sources: Vec::new(),
    })
}

fn posted(operation: RoomPublicationKind, routes: Vec<RoomRoute>) -> AnyFact {
    any(RoomPosted {
        publication: authority(2_000),
        text: "Operator input.".to_owned(),
        operation,
        routes,
        directed: Vec::new(),
        expires: instant(),
    })
}

fn route(destination: RoomDestination, context: Option<TaskStamp>) -> RoomRoute {
    RoomRoute {
        destination,
        context,
        direction: None,
    }
}

fn result() -> QuestionResultRef {
    serde_json::from_value(serde_json::to_value(authority(3_000)).expect("stamp"))
        .expect("question result")
}

fn published(content: RoomPublicationContent) -> AnyFact {
    any(RoomPublished {
        room: design(),
        publication: authority(4_000),
        content,
    })
}

fn proposed(micros: i64, outputs: Vec<TaskStamp>) -> AnyFact {
    let recommendation = if outputs.is_empty() {
        DesignPassRecommendation::Conclude
    } else {
        DesignPassRecommendation::Work
    };
    any(DesignProposed {
        candidate: DesignPassProposal {
            candidate: candidate(micros),
            statement: Seq::new(1).expect("statement"),
            claim: Seq::new(2),
            recommendation,
            governed_inputs: Vec::new(),
            outputs,
            justification: "the cited contract supports this pass".to_owned(),
            references: vec![CandidateReference {
                relation: "contract".parse::<Name>().expect("name"),
                source: RoomSource::Journal {
                    fact: SequenceCode::new(1),
                },
            }],
            evidence: Vec::new(),
        },
    })
}

fn decided(sequence: u64, outcome: DecisionOutcome) -> AnyFact {
    any(PreparationDecided {
        decision: PreparationDecision {
            decision: DecisionRef {
                controller: design(),
                decision: SequenceCode::new(sequence),
            },
            source: RoomSource::Journal {
                fact: SequenceCode::new(1),
            },
            outcome,
        },
    })
}

fn resolved(micros: i64, verdicts: &[(i64, OutputVerdict)]) -> DecisionOutcome {
    DecisionOutcome::Resolved {
        candidate: candidate(micros),
        outputs: verdicts
            .iter()
            .map(|&(output, verdict)| OutputDecision {
                output: task(output),
                verdict,
                comment: None,
            })
            .collect(),
        comment: None,
    }
}

fn authorized(sequence: u64, micros: i64, outputs: &[i64]) -> AnyFact {
    any(ExecutionAuthorized {
        authorization: ExecutionAuthorization {
            decision: DecisionRef {
                controller: design(),
                decision: SequenceCode::new(sequence),
            },
            candidate: candidate(micros),
            governed_inputs: Vec::new(),
            selected_outputs: outputs.iter().copied().map(task).collect(),
            coverage: Vec::new(),
        },
    })
}

fn freed(subject: i64) -> AnyFact {
    any(ClaimFreed {
        task: task(subject),
        actor: Actor::Lane(lane(1)),
        parked: None,
    })
}

fn completed(subject: i64) -> AnyFact {
    any(TaskDone {
        task: task(subject),
        validation: "checked".to_owned(),
        terminal: Some(TaskCompletionTerminal {
            operation: lane(9),
            status: TaskCompletionStatus::Completed,
            next: None,
        }),
        git: None,
        source: crate::body::SourceBoundary::Proven { heads: Vec::new() },
    })
}

/// A fold driven in journal order, numbering facts from 1.
struct Journal {
    cycles: DesignCycles,
    next: u64,
}

impl Journal {
    fn new() -> Self {
        Self {
            cycles: DesignCycles::default(),
            next: 1,
        }
    }

    fn by(&mut self, actor: &Actor, fact: &AnyFact) -> &mut Self {
        let seq = Seq::new(self.next).expect("seq");
        self.next += 1;
        assert!(
            DesignCycles::concerns(fact.kind()),
            "the cheap filter admits every kind the fold reads"
        );
        self.cycles.observe(seq, actor, fact);
        self
    }

    fn lane(&mut self, fact: &AnyFact) -> &mut Self {
        self.by(&Actor::Lane(lane(1)), fact)
    }

    fn operator(&mut self, fact: &AnyFact) -> &mut Self {
        self.by(&Actor::Human(lane(200)), fact)
    }

    fn cycle(&self, working: bool) -> DesignCycle {
        self.cycles.facts(&design()).cycle(working)
    }

    /// One completed pass: proposal, release, and a decision.
    fn passed(&mut self, outcome: DecisionOutcome, outputs: Vec<TaskStamp>) -> &mut Self {
        self.lane(&proposed(10, outputs))
            .lane(&freed(DESIGN))
            .operator(&decided(1, outcome))
    }
}

fn steeping(journal: &mut Journal, lanes: u8) {
    let kinds = [
        RoomContributionKind::ReasonFor,
        RoomContributionKind::ReasonAgainst,
        RoomContributionKind::Evidence,
    ];
    for by in 1..=lanes {
        journal.lane(&contributed(DESIGN, by, kinds[usize::from(by - 1) % 3]));
    }
}

#[test]
fn first_pass_needs_steeped_maturity_or_a_nudge() {
    let mut journal = Journal::new();
    steeping(&mut journal, 2);
    assert_eq!(journal.cycle(false), DesignCycle::GatheringInput);
    steeping(&mut journal, 3);
    assert_eq!(journal.cycle(false), DesignCycle::ReadyForProposal);

    let mut nudged = Journal::new();
    nudged.lane(&contributed(DESIGN, 1, RoomContributionKind::Evidence));
    assert_eq!(nudged.cycle(false), DesignCycle::GatheringInput);
    nudged.operator(&any(DesignNudged { design: design() }));
    assert_eq!(nudged.cycle(false), DesignCycle::ReadyForProposal);
}

#[test]
fn only_counted_input_after_the_horizon_refills_a_design() {
    let accepted = || DecisionOutcome::Accepted {
        candidate: candidate(10),
    };
    let task_route = || vec![route(RoomDestination::Task { task: design() }, None)];
    let context_route = || {
        vec![route(
            RoomDestination::Lane { lane: lane(2) },
            Some(design()),
        )]
    };
    let counted: Vec<(&str, bool, AnyFact)> = vec![
        (
            "contribution",
            false,
            contributed(DESIGN, 2, RoomContributionKind::Evidence),
        ),
        ("nudge", true, any(DesignNudged { design: design() })),
        (
            "task-routed MSG",
            true,
            posted(RoomPublicationKind::Msg {}, task_route()),
        ),
        (
            "context MSG",
            true,
            posted(RoomPublicationKind::Msg {}, context_route()),
        ),
        (
            "answer context",
            true,
            posted(
                RoomPublicationKind::Ans {
                    result: result(),
                    context: Some(design()),
                },
                Vec::new(),
            ),
        ),
        (
            "authored publication",
            true,
            published(RoomPublicationContent::Authored {
                text: "Operator note.".to_owned(),
            }),
        ),
    ];
    for (name, operator, fact) in counted {
        let mut journal = Journal::new();
        steeping(&mut journal, 3);
        journal.passed(accepted(), Vec::new());
        assert_eq!(journal.cycle(false), DesignCycle::GatheringInput, "{name}");
        if operator {
            journal.operator(&fact);
        } else {
            journal.lane(&fact);
        }
        assert_eq!(
            journal.cycle(false),
            DesignCycle::ReadyForProposal,
            "{name}"
        );
    }

    let inert: Vec<(&str, bool, AnyFact)> = vec![
        (
            "operator ASK",
            true,
            posted(
                RoomPublicationKind::Ask {
                    result: result(),
                    deadline: instant(),
                },
                task_route(),
            ),
        ),
        (
            "lane-authored MSG",
            false,
            posted(RoomPublicationKind::Msg {}, task_route()),
        ),
        (
            "linked publication",
            true,
            published(RoomPublicationContent::Linked {
                link: RoomSource::Journal {
                    fact: SequenceCode::new(1),
                },
            }),
        ),
        (
            "another room",
            false,
            contributed(OTHER, 2, RoomContributionKind::Evidence),
        ),
        ("unrelated release", false, freed(OTHER)),
        ("unrelated completion", false, completed(OTHER)),
    ];
    for (name, operator, fact) in inert {
        let mut journal = Journal::new();
        steeping(&mut journal, 3);
        journal.passed(accepted(), Vec::new());
        if operator {
            journal.operator(&fact);
        } else {
            journal.lane(&fact);
        }
        assert_eq!(journal.cycle(false), DesignCycle::GatheringInput, "{name}");
    }
}

#[test]
fn an_undecided_proposal_holds_selection_until_decided_or_withdrawn() {
    let mut journal = Journal::new();
    steeping(&mut journal, 3);
    journal.lane(&proposed(10, vec![task(OUTPUT)]));
    journal.lane(&freed(DESIGN));
    journal.lane(&contributed(DESIGN, 2, RoomContributionKind::Evidence));
    assert_eq!(journal.cycle(false), DesignCycle::AwaitingDecision);
    assert_eq!(
        journal.cycles.facts(&design()).end,
        Some(DesignPassEnd::Proposed)
    );

    let withdrawn = |micros| {
        any(CandidateWithdrawn {
            withdrawal: CandidateWithdrawal {
                candidate: candidate(micros),
                reason: "superseded".to_owned(),
            },
        })
    };
    journal.lane(&withdrawn(10));
    assert_eq!(journal.cycle(false), DesignCycle::ReadyForProposal);

    let mut stale = Journal::new();
    stale.lane(&proposed(10, vec![task(OUTPUT)]));
    stale.lane(&proposed(11, vec![task(OUTPUT)]));
    stale.operator(&decided(
        1,
        resolved(10, &[(OUTPUT, OutputVerdict::Accepted)]),
    ));
    assert_eq!(
        stale.cycle(false),
        DesignCycle::AwaitingDecision,
        "a decision of an older proposal leaves the latest one undecided"
    );

    let mut shadowed = Journal::new();
    shadowed.lane(&proposed(10, vec![task(OUTPUT)]));
    shadowed.lane(&proposed(11, vec![task(OUTPUT)]));
    shadowed.lane(&withdrawn(11));
    assert_eq!(
        shadowed.cycle(false),
        DesignCycle::AwaitingDecision,
        "withdrawing a later proposal leaves an earlier undecided one owed"
    );
}

#[test]
fn decisions_wait_on_released_work_unless_feedback_or_later_input_arrives() {
    let outputs = || vec![task(OUTPUT), task(SECOND)];
    let mut accepted = Journal::new();
    accepted.passed(
        resolved(
            10,
            &[
                (OUTPUT, OutputVerdict::Accepted),
                (SECOND, OutputVerdict::Accepted),
            ],
        ),
        outputs(),
    );
    accepted.operator(&authorized(1, 10, &[OUTPUT, SECOND]));
    assert_eq!(accepted.cycle(true), DesignCycle::WaitingOnSelectedWork);
    accepted.lane(&completed(OUTPUT));
    assert_eq!(
        accepted.cycle(true),
        DesignCycle::WaitingOnSelectedWork,
        "a partial settlement leaves the remaining selected work running"
    );
    accepted.lane(&completed(SECOND));
    assert_eq!(accepted.cycle(false), DesignCycle::ReadyForProposal);

    let mut contributed_while_running = Journal::new();
    contributed_while_running.passed(
        resolved(10, &[(OUTPUT, OutputVerdict::Accepted)]),
        vec![task(OUTPUT)],
    );
    contributed_while_running.operator(&authorized(1, 10, &[OUTPUT]));
    contributed_while_running.lane(&contributed(DESIGN, 2, RoomContributionKind::ReasonAgainst));
    assert_eq!(
        contributed_while_running.cycle(true),
        DesignCycle::ReadyForProposal
    );

    for (name, outcome) in [
        (
            "mixed",
            resolved(
                10,
                &[
                    (OUTPUT, OutputVerdict::Accepted),
                    (SECOND, OutputVerdict::Rejected),
                ],
            ),
        ),
        (
            "all rejected",
            resolved(
                10,
                &[
                    (OUTPUT, OutputVerdict::Rejected),
                    (SECOND, OutputVerdict::Rejected),
                ],
            ),
        ),
        (
            "returned",
            DecisionOutcome::Returned {
                candidate: candidate(10),
                reason: "cover the failure path".to_owned(),
            },
        ),
    ] {
        let mut journal = Journal::new();
        journal.passed(outcome, outputs());
        assert_eq!(journal.cycle(true), DesignCycle::ReadyForProposal, "{name}");
    }
}

#[test]
fn abandonment_moves_the_horizon_and_settlement_drops_the_fold() {
    let mut journal = Journal::new();
    steeping(&mut journal, 3);
    journal.lane(&freed(DESIGN));
    let facts = journal.cycles.facts(&design());
    assert_eq!(facts.end, Some(DesignPassEnd::Abandoned));
    assert_eq!(
        journal.cycle(false),
        DesignCycle::GatheringInput,
        "maturity reached before an abandoned pass is spent by it"
    );
    journal.by(
        &Actor::Spice,
        &any(ClaimLapsed {
            task: design(),
            occupant: lane(1),
            expiry: instant(),
        }),
    );
    assert_eq!(
        journal.cycles.facts(&design()).end,
        Some(DesignPassEnd::Abandoned)
    );

    journal.operator(&any(TaskConcluded {
        task: design(),
        conclusion: DesignConclusion::Done {},
    }));
    assert_eq!(journal.cycles.facts(&design()), DesignCycleFacts::default());
    assert!(journal.cycles.designs.is_empty());
}

#[test]
fn plan_constructed_work_settles_into_its_controlling_design() {
    let mut journal = Journal::new();
    journal.passed(
        DecisionOutcome::Accepted {
            candidate: candidate(10),
        },
        Vec::new(),
    );
    journal.operator(&any(OutputControlled {
        output: task(OUTPUT),
        preparation: TaskPreparation {
            parent: task(OTHER),
            controller: design(),
            construction: task(OUTPUT),
        },
    }));
    assert_eq!(journal.cycle(true), DesignCycle::WaitingOnSelectedWork);
    journal.operator(&any(TaskClosed {
        task: task(OUTPUT),
        validation: "done elsewhere".to_owned(),
        phase: "todo".parse().expect("phase"),
        claimed: None,
    }));
    assert_eq!(journal.cycle(false), DesignCycle::ReadyForProposal);
}

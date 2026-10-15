//! One candidate decision as immutable room history and, when it carries
//! feedback, as the active Design holder's next input.

use std::collections::BTreeMap;

use serde_json::Value;

use super::contribution::history;
use super::*;
use crate::board_command::{owed_returns, record_return};

struct Returnable {
    root: FixtureRoot,
    bus: Bus,
    epoch: CanonicalUuid,
    seats: Vec<Seat>,
    design: TaskHandle,
    outputs: [TaskHandle; 2],
    candidate: CandidateRef,
    source: RoomSource,
}

impl Returnable {
    fn new(name: &str, lanes: usize, holder: Option<usize>) -> Self {
        let (root, bus, epoch, seats) = fixture(name, lanes);
        let design = add_controller(&bus, epoch, name, wire::TaskIntent::Design, None);
        if let Some(holder) = holder {
            group_design(&bus, epoch, &design);
            let seat = &seats[holder];
            claim_task(&bus, epoch, seat, &Actor::Lane(seat.lane), &design);
        }
        let outputs = [
            add_output(&bus, epoch, &design, "First output"),
            add_output(&bus, epoch, &design, "Second output"),
        ];
        let (candidate, source) =
            retain_test_candidate(&bus, epoch, &seats, &design, &outputs[0], &outputs[1]);
        Self {
            root,
            bus,
            epoch,
            seats,
            design,
            outputs,
            candidate,
            source,
        }
    }

    fn decide(&self, outcome: DecisionOutcome) -> Outcome<CommandAnswer> {
        decide(
            &self.bus,
            self.epoch,
            &self.design,
            1,
            &self.source,
            outcome,
        )
    }

    fn returned(&self, reason: &str) -> DecisionOutcome {
        DecisionOutcome::Returned {
            candidate: self.candidate.clone(),
            reason: reason.to_owned(),
        }
    }

    fn rejected(&self) -> DecisionOutcome {
        DecisionOutcome::Resolved {
            candidate: self.candidate.clone(),
            outputs: vec![
                OutputDecision {
                    output: self.outputs[0].born().expect("first output stamp"),
                    verdict: OutputVerdict::Rejected,
                    comment: Some("revise the boundary".to_owned()),
                },
                OutputDecision {
                    output: self.outputs[1].born().expect("second output stamp"),
                    verdict: OutputVerdict::Rejected,
                    comment: None,
                },
            ],
            comment: Some("keep the useful evidence".to_owned()),
        }
    }

    fn occupied(&self) -> BTreeMap<CanonicalUuid, CanonicalUuid> {
        self.seats
            .iter()
            .map(|seat| (seat.lane, seat.occupant))
            .collect()
    }

    fn tell(
        &self,
        occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    ) -> Vec<(CanonicalUuid, String)> {
        let owed = owed_returns(&self.bus, occupied).expect("owed input reads");
        for owed in &owed {
            record_return(&self.bus, owed).expect("delivery records");
        }
        owed.into_iter()
            .map(|owed| (owed.lane, owed.reason))
            .collect()
    }

    fn free(&self, seat: usize) {
        let seat = &self.seats[seat];
        free_active(
            &self.bus,
            self.epoch,
            &Actor::Lane(seat.lane),
            seat,
            &self.design,
        );
    }

    fn claim(&self, seat: usize) {
        let seat = &self.seats[seat];
        claim_task(
            &self.bus,
            self.epoch,
            seat,
            &Actor::Lane(seat.lane),
            &self.design,
        );
    }

    fn decision_outcomes(&self) -> Vec<Value> {
        history(
            self.root.as_ref(),
            self.root.as_ref(),
            &[self.design.as_str()],
        )["decisions"]["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| row["outcome"].clone())
            .collect()
    }

    fn finish(self) {
        clean(&self.root, self.bus);
    }
}

#[test]
fn legacy_return_shape_is_read_only_on_the_current_command_path() {
    let fixture = Returnable::new("design-legacy-return", 1, None);
    let refused = fixture.decide(fixture.returned("revise the premise"));
    assert!(matches!(
        &refused,
        Outcome::Refusal(refusal) if refusal.reason() == "preparation::decision-retired"
    ));
    assert!(fixture.decision_outcomes().is_empty());
    assert!(
        board(&fixture.bus, fixture.epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.authorization.is_none())
    );
    fixture.finish();
}

#[test]
fn rejected_output_and_overall_comments_reach_the_holder_once() {
    let fixture = Returnable::new("design-resolved-feedback", 1, Some(0));
    let rejected = fixture.rejected();
    assert_committed(fixture.decide(rejected.clone()));
    assert_eq!(
        fixture.decision_outcomes(),
        [serde_json::to_value(rejected).expect("decision encodes")]
    );
    let heard = fixture.tell(&fixture.occupied());
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].0, fixture.seats[0].lane);
    assert!(heard[0].1.contains("rejected output"));
    assert!(heard[0].1.contains("revise the boundary"));
    assert!(heard[0].1.contains("overall: keep the useful evidence"));
    assert_eq!(
        fixture.tell(&fixture.occupied()),
        [],
        "restart reconciliation does not redeliver consumed feedback"
    );
    fixture.finish();
}

#[test]
fn feedback_remains_owed_when_the_claim_moves_before_consumption() {
    let fixture = Returnable::new("design-feedback-claim-race", 2, Some(0));
    assert_committed(fixture.decide(fixture.rejected()));
    let occupied = fixture.occupied();
    let stale = owed_returns(&fixture.bus, &occupied)
        .expect("owed input reads")
        .pop()
        .expect("first holder is owed input");
    fixture.free(0);
    for output in &fixture.outputs {
        settle_output(&fixture.bus, fixture.epoch, output, true);
    }
    fixture.claim(1);
    assert!(record_return(&fixture.bus, &stale).is_err());
    assert_eq!(
        fixture.tell(&occupied).first().map(|(lane, _)| *lane),
        Some(fixture.seats[1].lane)
    );
    fixture.finish();
}

#[test]
fn collapse_still_uses_the_operator_conclusion_path() {
    let fixture = Returnable::new("design-collapse", 1, Some(0));
    let collapse = DecisionOutcome::Collapse {
        candidate: fixture.candidate.clone(),
        reason: "the premise did not hold".to_owned(),
    };
    let held = fixture.decide(collapse.clone());
    assert!(matches!(
        &held,
        Outcome::Refusal(refusal) if refusal.reason() == "task::conclude-claimed"
    ));
    fixture.free(0);
    assert_committed(fixture.decide(collapse));
    let current = board(&fixture.bus, fixture.epoch, Actor::Operator);
    let workflow = &at(&current, &fixture.design).workflow.body;
    assert_eq!(workflow.status, "completed");
    assert!(!workflow.concluded_positively());
    fixture.finish();
}

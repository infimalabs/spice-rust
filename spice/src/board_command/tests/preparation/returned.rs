//! The operator's return of one candidate version: immutable history in the
//! room read, and the newest return as its active Design's next input, consumed
//! only for the exact active claim that received it. A collapse decision
//! concludes the Design through the operator's conclusion path.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::contribution::{authority, history};
use super::*;
use crate::board_command::{owed_returns, record_return};

/// One Design with a retained candidate over two prepared outputs.
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
    /// The Design, grouped and claimed by `holder`'s seat before it prepares
    /// its outputs when `holder` is named, since unselected outputs hold an
    /// unclaimed Design for the operator.
    fn new(name: &str, lanes: usize, holder: Option<usize>) -> Self {
        let (root, bus, epoch, seats) = fixture(name, lanes);
        let design = add_controller(&bus, epoch, name, wire::TaskIntent::Design, None);
        if let Some(holder) = holder {
            group_design(&bus, epoch, &design);
            let seat = &seats[holder];
            claim_task(&bus, epoch, seat, &Actor::Lane(seat.lane), &design);
        }
        let outputs = [
            add_output(&bus, epoch, &design, "Returned output"),
            add_output(&bus, epoch, &design, "Returned spare"),
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

    fn returned(&self, reason: &str) -> DecisionOutcome {
        DecisionOutcome::Returned {
            candidate: self.candidate.clone(),
            reason: reason.to_owned(),
        }
    }

    fn decide(&self, sequence: u64, outcome: DecisionOutcome) -> Outcome<CommandAnswer> {
        decide(
            &self.bus,
            self.epoch,
            &self.design,
            sequence,
            &self.source,
            outcome,
        )
    }

    /// Return the candidate once per reason, in order from decision 1.
    fn return_each(&self, reasons: &[&str]) {
        for (sequence, reason) in (1..).zip(reasons) {
            assert_committed(self.decide(sequence, self.returned(reason)));
        }
    }

    /// Delete every prepared output so the Design is offered again.
    fn settle_outputs(&self) {
        for output in &self.outputs {
            settle_output(&self.bus, self.epoch, output, true);
        }
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

    /// Every fixture lane seated by its own occupant.
    fn occupied(&self) -> BTreeMap<CanonicalUuid, CanonicalUuid> {
        self.seats
            .iter()
            .map(|seat| (seat.lane, seat.occupant))
            .collect()
    }

    /// Record every return `occupied` is owed after its supervisor has
    /// confirmed delivery, and name each lane told with the reason it heard.
    fn tell(
        &self,
        occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
    ) -> Vec<(CanonicalUuid, String)> {
        let owed = owed_returns(&self.bus, occupied).expect("owed returns read");
        for owed in &owed {
            record_return(&self.bus, owed).expect("the delivery records");
        }
        owed.into_iter()
            .map(|owed| (owed.lane, owed.reason))
            .collect()
    }

    fn told(&self, seat: usize, reason: &str) -> Vec<(CanonicalUuid, String)> {
        vec![(self.seats[seat].lane, reason.to_owned())]
    }

    /// Each decision the room read shows, as (decision, outcome).
    fn decisions(&self) -> Vec<(Value, Value)> {
        let page = history(
            self.root.as_ref(),
            self.root.as_ref(),
            &[self.design.as_str()],
        );
        page["decisions"]["rows"]
            .as_array()
            .expect("decision rows")
            .iter()
            .map(|row| (row["decision"].clone(), row["outcome"].clone()))
            .collect()
    }

    fn finish(self) {
        clean(&self.root, self.bus);
    }
}

fn encoded(outcome: &DecisionOutcome) -> Value {
    serde_json::to_value(outcome).expect("outcome encodes")
}

#[test]
fn returns_read_in_decision_order_and_change_no_authority() {
    let fixture = Returnable::new("design-returned", 1, None);
    let head = candidate_witness(&fixture.bus, &fixture.candidate);
    let before = authority(&fixture.bus, fixture.epoch, &fixture.root, &fixture.design);

    fixture.return_each(&["name the premise", "split the outputs"]);
    assert_eq!(
        candidate_witness(&fixture.bus, &fixture.candidate),
        head,
        "a return leaves the candidate version untouched"
    );
    let after = authority(&fixture.bus, fixture.epoch, &fixture.root, &fixture.design);
    for unchanged in ["statement", "workflow", "claim"] {
        assert_eq!(
            after[unchanged], before[unchanged],
            "{unchanged} is unchanged"
        );
    }
    assert!(
        board(&fixture.bus, fixture.epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.authorization.is_none()),
        "a return authorizes no output"
    );
    assert_eq!(
        fixture.decisions(),
        [
            (
                json!(wire::SequenceCode::new(1)),
                encoded(&fixture.returned("name the premise"))
            ),
            (
                json!(wire::SequenceCode::new(2)),
                encoded(&fixture.returned("split the outputs"))
            ),
        ],
        "every return reads whole, in decision order"
    );
    fixture.finish();
}

#[test]
fn an_active_holder_is_told_the_newest_return_once() {
    let fixture = Returnable::new("design-return-held", 1, Some(0));
    let occupied = fixture.occupied();
    fixture.return_each(&["older reason", "newest reason"]);
    assert_eq!(
        fixture.tell(&BTreeMap::new()),
        [],
        "a holder whose lane is unoccupied is told nothing yet"
    );
    assert_eq!(
        fixture.tell(&occupied),
        fixture.told(0, "newest reason"),
        "only the newest return is input"
    );
    assert_eq!(
        fixture.tell(&occupied),
        [],
        "a restarted resident reads the delivery and tells no one twice"
    );

    assert_committed(fixture.decide(3, fixture.returned("a later reason")));
    assert_eq!(
        fixture.tell(&occupied),
        fixture.told(0, "a later reason"),
        "a later return is new input"
    );
    fixture.finish();
}

#[test]
fn a_delivery_cannot_consume_a_return_after_its_claim_moves() {
    let fixture = Returnable::new("design-return-claim-race", 2, Some(0));
    let occupied = fixture.occupied();
    fixture.return_each(&["route this to the current holder"]);
    let stale = owed_returns(&fixture.bus, &occupied)
        .expect("owed return reads")
        .pop()
        .expect("the first holder is owed the return");

    fixture.free(0);
    fixture.settle_outputs();
    fixture.claim(1);
    assert!(
        record_return(&fixture.bus, &stale).is_err(),
        "a delivery confirmed for the former claim cannot consume the return"
    );
    assert_eq!(
        fixture.tell(&occupied),
        fixture.told(1, "route this to the current holder"),
        "the return remains owed to the new active holder"
    );
    assert_eq!(
        fixture.tell(&occupied),
        [],
        "the current claim's delivery remains consumed across reconciliation"
    );
    fixture.finish();
}

#[test]
fn an_unheld_return_waits_for_the_next_claimant() {
    let fixture = Returnable::new("design-return-unheld", 2, Some(0));
    let occupied = fixture.occupied();
    fixture.free(0);
    fixture.return_each(&["older reason", "newest reason"]);
    assert_eq!(fixture.tell(&occupied), [], "no holder, no delivery");

    fixture.settle_outputs();
    fixture.claim(1);
    assert_eq!(fixture.tell(&occupied), fixture.told(1, "newest reason"));
    fixture.free(1);
    fixture.claim(0);
    assert_eq!(
        fixture.tell(&occupied),
        [],
        "a told return is consumed for every later claimant"
    );
    fixture.finish();
}

#[test]
fn a_passive_return_is_told_only_once_group_entry_leads_to_a_claim() {
    let fixture = Returnable::new("design-return-passive", 1, None);
    let occupied = fixture.occupied();
    fixture.return_each(&["older reason", "newest reason"]);
    fixture.settle_outputs();
    assert_eq!(
        fixture.tell(&occupied),
        [],
        "a passive Design wakes no lane"
    );

    group_design(&fixture.bus, fixture.epoch, &fixture.design);
    assert_eq!(
        fixture.tell(&occupied),
        [],
        "group entry alone tells no one"
    );
    fixture.claim(0);
    assert_eq!(
        fixture.tell(&occupied),
        fixture.told(0, "newest reason"),
        "the claim that group entry allowed hears the newest return"
    );
    fixture.finish();
}

#[test]
fn a_collapse_decision_concludes_through_the_operator_conclusion() {
    let fixture = Returnable::new("design-return-collapse", 1, Some(0));
    fixture.return_each(&["revise"]);
    let collapse = DecisionOutcome::Collapse {
        candidate: fixture.candidate.clone(),
        reason: "the premise did not hold".to_owned(),
    };
    let held = fixture.decide(2, collapse.clone());
    assert!(
        matches!(&held, Outcome::Refusal(refusal) if refusal.reason() == "task::conclude-claimed"),
        "a collapse refuses a held Design as conclusion does: {held:?}"
    );

    fixture.free(0);
    assert_committed(fixture.decide(2, collapse.clone()));
    let workflow = at(
        &board(&fixture.bus, fixture.epoch, Actor::Operator),
        &fixture.design,
    )
    .workflow
    .body
    .clone();
    assert_eq!(workflow.status, "completed");
    assert!(
        !workflow.concluded_positively(),
        "collapse concludes negatively"
    );
    let design = fixture.design.born().expect("design stamp");
    let concluded = fixture
        .bus
        .fact_snapshot()
        .expect("facts read")
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<body::TaskConcluded>())
        .filter(|concluded| concluded.task == design)
        .collect::<Vec<_>>();
    assert_eq!(
        concluded,
        [body::TaskConcluded {
            task: design,
            conclusion: body::DesignConclusion::Collapsed {
                reason: "the premise did not hold".to_owned(),
            },
        }],
        "the collapse is the Design's one conclusion"
    );
    assert_eq!(
        fixture.tell(&fixture.occupied()),
        [],
        "a concluded Design leaves no input to tell"
    );
    assert_eq!(
        fixture
            .decisions()
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect::<Vec<_>>(),
        [encoded(&fixture.returned("revise")), encoded(&collapse)],
        "history keeps the return and the collapse with its candidate and reason"
    );
    fixture.finish();
}

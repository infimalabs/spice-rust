//! One drag, one durable arrangement.
//!
//! Every gesture the canvas offers -- dragging a team into another's place,
//! dropping one onto another's edge to join them -- arrives here as the same
//! whole restatement of the lane table. These tests share one fixture because
//! the law under them is shared: an arrangement is admitted entire or refused
//! entire, and the standing it leaves behind is read from the durable head
//! rather than the reply, which only ever describes the lineages it returned.

use super::*;

/// Supervised lanes standing in two teams, the shape an arrangement starts from.
struct Arranged {
    root: PathBuf,
    bus: Bus,
    epoch: CanonicalUuid,
    seats: Seats,
    teams: [CanonicalUuid; 2],
    lanes: Vec<CanonicalUuid>,
    occupants: Vec<CanonicalUuid>,
}

impl Arranged {
    /// Seat one supervised lane per entry, in the team that entry names.
    fn seat(name: &str, membership: &[usize]) -> Self {
        let (root, paths) = resident_fixture(name);
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let teams = [test_uuid(2), test_uuid(8)];
        let mut seats = Seats::new();
        let mut lanes = Vec::new();
        let mut occupants = Vec::new();
        for (index, team) in membership.iter().enumerate() {
            let step = u8::try_from(index).expect("fixture lane count");
            let lane = test_uuid(30 + step * 2);
            let occupant = test_uuid(31 + step * 2);
            seats
                .seat(
                    lane,
                    occupant,
                    process_spec("sleep 0.05", Duration::from_secs(5)),
                )
                .expect("supervised occupant starts");
            seed_lane_in_lineage(&bus, epoch, lane, teams[*team], occupant);
            lanes.push(lane);
            occupants.push(occupant);
        }
        Self {
            root,
            bus,
            epoch,
            seats,
            teams,
            lanes,
            occupants,
        }
    }

    /// State a whole arrangement the way one drag states it.
    fn arrange(
        &self,
        id: u8,
        lineages: Vec<wire::intent::LineageArrangement>,
    ) -> Outcome<CommandAnswer> {
        let request = wire::intent::LineageArrange { lineages };
        let command = arrange_command(self.epoch, test_uuid(id), &request);
        lane_surface::arrange(&self.bus, self.epoch, &self.seats, &command, &request)
    }

    /// The authoritative lane table a committed arrangement replies with.
    fn commit(
        &self,
        id: u8,
        lineages: Vec<wire::intent::LineageArrangement>,
    ) -> Box<wire::intent::LaneAnswer> {
        let Outcome::Reply(reply) = self.arrange(id, lineages) else {
            panic!("lineage arrangement replies");
        };
        assert!(matches!(reply.position, ReplyPosition::Committed(_)));
        let CommandAnswer::Lane(answer) = reply.body else {
            panic!("lineage arrangement returns the authoritative lane table");
        };
        answer
    }

    /// The standing the resident would restart from.
    fn head(&self) -> wire::ResidencyHead {
        current_residency(&self.bus, self.epoch)
            .expect("arranged residency reads")
            .into_body()
    }

    /// The lineage every seated lane now belongs to, in durable lane order.
    fn durable_membership(&self) -> Vec<(CanonicalUuid, CanonicalUuid)> {
        self.head()
            .lanes
            .iter()
            .map(|row| (row.lane, row.lineage_id))
            .collect()
    }

    /// Every lineage the durable head still holds a task subscription for.
    fn durable_subscriptions(&self) -> Vec<CanonicalUuid> {
        self.head()
            .subscriptions
            .iter()
            .map(|subscription| subscription.lineage_id)
            .collect()
    }

    fn retire(mut self) {
        for lane in &self.lanes {
            self.seats
                .vacate(*lane)
                .expect("arranged occupant remains supervised")
                .wait()
                .expect("fixture occupant exits");
        }
        self.bus.verify().expect("arranged authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

/// One arranged lineage, stated as the operator's canvas states it.
fn standing(
    lineage_id: CanonicalUuid,
    lanes: &[CanonicalUuid],
) -> wire::intent::LineageArrangement {
    wire::intent::LineageArrangement {
        lineage_id,
        lanes: lanes.to_vec(),
    }
}

fn arrange_command(
    epoch: CanonicalUuid,
    id: CanonicalUuid,
    request: &wire::intent::LineageArrange,
) -> wire::AdmittedCommand {
    Command {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::LineageArrange,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("lineage arrangement encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator lineage arrangement admits")
}

/// The arranged standings a fact recorded, read back off the journal.
fn arranged_fact(bus: &Bus, field: &str) -> serde_json::Value {
    let snapshot = bus.canvas_snapshot().expect("arranged journal reads");
    let facts = snapshot.facts();
    let arranged = facts
        .facts()
        .iter()
        .rfind(|fact| fact.fact.kind() == wire::FactKind::LineageArranged)
        .expect("lineage arrangement is durable");
    arranged
        .fact
        .body()
        .get(field)
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// Both drag gestures commit through one arrangement.
///
/// The reorder and the join are asserted in a single arrangement because that
/// is the only shape the canvas can send: a drag states where every team and
/// every lane stands afterwards, so a test that moved one lane at a time would
/// be proving a law this resident does not offer. The occupant, its lineage,
/// and its supervised seat are asserted across the move because a joined lane
/// that lost them would be a new lane wearing an old identity.
#[test]
fn one_arrangement_reorders_teams_and_joins_lanes_without_losing_composer_identity() {
    let fixture = Arranged::seat("lineage-arrangement", &[0, 0, 1]);
    let [first_team, second_team] = fixture.teams;
    let lanes = fixture.lanes.clone();
    let occupants = fixture.occupants.clone();

    let answer = fixture.commit(
        11,
        vec![
            standing(second_team, &[lanes[2], lanes[1]]),
            standing(first_team, &[lanes[0]]),
        ],
    );

    assert_eq!(
        answer.lanes.iter().map(|row| row.lane).collect::<Vec<_>>(),
        vec![lanes[2], lanes[1], lanes[0]],
        "the arranged order is the durable lane order",
    );
    assert_eq!(
        answer
            .lanes
            .iter()
            .map(|row| row.lineage_id)
            .collect::<Vec<_>>(),
        vec![second_team, second_team, first_team],
        "the joined lane holds the target team",
    );
    assert_eq!(
        answer
            .lanes
            .iter()
            .map(|row| row.occupant)
            .collect::<Vec<_>>(),
        vec![Some(occupants[2]), Some(occupants[1]), Some(occupants[0])],
        "a joined lane keeps the composer that occupies it",
    );
    assert_eq!(answer.lanes[1].lineage, vec![occupants[1]]);
    assert!(
        fixture.seats.occupied(lanes[1]),
        "joining never vacates a seat"
    );
    assert_eq!(
        answer
            .subscriptions
            .iter()
            .map(|subscription| subscription.lineage_id)
            .collect::<Vec<_>>(),
        vec![first_team, second_team],
        "both surviving teams keep their task subscription",
    );
    assert_eq!(
        fixture
            .head()
            .lanes
            .iter()
            .map(|row| row.lane)
            .collect::<Vec<_>>(),
        vec![lanes[2], lanes[1], lanes[0]],
    );
    assert_eq!(
        arranged_fact(&fixture.bus, "from"),
        serde_json::json!([
            {
                "lineage_id": first_team.to_string(),
                "lanes": [lanes[0].to_string(), lanes[1].to_string()],
            },
            { "lineage_id": second_team.to_string(), "lanes": [lanes[2].to_string()] },
        ]),
        "the fact records the standing the arrangement replaced",
    );
    assert_eq!(
        arranged_fact(&fixture.bus, "lineages"),
        serde_json::json!([
            {
                "lineage_id": second_team.to_string(),
                "lanes": [lanes[2].to_string(), lanes[1].to_string()],
            },
            { "lineage_id": first_team.to_string(), "lanes": [lanes[0].to_string()] },
        ]),
    );

    fixture.retire();
}

/// Joining the last lane out of a team retires that team's subscription.
///
/// A policy left behind would be inherited by whichever lineage next carried
/// that id, so this cannot be read off the reply: the answer states one
/// subscription per lineage it returned, and an emptied lineage is returned by
/// nobody. Only the durable head can show the orphan, and a stale row there
/// faults the next startup as a misaligned subscription table.
#[test]
fn emptying_a_lineage_retires_the_task_subscription_it_left_behind() {
    let fixture = Arranged::seat("lineage-arrangement-emptied", &[0, 0, 1]);
    let [_, second_team] = fixture.teams;
    let lanes = fixture.lanes.clone();

    let answer = fixture.commit(
        12,
        vec![standing(second_team, &[lanes[2], lanes[1], lanes[0]])],
    );

    assert!(answer.lanes.iter().all(|row| row.lineage_id == second_team));
    assert_eq!(
        answer
            .subscriptions
            .iter()
            .map(|subscription| subscription.lineage_id)
            .collect::<Vec<_>>(),
        vec![second_team],
    );
    assert_eq!(
        fixture.durable_subscriptions(),
        vec![second_team],
        "an emptied team keeps no durable task subscription",
    );

    fixture.retire();
}

/// An arrangement is admitted as a whole or not at all.
///
/// Each case below is built to trip exactly one rule, because a fixture that
/// broke two would only ever prove the first. The durable head is read after
/// every refusal: a partially applied arrangement would leave a lane holding a
/// team the operator never stated.
#[test]
fn an_arrangement_that_is_not_a_faithful_restatement_of_the_lane_table_is_refused() {
    let fixture = Arranged::seat("lineage-arrangement-refusals", &[0, 1]);
    let [first_team, second_team] = fixture.teams;
    let lanes = fixture.lanes.clone();
    let stranger = test_uuid(99);
    let seated = vec![(lanes[0], first_team), (lanes[1], second_team)];

    let cases = [
        ("lineage::arrangement-empty", Vec::new()),
        (
            "lineage::arrangement-unknown-lineage",
            vec![
                standing(stranger, &[lanes[0]]),
                standing(second_team, &[lanes[1]]),
            ],
        ),
        (
            "lineage::arrangement-duplicate-lineage",
            vec![
                standing(first_team, &[lanes[0]]),
                standing(first_team, &[lanes[1]]),
            ],
        ),
        (
            "lineage::arrangement-vacant-lineage",
            vec![
                standing(first_team, &[]),
                standing(second_team, &[lanes[0], lanes[1]]),
            ],
        ),
        (
            "lineage::arrangement-unknown-lane",
            vec![
                standing(first_team, &[lanes[0]]),
                standing(second_team, &[lanes[1], stranger]),
            ],
        ),
        (
            "lineage::arrangement-duplicate-lane",
            vec![
                standing(first_team, &[lanes[0], lanes[0]]),
                standing(second_team, &[lanes[1]]),
            ],
        ),
        (
            "lineage::arrangement-incomplete",
            vec![standing(first_team, &[lanes[0]])],
        ),
    ];
    for (index, (reason, lineages)) in cases.into_iter().enumerate() {
        let id = 20 + u8::try_from(index).expect("fixture case count");
        let outcome = fixture.arrange(id, lineages);
        let Outcome::Refusal(refusal) = outcome else {
            panic!("{reason} must refuse rather than commit: {outcome:?}");
        };
        assert_eq!(refusal.reason, reason);
        assert_eq!(refusal.witness.as_deref(), Some("lane"));
        assert_eq!(
            fixture.durable_membership(),
            seated,
            "{reason} moved a lane before refusing",
        );
    }

    fixture.retire();
}

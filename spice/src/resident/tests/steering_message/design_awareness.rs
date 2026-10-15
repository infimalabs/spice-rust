//! A claimant's occupant is invited once to each matching passive Design,
//! never to one that is active, concluded, its own, or a room it already
//! listens to; a lane that contributed to or left a Design is never invited
//! to it again, and an ignored invitation waits for a later occupant.

use std::collections::BTreeMap;

use wire::rooms::DesignNoticed;

use super::*;

const PITCH: &str = "Lanes page retained publications through bounded cursors.";
const MATCHING: &str = "Retained publications page through bounded cursors.";
const TERMS: [&str; 7] = [
    "catch",
    "cursor",
    "lane",
    "page",
    "publication",
    "retained",
    "room",
];
const TASK_TITLE: &str = "Page room catch-up";
const TASK_DESCRIPTION: &str = "Bound the cursor of retained publications per lane.";

/// Resident seats for each `(lane, occupant)`.
fn seats(occupants: &[(u8, CanonicalUuid)]) -> Seats {
    let mut seats = Seats::new();
    for &(lane, occupant) in occupants {
        seats
            .seat_detached(
                test_uuid(lane),
                occupant,
                Box::new(DetachedFixture {
                    dropped: Arc::new(AtomicU64::new(0)),
                }),
            )
            .expect("claimant seats");
    }
    seats
}

/// FIRST and SECOND seated, each with an occupant born one past its lane.
fn claimant_seats() -> Seats {
    seats(&[
        (FIRST, test_uuid(FIRST + 1)),
        (SECOND, test_uuid(SECOND + 1)),
    ])
}

fn stamp(handle: &wire::TaskHandle) -> wire::TaskStamp {
    handle.born().expect("fixture handle has a birth")
}

impl Lanes {
    fn noticed(&self) -> Vec<DesignNoticed> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| envelope.fact.read::<DesignNoticed>())
            .collect()
    }

    /// The Designs told to `occupant` on `lane`, in record order.
    fn told(&self, lane: u8, occupant: CanonicalUuid) -> Vec<wire::TaskStamp> {
        self.noticed()
            .into_iter()
            .filter(|row| row.lane == test_uuid(lane) && row.occupant == occupant)
            .map(|row| row.design)
            .collect()
    }

    fn design(&self, stamp: &str, title: &str, description: &str) -> wire::TaskHandle {
        self.design_held(stamp, title, description, None)
    }

    fn design_held(
        &self,
        stamp: &str,
        title: &str,
        description: &str,
        holder: Option<u8>,
    ) -> wire::TaskHandle {
        self.seed_task(
            stamp,
            title,
            Some(description),
            Some(wire::TaskIntent::Design),
            holder.map(test_uuid),
        )
    }

    /// A new task matching every fixture Design, claimed by `lane`.
    fn claimed(&self, stamp: &str, lane: u8) -> wire::TaskHandle {
        self.seed_task(
            stamp,
            TASK_TITLE,
            Some(TASK_DESCRIPTION),
            None,
            Some(test_uuid(lane)),
        )
    }

    fn claim_head(&self, task: &wire::TaskHandle) -> Option<wire::ClaimHead> {
        let task = stamp(task);
        self.bus
            .read(move |read| read.claim_head(&task))
            .expect("claim reads")
            .map(|head| head.body().clone())
    }

    /// Release the active claim on `task`, as `spice task free` does.
    fn free(&self, task: &wire::TaskHandle) {
        let claim = self.claim_head(task).expect("the task is claimed");
        let mut release = write(claim.actor.clone(), self.epoch, Producer::CommandDoor)
            .expect("release write starts");
        let freed = release
            .fact(
                wire::body::ClaimFreed {
                    task: stamp(task),
                    actor: claim.actor.clone(),
                    parked: None,
                },
                FactCause::Root,
            )
            .expect("release fact is valid");
        release.claim_head(stamp(task), freed, move |fact| wire::ClaimHead {
            fact,
            disposition: wire::ClaimDisposition::Freed,
            ..claim
        });
        commit(&self.bus, release).expect("release commits");
    }

    fn command(
        &self,
        id: u8,
        actor: Actor,
        kind: CommandKind,
        expected: Vec<Witness>,
        body: &impl serde::Serialize,
    ) -> wire::AdmittedCommand {
        Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind,
            expected,
            body: serde_json::to_value(body).expect("command encodes"),
        }
        .admit(
            &wire::Accepted {
                wire: wire::WireVersion::CURRENT,
                actor,
                epoch: self.epoch,
                release: "1.2.3".parse().expect("fixture release"),
            },
            &wire::principal::PrincipalRoles::default(),
        )
        .expect("command admits")
    }

    /// Stand `design` in the fixture working group, or out of every group.
    fn group(&self, id: u8, config: &SurfaceConfig, design: &wire::TaskHandle, grouped: bool) {
        let revision = self
            .bus
            .read(|read| read.group_partition())
            .expect("partition reads")
            .map_or(0, |head| head.body().revision);
        let request = wire::rooms::WorkingGroupMove {
            design: stamp(design),
            to: grouped.then(|| {
                wire::rooms::RoomAuthorityStamp::for_micros(1_750_000_000_000_000)
                    .expect("fixture group stamp")
            }),
        };
        let command = self.command(
            id,
            Actor::Operator,
            CommandKind::WorkingGroupMove,
            vec![
                Witness::Group { revision },
                Witness::Existence {
                    task: stamp(design),
                },
            ],
            &request,
        );
        let (outcome, _) = crate::board_command::move_working_group(
            &self.bus,
            &command,
            &request,
            &config.steering,
            &config.projects,
            &config.allocation,
        );
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "group move commits: {outcome:?}"
        );
    }

    fn contribute(&self, id: u8, config: &SurfaceConfig, lane: u8, design: &wire::TaskHandle) {
        let request = wire::intent::RoomContribute {
            room: design.clone(),
            kind: wire::rooms::RoomContributionKind::Evidence,
            text: "The catch-up page already bounds its cursor.".to_owned(),
            sources: Vec::new(),
        };
        let task = stamp(design);
        let (statement, workflow) = self
            .bus
            .read(move |read| Ok((read.task_head(&task)?, read.workflow_head(&task)?)))
            .expect("Design heads read");
        let workflow = workflow.expect("the Design has a workflow");
        let command = self.command(
            id,
            Actor::Lane(test_uuid(lane)),
            CommandKind::RoomContribute,
            vec![
                Witness::Task {
                    task: stamp(design),
                    head: statement.expect("the Design exists").position(),
                },
                Witness::Workflow {
                    task: stamp(design),
                    head: workflow.position(),
                    phase: workflow.body().phase.clone(),
                },
            ],
            &request,
        );
        let outcome = crate::board_command::room_contribute(
            &self.bus,
            &command,
            &request,
            None,
            &config.projects,
            &config.allocation,
        );
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "contribution commits: {outcome:?}"
        );
    }

    fn leave(&self, id: u8, config: &SurfaceConfig, lane: u8, design: &wire::TaskHandle) {
        let request = wire::intent::RoomLeave {
            room: design.clone(),
            clause: "not my problem space".to_owned(),
        };
        let command = self.command(
            id,
            Actor::Lane(test_uuid(lane)),
            CommandKind::RoomLeave,
            Vec::new(),
            &request,
        );
        let outcome = crate::board_command::room_leave(
            &self.bus,
            &command,
            &request,
            &config.projects,
            &config.allocation,
        );
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "leave commits: {outcome:?}"
        );
    }

    /// Run one resident awareness pass over `seats`, failing on any
    /// diagnostic.
    fn reconcile(&self, config: &SurfaceConfig, seats: &Seats) {
        let mut diagnostics = Vec::new();
        crate::resident::design_awareness::reconcile(
            &self.bus,
            &self.paths,
            config,
            seats,
            &mut diagnostics,
        )
        .expect("awareness reconciles");
        assert!(
            diagnostics.is_empty(),
            "{}",
            String::from_utf8_lossy(&diagnostics)
        );
    }

    /// Whether `lane` listens to `room` by the current attention projection.
    fn listens(&self, config: &SurfaceConfig, room: &wire::TaskStamp, lane: u8) -> bool {
        let snapshot = self.bus.task_snapshot().expect("snapshot reads");
        let entries = snapshot
            .tasks()
            .iter()
            .map(|heads| {
                board::Entry::new(
                    heads.statement().body().clone(),
                    heads.workflow().body().clone(),
                    heads.claim().map(|head| head.body().clone()),
                    heads.lease().map(|head| head.body().clone()),
                    &config.projects,
                )
                .expect("fixture entry")
            })
            .collect::<Vec<_>>();
        crate::board_command::project_attention(
            &self.bus,
            &entries,
            snapshot.residency().expect("residency").body(),
            room,
            &[],
            Instant::new(jiff::Timestamp::now()).expect("clock"),
        )
        .expect("attention projects")
        .lanes
        .iter()
        .any(|row| row.lane == test_uuid(lane) && row.listens())
    }
}

fn surface(lanes: &Lanes) -> SurfaceConfig {
    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    load_startup_surface(&repository)
        .expect("fixture surface loads")
        .config
}

#[test]
fn a_claimant_occupant_is_invited_once_to_each_matching_passive_design() {
    let lanes = Lanes::new("design-awareness");
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let passive = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let concluded = lanes.design("dVrD245Km", "Room catch-up retention", MATCHING);
    lanes.close_room(&concluded);
    let active = lanes.design("dVrD245Kn", "Room catch-up ordering", MATCHING);
    lanes.group(30, &config, &active, true);
    let unrelated = lanes.design("dVrD245Kf", "Working groups", "Arrange the board.");
    let listened = lanes.design("dVrD245Kg", "Room catch-up invitations", MATCHING);
    let held = lanes.design_held(
        "dVrD245Kh",
        "Room catch-up horizons",
        MATCHING,
        Some(SECOND),
    );
    let first = lanes.address(FIRST);
    lanes.author(SENDER, &format!("MSG {first}#{listened}: join us here"));
    lanes.account(SENDER);
    let _ = lanes.hear(SENDER, test_uuid(SENDER + 1));
    let claim = lanes.claim_head(&task);
    let pending = lanes.pending(FIRST);

    let mut seated = claimant_seats();
    lanes.reconcile(&config, &seated);
    let noticed = lanes.noticed();
    assert_eq!(
        lanes.told(FIRST, test_uuid(FIRST + 1)),
        [stamp(&passive), stamp(&held)],
        "matching passive Designs the lane does not hold or hear are invited, even one \
         another lane holds; not {concluded}, {active}, {unrelated}, or {listened}"
    );
    let row = &noticed[0];
    assert_eq!(row.task, stamp(&task));
    assert_eq!(row.terms, TERMS);
    assert!(
        noticed
            .iter()
            .all(|row| !(row.lane == test_uuid(SECOND) && row.design == stamp(&held))),
        "a lane is never invited to the Design it holds"
    );
    assert_eq!(
        lanes.claim_head(&task),
        claim,
        "an invitation moves no claim"
    );
    assert_eq!(
        lanes.pending(FIRST),
        pending,
        "an invitation owes no acknowledgment"
    );
    assert!(
        !lanes.listens(&config, &stamp(&passive), FIRST),
        "an invitation adds no room attention"
    );

    let mut restarted = claimant_seats();
    lanes.reconcile(&config, &restarted);
    assert_eq!(
        lanes.noticed(),
        noticed,
        "a restarted resident reads every invitation back and repeats none"
    );
    let read = lanes.room_read(&["awareness", passive.as_str()]);
    let rows = read["page"]["rows"].as_array().expect("awareness rows");
    assert_eq!(
        rows.iter()
            .map(|row| (row["lane"].clone(), row["terms"].clone()))
            .collect::<Vec<_>>(),
        noticed
            .iter()
            .filter(|told| told.design == stamp(&passive))
            .map(|told| (serde_json::json!(told.lane), serde_json::json!(told.terms)))
            .collect::<Vec<_>>(),
        "the operator reads every lane invited to the Design with its matched terms"
    );
    assert_eq!(rows[0]["task"], serde_json::json!(task));
    let read = lanes.room_read(&["awareness", unrelated.as_str()]);
    assert_eq!(read["page"]["rows"], serde_json::json!([]));
    seated.release_all();
    restarted.release_all();
    lanes.finish();
}

#[test]
fn a_stale_observation_commits_no_invitation() {
    let lanes = Lanes::new("design-awareness-race");
    let config = surface(&lanes);
    let _task = lanes.claimed("dVrD245Kc", FIRST);
    let passive = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let left = lanes.design("dVrD245Kg", "Room catch-up invitations", MATCHING);
    let occupied = BTreeMap::from([(test_uuid(FIRST), test_uuid(FIRST + 1))]);
    let observe = || {
        let planning = crate::resident::room_catch_up::Planning::load(&lanes.bus, &config)
            .expect("planning loads");
        crate::resident::design_awareness::plan(&lanes.bus, &planning, &occupied)
            .expect("invitations plan")
    };
    let (earlier, later) = (observe(), observe());
    let designs = |told: &[crate::resident::design_awareness::Told]| {
        told.iter()
            .map(|told| told.design.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(designs(&earlier), [stamp(&passive), stamp(&left)]);
    assert_eq!(designs(&later), designs(&earlier));
    let record = |told| {
        crate::resident::design_awareness::record(&lanes.bus, told).expect("the record is judged")
    };
    assert!(record(&earlier[0]), "the first observation commits");
    assert!(
        !record(&later[0]),
        "a concurrent observation of the same invitation commits nothing"
    );
    lanes.leave(30, &config, FIRST, &left);
    assert!(
        !record(&earlier[1]),
        "an observation from before the lane left commits nothing"
    );
    assert_eq!(lanes.told(FIRST, test_uuid(FIRST + 1)), [stamp(&passive)]);
    lanes.finish();
}

#[test]
fn an_ignored_invitation_waits_for_a_later_occupant() {
    let lanes = Lanes::new("design-awareness-occupancy");
    let config = surface(&lanes);
    let passive = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let original = test_uuid(FIRST + 1);
    let mut seated = seats(&[(FIRST, original)]);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    lanes.reconcile(&config, &seated);
    assert_eq!(lanes.told(FIRST, original), [stamp(&passive)]);

    lanes.free(&first);
    let second = lanes.claimed("dVrD245Kp", FIRST);
    lanes.reconcile(&config, &seated);
    assert_eq!(
        lanes.told(FIRST, original),
        [stamp(&passive)],
        "the same occupant is not invited again at a later claim"
    );
    seated.release_all();

    lanes.vacate(FIRST);
    let before = lanes.noticed();
    let mut vacant = seats(&[]);
    lanes.reconcile(&config, &vacant);
    assert_eq!(lanes.noticed(), before, "a vacant lane is never invited");
    lanes.free(&second);
    let renewed = test_uuid(40);
    lanes.renew(FIRST, renewed);
    let mut reseated = seats(&[(FIRST, renewed)]);
    lanes.reconcile(&config, &reseated);
    assert!(
        lanes.told(FIRST, renewed).is_empty(),
        "a renewed occupant is invited only at a claim"
    );
    let _third = lanes.claimed("dVrD245Kq", FIRST);
    lanes.reconcile(&config, &reseated);
    assert_eq!(
        lanes.told(FIRST, renewed),
        [stamp(&passive)],
        "a later occupant of an unsuppressed lane is invited at its matching claim"
    );
    vacant.release_all();
    reseated.release_all();
    lanes.finish();
}

#[test]
fn a_lane_that_contributed_or_left_is_never_invited_again() {
    let lanes = Lanes::new("design-awareness-suppression");
    let config = surface(&lanes);
    let contributed = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let left = lanes.design("dVrD245Kg", "Room catch-up invitations", MATCHING);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    lanes.contribute(30, &config, FIRST, &contributed);
    lanes.leave(31, &config, FIRST, &left);
    let mut original = seats(&[(FIRST, test_uuid(FIRST + 1))]);
    lanes.reconcile(&config, &original);
    assert!(
        lanes.told(FIRST, test_uuid(FIRST + 1)).is_empty(),
        "a lane that contributed or left is not invited"
    );
    original.release_all();

    lanes.free(&first);
    for (id, design) in [(32, &contributed), (34, &left)] {
        lanes.group(id, &config, design, true);
        lanes.group(id + 1, &config, design, false);
    }
    lanes.vacate(FIRST);
    let renewed = test_uuid(40);
    lanes.renew(FIRST, renewed);
    let fresh = lanes.design("dVrD245Kh", "Room catch-up horizons", MATCHING);
    let _second = lanes.claimed("dVrD245Kp", FIRST);
    let _other = lanes.claimed("dVrD245Kq", SECOND);
    let mut reseated = seats(&[(FIRST, renewed), (SECOND, test_uuid(SECOND + 1))]);
    lanes.reconcile(&config, &reseated);
    assert_eq!(
        lanes.told(FIRST, renewed),
        [stamp(&fresh)],
        "contribution and leave suppress the lane across occupants and group moves"
    );
    assert_eq!(
        lanes.told(SECOND, test_uuid(SECOND + 1)),
        [stamp(&contributed), stamp(&left), stamp(&fresh)],
        "suppression belongs to the lane that contributed or left"
    );
    reseated.release_all();
    lanes.finish();
}

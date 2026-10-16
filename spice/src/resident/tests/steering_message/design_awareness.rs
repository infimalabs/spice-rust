//! Organic Design invitations remain once per occupant, retain every standing
//! and engagement suppression, survive restart, and refuse stale observations.

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

    /// Complete the task under its active claim with one durable `TaskDone`.
    fn complete(&self, handle: &wire::TaskHandle, validation: &str) {
        let claim = self.claim_head(handle).expect("the task is claimed");
        let task = stamp(handle);
        let workflow = self
            .bus
            .read({
                let task = task.clone();
                move |read| read.workflow_head(&task)
            })
            .expect("workflow reads")
            .expect("workflow exists")
            .into_body();
        let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
        let operation = wire::intent::TaskDone::operation_for_claim(claim.claimed);
        let validation = validation.to_owned();
        let mut completion = write(claim.actor.clone(), self.epoch, Producer::CommandDoor)
            .expect("completion write starts");
        let done = completion
            .fact(
                wire::body::TaskDone {
                    task: task.clone(),
                    validation,
                    terminal: Some(wire::body::TaskCompletionTerminal {
                        operation,
                        status: wire::body::TaskCompletionStatus::Completed,
                        next: None,
                    }),
                    git: None,
                    source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
                },
                FactCause::Root,
            )
            .expect("completion fact is valid");
        completion.workflow_head(task.clone(), done, move |_| wire::WorkflowHead {
            status: wire::WorkflowStatus::completed(at),
            ..workflow
        });
        completion.claim_head(task, done, move |fact| wire::ClaimHead {
            fact,
            disposition: wire::ClaimDisposition::Freed,
            ..claim
        });
        commit(&self.bus, completion).expect("completion commits");
    }

    /// Change one task statement after an awareness observation.
    fn retitle(&self, task: &wire::TaskHandle, title: &str) {
        let stamp = stamp(task);
        let head = self
            .bus
            .read({
                let stamp = stamp.clone();
                move |read| read.task_head(&stamp)
            })
            .expect("task statement reads")
            .expect("task statement exists")
            .into_body();
        let mut edit = write(Actor::Operator, self.epoch, Producer::CommandDoor)
            .expect("statement edit starts");
        let fact = edit
            .fact(
                wire::body::TaskEdited {
                    task: stamp,
                    revision: 2,
                    dependencies: None,
                    intent: None,
                    evidence: None,
                },
                FactCause::Root,
            )
            .expect("statement edit fact builds");
        let title = title.to_owned();
        edit.task_head(fact, move |revision| wire::TaskHead {
            title,
            revision,
            ..head
        });
        commit(&self.bus, edit).expect("statement edit commits");
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
        if grouped {
            self.stance(
                id.saturating_add(1),
                config,
                wire::rooms::WorkingGroupStance::Explore,
            );
        }
    }

    fn stance(&self, id: u8, config: &SurfaceConfig, stance: wire::rooms::WorkingGroupStance) {
        let partition = self
            .bus
            .read(|read| read.group_partition())
            .expect("partition reads")
            .expect("fixture group stands")
            .into_body();
        let request = wire::rooms::WorkingGroupStanceSet {
            group: partition.groups[0].group.clone(),
            stance,
        };
        let command = self.command(
            id,
            Actor::Operator,
            CommandKind::WorkingGroupStance,
            vec![Witness::Group {
                revision: partition.revision,
            }],
            &request,
        );
        let (outcome, _) = crate::board_command::set_working_group_stance(
            &self.bus,
            &command,
            &request,
            &config.steering,
            &config.projects,
            &config.allocation,
        );
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "group stance commits: {outcome:?}"
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
        crate::resident::design_awareness::reconcile_claims(
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

    /// Run one resident completion-aware pass, failing on any diagnostic.
    fn reconcile_completions(
        &self,
        config: &SurfaceConfig,
        seats: &Seats,
        completions: &mut crate::resident::design_awareness::CompletionCursor,
    ) {
        let mut diagnostics = Vec::new();
        crate::resident::design_awareness::reconcile(
            &self.bus,
            &self.paths,
            config,
            seats,
            completions,
            &mut diagnostics,
        )
        .expect("completion awareness reconciles");
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
fn assert_awareness_projection(
    lanes: &Lanes,
    passive: &wire::TaskHandle,
    unrelated: &wire::TaskHandle,
    noticed: &[DesignNoticed],
    task: &wire::TaskHandle,
) {
    let read = lanes.room_read(&["awareness", passive.as_str()]);
    let rows = read["page"]["rows"].as_array().expect("awareness rows");
    assert_eq!(
        rows.iter()
            .map(|row| {
                (
                    row["lane"].clone(),
                    row["statement"].clone(),
                    row["terms"].clone(),
                )
            })
            .collect::<Vec<_>>(),
        noticed
            .iter()
            .filter(|told| told.design == stamp(passive))
            .map(|told| {
                (
                    serde_json::json!(told.lane),
                    serde_json::json!(told.statement),
                    serde_json::json!(wire::rooms::shown_notice_terms(&told.terms)),
                )
            })
            .collect::<Vec<_>>(),
        "the operator reads the exact statement and shown terms of every invitation"
    );
    assert_eq!(rows[0]["task"], serde_json::json!(task));
    let read = lanes.room_read(&["awareness", unrelated.as_str()]);
    assert!(
        read["page"]["rows"]
            .as_array()
            .expect("unmatched awareness rows")
            .is_empty(),
        "an unmatched passive Design receives no synthetic invitation"
    );
}
fn assert_planned_invitation(lanes: &Lanes, config: &SurfaceConfig, passive: &wire::TaskHandle) {
    let occupied = BTreeMap::from([
        (test_uuid(FIRST), test_uuid(FIRST + 1)),
        (test_uuid(SECOND), test_uuid(SECOND + 1)),
    ]);
    let planning =
        crate::resident::room_catch_up::Planning::load(&lanes.bus, config).expect("planning loads");
    let planned = crate::resident::design_awareness::plan(&lanes.bus, &planning, &occupied, &[])
        .expect("invitations plan");
    let invitation = planned
        .iter()
        .find(|told| told.lane == test_uuid(FIRST) && told.design == stamp(passive))
        .expect("passive Design invitation");
    let current_revision = {
        let design = stamp(passive);
        lanes
            .bus
            .read(move |read| read.task_head(&design))
            .expect("statement reads")
            .expect("Design statement exists")
            .body()
            .revision
    };
    assert_eq!(
        invitation.notice.revision,
        wire::SequenceCode::new(current_revision.get()),
        "the private invitation is constructed from the current statement revision"
    );
    let background = invitation.notice.background();
    assert_eq!(background.occupant, test_uuid(FIRST + 1));
    assert_eq!(background.purpose, format!("design:{passive}"));
    assert!(background.fits(), "the complete invitation fits one kick");
}

fn assert_notice_identity(
    lanes: &Lanes,
    noticed: &[DesignNoticed],
    task: &wire::TaskHandle,
    passive: &wire::TaskHandle,
    held: &wire::TaskHandle,
) {
    let row = &noticed[0];
    assert_eq!(row.task, stamp(task));
    assert_eq!(row.terms, TERMS);
    let passive_stamp = stamp(passive);
    let passive_statement = lanes
        .bus
        .read(move |read| read.task_head(&passive_stamp))
        .expect("Design statement reads")
        .expect("Design exists")
        .position();
    assert_eq!(row.statement, passive_statement);
    assert!(
        noticed
            .iter()
            .all(|row| !(row.lane == test_uuid(SECOND) && row.design == stamp(held))),
        "a lane is never invited to the Design it holds"
    );
}

struct AwarenessFixture {
    task: wire::TaskHandle,
    passive: wire::TaskHandle,
    concluded: wire::TaskHandle,
    active: wire::TaskHandle,
    unrelated: wire::TaskHandle,
    listened: wire::TaskHandle,
    held: wire::TaskHandle,
}

fn awareness_fixture(lanes: &Lanes, config: &SurfaceConfig) -> AwarenessFixture {
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let passive = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let concluded = lanes.design("dVrD245Km", "Room catch-up retention", MATCHING);
    lanes.close_room(&concluded);
    let active = lanes.design("dVrD245Kn", "Room catch-up ordering", MATCHING);
    lanes.group(30, config, &active, true);
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
    AwarenessFixture {
        task,
        passive,
        concluded,
        active,
        unrelated,
        listened,
        held,
    }
}

fn reconcile_first(
    lanes: &Lanes,
    config: &SurfaceConfig,
    fixture: &AwarenessFixture,
) -> (Seats, Vec<DesignNoticed>) {
    let claim = lanes.claim_head(&fixture.task);
    let pending = lanes.pending(FIRST);
    let seated = claimant_seats();
    lanes.reconcile(config, &seated);
    let noticed = lanes.noticed();
    assert_eq!(
        lanes.told(FIRST, test_uuid(FIRST + 1)),
        [stamp(&fixture.passive), stamp(&fixture.held)],
        "only matching passive Designs are invited; not {}, {}, {}, or {}",
        fixture.unrelated,
        fixture.concluded,
        fixture.active,
        fixture.listened,
    );
    assert_notice_identity(
        lanes,
        &noticed,
        &fixture.task,
        &fixture.passive,
        &fixture.held,
    );
    assert_eq!(
        lanes.claim_head(&fixture.task),
        claim,
        "an invitation moves no claim"
    );
    assert_eq!(
        lanes.pending(FIRST),
        pending,
        "an invitation owes no acknowledgment"
    );
    assert!(
        !lanes.listens(config, &stamp(&fixture.passive), FIRST),
        "an invitation adds no room attention"
    );
    (seated, noticed)
}

fn reconcile_restart(
    lanes: &Lanes,
    config: &SurfaceConfig,
    fixture: &AwarenessFixture,
    noticed: &[DesignNoticed],
) -> Seats {
    let restarted = claimant_seats();
    lanes.reconcile(config, &restarted);
    assert_eq!(
        lanes.noticed(),
        noticed,
        "a restarted resident reads every invitation back and repeats none"
    );
    assert_awareness_projection(
        lanes,
        &fixture.passive,
        &fixture.unrelated,
        noticed,
        &fixture.task,
    );
    restarted
}

#[test]
fn a_claimant_occupant_is_invited_once_to_each_matching_passive_design() {
    let lanes = Lanes::new("design-awareness");
    let config = surface(&lanes);
    let fixture = awareness_fixture(&lanes, &config);
    assert_planned_invitation(&lanes, &config, &fixture.passive);
    let (mut seated, noticed) = reconcile_first(&lanes, &config, &fixture);
    let mut restarted = reconcile_restart(&lanes, &config, &fixture, &noticed);
    seated.release_all();
    restarted.release_all();
    lanes.finish();
}

#[test]
fn a_design_in_a_passive_group_receives_organic_invitations() {
    let lanes = Lanes::new("design-awareness-passive");
    let config = surface(&lanes);
    let design = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    lanes.group(50, &config, &design, true);
    lanes.stance(52, &config, wire::rooms::WorkingGroupStance::Passive);
    let _task = lanes.claimed("dVrD245Kc", FIRST);
    let occupant = test_uuid(FIRST + 1);
    let mut seated = seats(&[(FIRST, occupant)]);

    lanes.reconcile(&config, &seated);
    assert_eq!(lanes.told(FIRST, occupant), [stamp(&design)]);
    seated.release_all();
    lanes.finish();
}

#[test]
fn a_new_completion_invites_its_unchanged_occupant_without_restart_backfill() {
    let lanes = Lanes::new("design-awareness-completion");
    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(FIRST));
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design(
        "dVrD245Kd",
        "Cache invalidation cursor",
        "Keep retained cache invalidation evidence bounded.",
    );
    let mut observed = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    lanes.complete(
        &task,
        "Verified cache invalidation through the bounded cursor.",
    );
    let mut restarted = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("restarted completion cursor starts");
    let occupant = test_uuid(FIRST);
    let mut seated = seats(&[(FIRST, occupant)]);
    lanes.reconcile_completions(&config, &seated, &mut restarted);
    assert!(
        lanes.noticed().is_empty(),
        "a restarted resident does not backfill an earlier completion"
    );
    lanes.reconcile_completions(&config, &seated, &mut observed);
    assert_eq!(lanes.told(FIRST, occupant), [stamp(&design)]);
    lanes.reconcile_completions(&config, &seated, &mut observed);
    assert_eq!(
        lanes.told(FIRST, occupant),
        [stamp(&design)],
        "the completion is consumed once"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn completion_overlap_made_only_of_board_common_terms_invites_nothing() {
    let lanes = Lanes::new("design-awareness-completion-common");
    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(FIRST));
    let config = surface(&lanes);
    let task = lanes.seed_task(
        "dVrD245Kc",
        "Lane room cursor",
        Some("Retained lane room cursor."),
        None,
        Some(test_uuid(FIRST)),
    );
    let _design = lanes.design(
        "dVrD245Kd",
        "Room cursor lane",
        "Retained room cursor lane.",
    );
    for index in 0..8 {
        let stamp = wire::TaskStamp::for_micros(20_000 + index)
            .expect("fixture stamp")
            .to_string();
        lanes.seed_task(
            &stamp,
            "Lane room cursor",
            Some("Retained lane room cursor."),
            None,
            None,
        );
    }
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    lanes.complete(&task, "Retained lane room cursor.");
    let occupant = test_uuid(FIRST);
    let mut seated = seats(&[(FIRST, occupant)]);
    lanes.reconcile_completions(&config, &seated, &mut completions);
    assert!(
        lanes.noticed().is_empty(),
        "board-common completion terms provide no match weight"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn claim_and_completion_triggers_share_once_per_occupant_suppression() {
    {
        let lanes = Lanes::new("design-awareness-claim-then-completion");
        lanes.vacate(FIRST);
        lanes.renew(FIRST, test_uuid(FIRST));
        let config = surface(&lanes);
        let task = lanes.claimed("dVrD245Kc", FIRST);
        let design = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
        let occupant = test_uuid(FIRST);
        let mut seated = seats(&[(FIRST, occupant)]);
        lanes.reconcile(&config, &seated);
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("completion cursor starts");
        lanes.complete(&task, MATCHING);
        lanes.reconcile_completions(&config, &seated, &mut completions);
        assert_eq!(lanes.told(FIRST, occupant), [stamp(&design)]);
        seated.release_all();
        lanes.finish();
    }
    {
        let lanes = Lanes::new("design-awareness-completion-then-claim");
        lanes.vacate(FIRST);
        lanes.renew(FIRST, test_uuid(FIRST));
        let config = surface(&lanes);
        let task = lanes.claimed("dVrD245Kc", FIRST);
        let design = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
        let occupant = test_uuid(FIRST);
        let mut seated = seats(&[(FIRST, occupant)]);
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("completion cursor starts");
        lanes.complete(&task, MATCHING);
        lanes.reconcile_completions(&config, &seated, &mut completions);
        let _later = lanes.claimed("dVrD245Kp", FIRST);
        lanes.reconcile(&config, &seated);
        assert_eq!(lanes.told(FIRST, occupant), [stamp(&design)]);
        seated.release_all();
        lanes.finish();
    }
}

#[test]
fn a_completion_observation_refuses_after_the_design_statement_moves() {
    let lanes = Lanes::new("design-awareness-completion-race");
    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(FIRST));
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    lanes.complete(&task, MATCHING);
    let completed = completions.take(&lanes.bus).expect("completion page reads");
    let occupied = BTreeMap::from([(test_uuid(FIRST), test_uuid(FIRST))]);
    let planning = crate::resident::room_catch_up::Planning::load(&lanes.bus, &config)
        .expect("planning loads");
    let mut planned =
        crate::resident::design_awareness::plan(&lanes.bus, &planning, &occupied, &completed)
            .expect("completion invitation plans");
    assert_eq!(planned.len(), 1);
    lanes.retitle(&design, "A different statement");
    assert!(
        !crate::resident::design_awareness::record(&lanes.bus, &planned.remove(0))
            .expect("stale invitation is judged"),
        "a moved Design statement commits no completion invitation"
    );
    assert!(lanes.noticed().is_empty());
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
        crate::resident::design_awareness::plan(&lanes.bus, &planning, &occupied, &[])
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
    for (id, design) in [(32, &contributed), (36, &left)] {
        lanes.group(id, &config, design, true);
        lanes.group(id + 2, &config, design, false);
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

fn observed_invitation(
    lanes: &Lanes,
    config: &SurfaceConfig,
) -> (
    wire::TaskHandle,
    wire::TaskHandle,
    crate::resident::design_awareness::Told,
) {
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Room catch-up pagination", PITCH);
    let occupied = BTreeMap::from([(test_uuid(FIRST), test_uuid(FIRST + 1))]);
    let planning =
        crate::resident::room_catch_up::Planning::load(&lanes.bus, config).expect("planning loads");
    let mut planned =
        crate::resident::design_awareness::plan(&lanes.bus, &planning, &occupied, &[])
            .expect("invitation plans");
    assert_eq!(planned.len(), 1);
    (task, design, planned.remove(0))
}

#[test]
fn stale_claim_occupant_statement_and_standing_each_record_nothing() {
    {
        let lanes = Lanes::new("design-awareness-stale-claim");
        let config = surface(&lanes);
        let (task, _design, told) = observed_invitation(&lanes, &config);
        lanes.free(&task);
        assert!(
            !crate::resident::design_awareness::record(&lanes.bus, &told)
                .expect("stale claim is judged")
        );
        assert!(lanes.noticed().is_empty());
        lanes.finish();
    }
    {
        let lanes = Lanes::new("design-awareness-stale-occupant");
        let config = surface(&lanes);
        let (_task, _design, told) = observed_invitation(&lanes, &config);
        lanes.vacate(FIRST);
        lanes.renew(FIRST, test_uuid(40));
        assert!(
            !crate::resident::design_awareness::record(&lanes.bus, &told)
                .expect("stale occupant is judged")
        );
        assert!(lanes.noticed().is_empty());
        lanes.finish();
    }
    {
        let lanes = Lanes::new("design-awareness-stale-statement");
        let config = surface(&lanes);
        let (_task, design, told) = observed_invitation(&lanes, &config);
        lanes.retitle(&design, "Revised room catch-up pagination");
        assert!(
            !crate::resident::design_awareness::record(&lanes.bus, &told)
                .expect("stale statement is judged")
        );
        assert!(lanes.noticed().is_empty());
        lanes.finish();
    }
    {
        let lanes = Lanes::new("design-awareness-stale-standing");
        let config = surface(&lanes);
        let (_task, design, told) = observed_invitation(&lanes, &config);
        lanes.group(50, &config, &design, true);
        assert!(
            !crate::resident::design_awareness::record(&lanes.bus, &told)
                .expect("stale standing is judged")
        );
        assert!(lanes.noticed().is_empty());
        lanes.finish();
    }
}

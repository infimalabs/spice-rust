//! Completion-bound Design requests are durable, singular, and released by
//! either substantive room publication or a Design standing change.

use wire::rooms::DesignNoticed;

use super::*;

const PITCH: &str = "Lanes page retained publications through bounded cursors.";
const MATCHING: &str = "Retained publications page through bounded cursors.";
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

fn claimant_seats() -> Seats {
    seats(&[(FIRST, test_uuid(FIRST))])
}

fn stamp(handle: &wire::TaskHandle) -> wire::TaskStamp {
    handle.born().expect("fixture handle has a birth")
}

fn surface(lanes: &Lanes) -> SurfaceConfig {
    let repository = Repository::discover(&lanes.root).expect("fixture repository resolves");
    load_startup_surface(&repository)
        .expect("fixture surface loads")
        .config
}

fn align_completion_lane(lanes: &Lanes) {
    lanes.vacate(FIRST);
    lanes.renew(FIRST, test_uuid(FIRST));
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

    fn design(&self, stamp: &str, title: &str, description: &str) -> wire::TaskHandle {
        self.design_with_questions(stamp, title, description, &[])
    }

    fn design_with_questions(
        &self,
        stamp: &str,
        title: &str,
        description: &str,
        questions: &[&str],
    ) -> wire::TaskHandle {
        self.seed_task_with_acceptance(
            stamp,
            title,
            Some(description),
            Some(wire::TaskIntent::Design),
            None,
            questions,
        )
    }

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

    /// Commit one statement edit giving `task` a new title.
    fn retitle(&self, handle: &wire::TaskHandle, title: &str) {
        let task = stamp(handle);
        let statement = self
            .bus
            .read({
                let task = task.clone();
                move |read| read.task_head(&task)
            })
            .expect("statement reads")
            .expect("statement exists")
            .into_body();
        let title = title.to_owned();
        let mut edit =
            write(Actor::Operator, self.epoch, Producer::CommandDoor).expect("edit write starts");
        let edited = edit
            .fact(
                wire::body::TaskEdited {
                    intent: None,
                    evidence: None,
                    task,
                    revision: 2,
                    dependencies: None,
                },
                FactCause::Root,
            )
            .expect("edit fact is valid");
        edit.task_head(edited, move |position| wire::TaskHead {
            title,
            revision: position,
            ..statement
        });
        commit(&self.bus, edit).expect("edit commits");
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

    /// Post one operator message to `design` and return the listeners its
    /// reply reports.
    fn post_listeners(
        &self,
        id: u8,
        config: &SurfaceConfig,
        design: &wire::TaskHandle,
    ) -> Vec<CanonicalUuid> {
        let request = wire::rooms::RoomPost {
            room: design.clone(),
            text: "Operator input for the room.".to_owned(),
            grooming: None,
        };
        let task = stamp(design);
        let statement = self
            .bus
            .read(move |read| read.task_head(&task))
            .expect("Design head reads")
            .expect("the Design exists");
        let command = self.command(
            id,
            Actor::Operator,
            CommandKind::RoomPost,
            vec![Witness::Task {
                task: stamp(design),
                head: statement.position(),
            }],
            &request,
        );
        let outcome = crate::board_command::dispatch_with_repository(
            &self.bus,
            &command,
            None,
            &config.projects,
            &config.allocation,
        )
        .expect("room post dispatches");
        let Outcome::Reply(wire::Reply {
            body: wire::intent::CommandAnswer::RoomPost(answer),
            ..
        }) = outcome
        else {
            panic!("room post commits: {outcome:?}");
        };
        match answer.outcome {
            wire::rooms::RoomPostOutcome::RecordedWithListeners { listeners } => {
                listeners.iter().map(|listener| listener.lane).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Stand `design` in the fixture working group, then select its stance.
    fn group(
        &self,
        id: u8,
        config: &SurfaceConfig,
        design: &wire::TaskHandle,
        stance: wire::rooms::WorkingGroupStance,
    ) {
        let revision = self
            .bus
            .read(|read| read.group_partition())
            .expect("partition reads")
            .map_or(0, |head| head.body().revision);
        let request = wire::rooms::WorkingGroupMove {
            design: stamp(design),
            to: Some(
                wire::rooms::RoomAuthorityStamp::for_micros(1_750_000_000_000_000)
                    .expect("fixture group stamp"),
            ),
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
        if stance != wire::rooms::WorkingGroupStance::Passive {
            self.stance(id.saturating_add(1), config, stance);
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

    fn reconcile_completions(
        &self,
        config: &SurfaceConfig,
        seats: &Seats,
        completions: &mut crate::resident::design_awareness::CompletionCursor,
    ) {
        let mut diagnostics = Vec::new();
        crate::resident::design_awareness::reconcile(
            &self.bus,
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

    fn next(&self, config: &SurfaceConfig, lane: u8) -> wire::intent::NextWorkState {
        let observed = self.bus.task_snapshot().expect("task snapshot reads");
        crate::board_command::read_next_work(
            &self.bus,
            &observed,
            Actor::Lane(test_uuid(lane)),
            &config.projects,
            &config.allocation,
        )
        .expect("next-work projection succeeds")
        .next
        .expect("next-work projection returns a state")
    }

    fn request(&self, config: &SurfaceConfig, lane: u8) -> Option<wire::intent::DesignRequest> {
        match self.next(config, lane) {
            wire::intent::NextWorkState::DesignRequest { request } => Some(request),
            _ => None,
        }
    }
}

#[test]
fn only_a_completion_owes_the_best_single_design_request() {
    let lanes = Lanes::new("design-request-completion");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let best = lanes.design_with_questions(
        "dVrD245Kd",
        "Page room catch-up",
        "Problem. Lanes page retained publications through bounded cursors.",
        &[
            "Should each lane retain its own cursor?",
            "Which publications belong on the bounded page?",
        ],
    );
    let _lesser = lanes.design(
        "dVrD245Kg",
        "Page cursor proposal",
        "Retained cursor notes for one lane.",
    );

    assert!(
        lanes.noticed().is_empty(),
        "a live claim records no request"
    );
    assert!(
        matches!(
            lanes.next(&config, FIRST),
            wire::intent::NextWorkState::Active { .. }
        ),
        "a live claim remains the next-work answer"
    );

    lanes.complete(&task, MATCHING);
    let request = lanes
        .request(&config, FIRST)
        .expect("completion returns its owed Design");
    assert_eq!(request.design, best);
    assert_eq!(request.task, task);
    assert!(!request.terms.is_empty());
    assert_eq!(
        request.pitch,
        "Lanes page retained publications through bounded cursors."
    );
    assert_eq!(
        request.questions,
        [
            "Should each lane retain its own cursor?",
            "Which publications belong on the bounded page?"
        ]
    );
    assert!(lanes.request(&config, FIRST).is_some());
    assert_eq!(
        lanes.noticed().len(),
        1,
        "one completion records at most one request"
    );
    lanes.finish();
}

#[test]
fn resident_and_next_use_the_same_completed_statement_window() {
    let mut selected = None;
    for (tag, next_first) in [
        ("design-request-common-next-first", true),
        ("design-request-common-resident-first", false),
    ] {
        let lanes = Lanes::new(tag);
        align_completion_lane(&lanes);
        let config = surface(&lanes);
        for index in 0..8 {
            let stamp = wire::TaskStamp::for_micros(20_000 + index)
                .expect("fixture stamp")
                .to_string();
            let task = lanes.seed_task(
                &stamp,
                "Lane room cursor retained publication bounded",
                None,
                None,
                Some(test_uuid(FIRST)),
            );
            lanes.complete(&task, "fixture completion");
        }
        let task = lanes.seed_task(
            "dVrD245Kc",
            "Lane room cursor retained publication bounded",
            Some("Catchup pagination snapshot."),
            None,
            Some(test_uuid(FIRST)),
        );
        let _generic = lanes.design(
            "dVrD245Kd",
            "Lane room cursor retained publication bounded",
            "Lane room cursor retained publication bounded.",
        );
        let distinctive = lanes.design(
            "dVrD245Km",
            "Catchup pagination snapshot",
            "Catchup pagination snapshot.",
        );
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("completion cursor starts");
        let mut seated = claimant_seats();

        lanes.complete(&task, "Lane room cursor retained publication bounded.");
        let request = if next_first {
            let request = lanes
                .request(&config, FIRST)
                .expect("task next chooses the distinctive Design");
            lanes.reconcile_completions(&config, &seated, &mut completions);
            request
        } else {
            lanes.reconcile_completions(&config, &seated, &mut completions);
            lanes
                .request(&config, FIRST)
                .expect("resident choice is returned by task next")
        };

        assert_eq!(request.design, distinctive);
        assert_eq!(request.terms, ["catchup", "pagination", "snapshot"]);
        let notices = lanes.noticed();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].design, stamp(&distinctive));
        assert_eq!(notices[0].terms, request.terms);
        if let Some(previous) = &selected {
            assert_eq!(
                previous, &request.terms,
                "both execution orders choose the same matched terms"
            );
        } else {
            selected = Some(request.terms);
        }
        seated.release_all();
        lanes.finish();
    }
}

#[test]
fn resident_and_next_orders_record_one_request() {
    for (tag, next_first) in [
        ("design-request-next-first", true),
        ("design-request-resident-first", false),
    ] {
        let lanes = Lanes::new(tag);
        align_completion_lane(&lanes);
        let config = surface(&lanes);
        let task = lanes.claimed("dVrD245Kc", FIRST);
        let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("completion cursor starts");
        let mut seated = claimant_seats();
        lanes.complete(&task, MATCHING);
        let request = if next_first {
            assert!(lanes.noticed().is_empty());
            let request = lanes
                .request(&config, FIRST)
                .expect("task next records and returns the request");
            lanes.reconcile_completions(&config, &seated, &mut completions);
            request
        } else {
            lanes.reconcile_completions(&config, &seated, &mut completions);
            lanes
                .request(&config, FIRST)
                .expect("task next returns the resident's request")
        };
        assert_eq!(request.design, design);
        assert_eq!(lanes.noticed().len(), 1);
        seated.release_all();
        lanes.finish();
    }
}

#[test]
fn contribution_discharges_the_request_and_suppresses_that_design_forever() {
    let lanes = Lanes::new("design-request-contribution");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    lanes.complete(&task, MATCHING);
    assert!(lanes.request(&config, FIRST).is_some());

    lanes.contribute(50, &config, FIRST, &design);
    assert!(lanes.request(&config, FIRST).is_none());
    let later = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&later, MATCHING);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a lane is never asked about a Design it published in"
    );
    lanes.finish();
}

#[test]
fn an_asked_lane_hears_later_posts_until_it_leaves_and_is_never_asked_again() {
    let lanes = Lanes::new("design-request-listening");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    assert!(lanes.post_listeners(70, &config, &design).is_empty());

    lanes.complete(&task, MATCHING);
    assert!(lanes.request(&config, FIRST).is_some());
    assert_eq!(
        lanes.post_listeners(71, &config, &design),
        vec![test_uuid(FIRST)]
    );

    lanes.leave(72, &config, FIRST, &design);
    assert!(lanes.post_listeners(73, &config, &design).is_empty());
    let later = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&later, MATCHING);
    assert!(lanes.request(&config, FIRST).is_none());
    assert!(lanes.post_listeners(74, &config, &design).is_empty());
    lanes.finish();
}

#[test]
fn active_standing_releases_an_unanswered_request() {
    let lanes = Lanes::new("design-request-standing");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let _other = lanes.design("dVrD245Kg", "Page room catch-up", PITCH);
    lanes.group(
        70,
        &config,
        &design,
        wire::rooms::WorkingGroupStance::Passive,
    );
    lanes.complete(&task, MATCHING);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("the completion initially owes its best Design")
            .design,
        design
    );

    lanes.stance(72, &config, wire::rooms::WorkingGroupStance::Explore);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "an active Design no longer owes the passive request"
    );
    lanes.finish();
}

#[test]
fn a_design_edit_keeps_the_request_owed() {
    let lanes = Lanes::new("design-request-edit");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    lanes.complete(&task, MATCHING);
    let before = lanes
        .request(&config, FIRST)
        .expect("the completion owes its Design");

    lanes.retitle(&design, "Page room catch-up by cursor");
    let after = lanes
        .request(&config, FIRST)
        .expect("an edited Design still owes the request");
    assert_eq!(after.design, design);
    assert_eq!(after.title, "Page room catch-up by cursor");
    assert_ne!(after.statement, before.statement);
    assert_eq!(
        lanes.noticed().len(),
        1,
        "the edit records no second request"
    );
    lanes.finish();
}

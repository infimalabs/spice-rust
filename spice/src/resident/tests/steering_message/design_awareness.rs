//! Session-profile Design requests are durable, singular, replaced only by a
//! strictly better match, and released by substantive room publication, source
//! replacement, or a Design statement change.

use wire::rooms::DesignNoticed;

use super::*;

const PITCH: &str = "Lanes page retained publications through bounded cursors.";
const MATCHING: &str = "Retained publications page through bounded cursors.";
const TASK_TITLE: &str = "Page room catch-up";
const TASK_DESCRIPTION: &str = "Bound the cursor of retained publications per lane.";

#[path = "design_awareness/items.rs"]
mod items;
#[path = "design_awareness/replacement.rs"]
mod replacement;

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
    lanes.declare(FIRST);
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
                    disposition: None,
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
    fn compact(&self, lane: u8) -> Seq {
        let mut write = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("compaction write starts");
        write
            .fact(
                wire::body::LaneCompacted {
                    lane: test_uuid(lane),
                    occupant: current_residency(&self.bus, self.epoch)
                        .expect("residency reads")
                        .body()
                        .lanes
                        .iter()
                        .find(|row| row.lane == test_uuid(lane))
                        .and_then(|row| row.occupant)
                        .expect("the lane has an occupant"),
                    stream: "stdout".to_owned(),
                    member: 0,
                    offset: 0,
                    length: 1,
                },
                FactCause::Root,
            )
            .expect("compaction fact is valid");
        let landed = commit(&self.bus, write).expect("compaction commits");
        Seq::new(landed.first()).expect("compaction has a position")
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

    fn contribute(&self, id: u8, lane: u8, design: &wire::TaskHandle) {
        self.contribute_kind(
            id,
            lane,
            design,
            wire::rooms::RoomContributionKind::Question,
            None,
        );
    }

    fn contribute_kind(
        &self,
        id: u8,
        lane: u8,
        design: &wire::TaskHandle,
        kind: wire::rooms::RoomContributionKind,
        question: Option<wire::rooms::RoomQuestion>,
    ) {
        let request = wire::intent::RoomContribute {
            room: design.clone(),
            kind,
            question,
            text: "Does the catch-up page already bound its cursor?".to_owned(),
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
        let outcome = crate::board_command::room_contribute(&self.bus, &command, &request, None);
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "contribution commits: {outcome:?}"
        );
    }

    fn leave(&self, id: u8, lane: u8, design: &wire::TaskHandle) {
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
        let outcome = crate::board_command::room_leave(&self.bus, &command, &request);
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
            &config.allocation,
            None,
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

    /// Stand `design` in the fixture working group.
    fn group(&self, id: u8, config: &SurfaceConfig, design: &wire::TaskHandle) {
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
        let outcome = crate::board_command::dispatch_with_repository(
            &self.bus,
            &command,
            None,
            &config.allocation,
            None,
        )
        .expect("group move dispatches");
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "group move commits: {outcome:?}"
        );
    }

    /// The operator's indefinite deferral of `design`.
    fn defer(&self, id: u8, config: &SurfaceConfig, design: &wire::TaskHandle) {
        let task = stamp(design);
        let workflow = self
            .bus
            .read(move |read| read.workflow_head(&task))
            .expect("workflow reads")
            .expect("the Design exists");
        let command = self.command(
            id,
            Actor::Operator,
            CommandKind::TaskDefer,
            vec![
                Witness::Workflow {
                    task: stamp(design),
                    head: workflow.position(),
                    phase: workflow.body().phase.clone(),
                },
                Witness::Deferral {
                    task: stamp(design),
                    head: workflow.position(),
                    deferred: false,
                },
            ],
            &wire::intent::TaskDefer {
                task: design.clone(),
                wait: wire::Instant::indefinite(),
            },
        );
        let wire::intent::Intent::TaskDefer(request) = command.body() else {
            panic!("fixture command carries a deferral");
        };
        let (outcome, _) =
            crate::board_command::task_defer(&self.bus, &command, request, None, &config.steering);
        assert!(
            matches!(outcome, Outcome::Reply(_)),
            "deferral commits: {outcome:?}"
        );
    }

    fn reconcile_completions(
        &self,
        seats: &Seats,
        completions: &mut crate::resident::design_awareness::CompletionCursor,
    ) {
        let profiles =
            std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
        self.reconcile_profiles(seats, completions, &profiles);
    }

    fn reconcile_profiles(
        &self,
        seats: &Seats,
        completions: &mut crate::resident::design_awareness::CompletionCursor,
        profiles: &std::sync::Mutex<crate::resident::message_surface::MessageState>,
    ) {
        let mut diagnostics = Vec::new();
        crate::resident::design_awareness::reconcile(
            &self.bus,
            seats,
            completions,
            profiles,
            &mut diagnostics,
        )
        .expect("Design awareness reconciles");
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
            &config.allocation,
            None,
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
fn an_unsteeped_design_invites_wide_grouped_or_not() {
    for (fixture, grouped) in [
        ("design-request-ungrouped", false),
        ("design-request-grouped", true),
    ] {
        let lanes = Lanes::new(fixture);
        align_completion_lane(&lanes);
        let config = surface(&lanes);
        let task = lanes.claimed("dVrD245Kc", FIRST);
        let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
        if grouped {
            lanes.group(30, &config, &design);
        }
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("completion cursor starts");
        let mut seated = claimant_seats();

        lanes.complete(&task, MATCHING);
        lanes.reconcile_completions(&seated, &mut completions);
        let request = lanes
            .request(&config, FIRST)
            .expect("a filed open Design can be requested grouped or not");
        assert_eq!(request.design, design, "grouped {grouped}");
        assert_eq!(
            request.reach,
            Some(wire::rooms::DesignReach::Wide),
            "grouped {grouped} moved the reach"
        );
        seated.release_all();
        lanes.finish();
    }
}

#[test]
fn a_completion_profile_opportunity_selects_the_best_single_design() {
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
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();

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
    lanes.reconcile_completions(&seated, &mut completions);
    let request = lanes
        .request(&config, FIRST)
        .expect("completion returns its owed Design");
    assert_eq!(request.design, best);
    assert!(matches!(
        request.evidence,
        wire::rooms::DesignNoticeEvidence::Task {
            task: ref evidence_task,
            ..
        } if evidence_task == &task.born().expect("fixture task has a birth")
    ));
    assert!(!request.terms.is_empty());
    assert_eq!(request.pitch, "Rooms need work to talk about.");
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
    seated.release_all();
    lanes.finish();
}

#[test]
fn profile_selection_uses_the_shared_tfidf_statement_window() {
    let lanes = Lanes::new("design-request-common-profile");
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
        "Lane notes",
        "A lane retains ordinary room notes.",
    );
    let distinctive = lanes.design(
        "dVrD245Km",
        "Catchup pagination snapshot",
        "Catchup pagination snapshot.",
    );
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();

    lanes.complete(&task, "Catchup pagination snapshot.");
    lanes.reconcile_completions(&seated, &mut completions);
    let request = lanes
        .request(&config, FIRST)
        .expect("resident choice is returned by task next");

    assert_eq!(request.design, distinctive);
    assert_eq!(request.terms, ["catchup", "pagination", "snapshot"]);
    let notices = lanes.noticed();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].design, stamp(&distinctive));
    assert_eq!(notices[0].terms, request.terms);
    seated.release_all();
    lanes.finish();
}

#[test]
fn resident_records_one_request_before_task_next_reads_it() {
    let lanes = Lanes::new("design-request-resident-first");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    lanes.complete(&task, MATCHING);
    lanes.reconcile_completions(&seated, &mut completions);
    let request = lanes
        .request(&config, FIRST)
        .expect("task next returns the resident's request");
    assert_eq!(request.design, design);
    assert_eq!(lanes.noticed().len(), 1);
    seated.release_all();
    lanes.finish();
}

#[test]
fn contribution_discharges_the_request_for_the_continuing_session() {
    let lanes = Lanes::new("design-request-contribution");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(lanes.request(&config, FIRST).is_some());

    lanes.contribute(50, FIRST, &design);
    assert!(lanes.request(&config, FIRST).is_none());
    let later = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&later, &format!("{MATCHING} Paged again."));
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a continuing session is not asked again about a Design it published in"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn compaction_preserves_participation_and_settled_request_history() {
    let lanes = Lanes::new("design-request-compaction-participation");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    lanes.complete(&first, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert_eq!(lanes.noticed().len(), 1);
    lanes.contribute(50, FIRST, &design);
    assert!(lanes.request(&config, FIRST).is_none());

    let compacted = lanes.compact(FIRST);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    let snapshot = lanes.bus.task_snapshot().expect("task snapshot reads");
    let context = snapshot.facts();
    let first_notice = lanes.noticed().into_iter().next().expect("first notice");
    assert_eq!(
        context.session_compaction(test_uuid(FIRST), &first_notice.session),
        Some(compacted)
    );
    assert!(context.design_was_asked_in_session(
        test_uuid(FIRST),
        &first_notice.session,
        &stamp(&design),
        None,
    ));
    assert!(context.design_ask_answered_in_session(
        test_uuid(FIRST),
        &first_notice.session,
        &stamp(&design),
        None,
    ));

    let after = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&after, &format!("{MATCHING} Paged again."));
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert_eq!(
        lanes.noticed().len(),
        1,
        "a later opportunity cannot turn participation back into recruitment"
    );
    assert!(lanes.request(&config, FIRST).is_none());
    seated.release_all();
    lanes.finish();
}

#[test]
fn compaction_preserves_one_unanswered_request() {
    let lanes = Lanes::new("design-request-compaction-outstanding");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    lanes.complete(&first, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    let expected = lanes
        .request(&config, FIRST)
        .map(|request| spice::rooms::awareness::request_text(&request))
        .expect("the first opportunity records one request");

    lanes.compact(FIRST);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    let after = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&after, &format!("{MATCHING} Paged after compaction."));
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);

    assert_eq!(lanes.noticed().len(), 1);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .map(|request| spice::rooms::awareness::request_text(&request)),
        Some(expected),
        "compaction and later opportunities preserve the exact unanswered request"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn renewal_with_a_fresh_vendor_identity_can_reoffer_a_settled_design() {
    let lanes = Lanes::new("design-request-renewal-decay");
    align_completion_lane(&lanes);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let mut seated = claimant_seats();

    lanes.complete(&first, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    lanes.leave(60, FIRST, &design);
    assert_eq!(lanes.noticed().len(), 1);

    seated.release_all();
    let successor = test_uuid(FIRST.saturating_add(40));
    lanes.vacate(FIRST);
    lanes.renew(FIRST, successor);
    lanes.declare_identity(FIRST, "renewed-session".to_owned());
    seated = seats(&[(FIRST, successor)]);
    let after = lanes.seed_task(
        "dVrD245Kp",
        "Bound a renewed room cursor",
        Some("Retain a renewed cursor over room publications."),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&after, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);

    let notices = lanes.noticed();
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[1].session.identity, "renewed-session");
    assert_eq!(notices[1].compaction, None);
    seated.release_all();
    lanes.finish();
}

#[test]
fn occupant_replacement_and_restart_preserve_same_context_suppression() {
    let lanes = Lanes::new("design-request-resume-suppression");
    align_completion_lane(&lanes);
    let first = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let mut seated = claimant_seats();

    lanes.complete(&first, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    lanes.leave(70, FIRST, &design);
    assert_eq!(lanes.noticed().len(), 1);

    seated.release_all();
    let successor = test_uuid(FIRST.saturating_add(41));
    lanes.vacate(FIRST);
    lanes.renew(FIRST, successor);
    lanes.declare_identity(FIRST, test_uuid(FIRST.saturating_add(20)).to_string());
    seated = seats(&[(FIRST, successor)]);

    let mut restarted = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("restarted completion cursor starts");
    let restarted_profiles =
        std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let after = lanes.seed_task(
        "dVrD245Kp",
        "Bound a resumed room cursor",
        Some("Retain a resumed cursor over room publications."),
        None,
        Some(test_uuid(FIRST)),
    );
    lanes.complete(&after, MATCHING);
    lanes.reconcile_profiles(&seated, &mut restarted, &restarted_profiles);
    assert_eq!(
        lanes.noticed().len(),
        1,
        "the same vendor identity and compaction generation remain suppressed"
    );
    seated.release_all();
    lanes.finish();
}

/// Plan room catch-up for the seated lanes and read FIRST's pending notice.
fn pending_catch_up(
    lanes: &Lanes,
    seated: &Seats,
    diagnostics: &mut Vec<u8>,
) -> Option<room_catch_up::Notice> {
    room_catch_up::reconcile(&lanes.bus, &lanes.paths, seated, diagnostics)
        .expect("room catch-up reconciles");
    room_catch_up::read_notice(&room_catch_up::notice_path(&lanes.paths, test_uuid(FIRST)))
}

/// The owed request reaches its seated occupant as the same text `task next`
/// shows, naming the supporting work by its claim phase, exact source, and
/// authored excerpt. Once that delivery lands the cursor passes it, so
/// catch-up never repeats it, while `task next` keeps it owed until the lane
/// answers.
#[test]
fn room_catch_up_delivers_the_owed_request_once_while_task_next_keeps_it() {
    let lanes = Lanes::new("design-request-delivery");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut seated = claimant_seats();
    let mut diagnostics = Vec::new();
    assert!(
        pending_catch_up(&lanes, &seated, &mut diagnostics).is_none(),
        "nothing is owed before completion"
    );
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_completions(&seated, &mut completions);

    let request = lanes
        .request(&config, FIRST)
        .expect("task next shows the owed request");
    let evidence_task = task.born().expect("fixture task has a birth");
    let recorded = lanes.noticed();
    let wire::rooms::DesignNoticeEvidence::Task {
        task: ref recorded_task,
        statement,
        completed,
        ..
    } = recorded[0].evidence
    else {
        panic!("a completion records task evidence");
    };
    assert_eq!(recorded_task, &evidence_task);
    let notice =
        pending_catch_up(&lanes, &seated, &mut diagnostics).expect("the seated occupant is told");
    assert_eq!(notice.occupant, test_uuid(FIRST));
    let shown = spice::rooms::awareness::request_text(&request);
    assert!(
        notice.text.contains(&shown),
        "catch-up carries the request task next shows: {}",
        notice.text
    );
    assert!(
        notice
            .text
            .contains(&format!("leave: spice room leave {design} --clause TEXT\n")),
        "the owed request's room offers its leave: {}",
        notice.text
    );
    for expected in [
        format!(
            "Supporting work: you implemented task {evidence_task} (statement revision {}, \
             completion {}): \"{MATCHING}\".",
            wire::SequenceCode::new(statement.get()),
            wire::SequenceCode::new(completed.get()),
        ),
        format!("`spice task show {evidence_task} --history`"),
    ] {
        assert!(shown.contains(&expected), "{expected} is missing: {shown}");
    }

    land_catch_up(&lanes, &notice, &mut diagnostics);
    assert!(
        pending_catch_up(&lanes, &seated, &mut diagnostics).is_none(),
        "a landed invitation is not delivered again"
    );
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .map(|owed| spice::rooms::awareness::request_text(&owed)),
        Some(spice::rooms::awareness::request_text(&request)),
        "task next keeps the delivered request owed"
    );
    seated.release_all();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    lanes.finish();
}

/// Land FIRST's catch-up `notice` in its transcript and recognize it, moving
/// the lane's cursor past what it carried.
fn land_catch_up(lanes: &Lanes, notice: &room_catch_up::Notice, diagnostics: &mut Vec<u8>) {
    let mut landed = lanes.operator(FIRST, &notice.text);
    landed.occupant = notice.occupant;
    let _ = ingest_transcripts(
        &lanes.bus,
        &lanes.paths,
        Some(test_uuid(FIRST)),
        lane::ingest::Bound::IMMEDIATE,
        lane::ingest::Coalescing::default(),
        diagnostics,
    )
    .expect("the delivery is accounted");
    let source = lanes
        .tape(FIRST)
        .source_id()
        .expect("tape source reads")
        .as_str()
        .parse()
        .expect("tape source is a UUID");
    room_catch_up::recognize(&lanes.bus, &lanes.paths, &landed, source)
        .expect("the delivery is recognized")
        .expect("the landed delivery advances the cursor");
}

/// A new occupant resuming the same session is told a still-owed request
/// once, though its predecessor's landed delivery moved the cursor past it.
#[test]
fn a_resumed_occupant_is_told_the_owed_request_once() {
    let lanes = Lanes::new("design-request-resumed-occupant");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut seated = claimant_seats();
    let mut diagnostics = Vec::new();
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let task = lanes.claimed("dVrD245Kc", FIRST);
    lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    lanes.complete(&task, MATCHING);
    lanes.reconcile_completions(&seated, &mut completions);
    let notice =
        pending_catch_up(&lanes, &seated, &mut diagnostics).expect("the first occupant is told");
    land_catch_up(&lanes, &notice, &mut diagnostics);

    seated.release_all();
    let successor = test_uuid(FIRST.saturating_add(42));
    lanes.vacate(FIRST);
    lanes.renew(FIRST, successor);
    lanes.declare(FIRST);
    seated = seats(&[(FIRST, successor)]);
    let restart = |seated: &Seats| {
        let mut completions =
            crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
                .expect("restarted completion cursor starts");
        lanes.reconcile_completions(seated, &mut completions);
    };
    restart(&seated);
    let notices = lanes.noticed();
    assert_eq!(
        notices.len(),
        2,
        "the resumed occupant is owed a restatement"
    );
    assert_eq!(notices[1].occupant, successor);
    assert_eq!(notices[1].design, notices[0].design);
    assert!(matches!(
        notices[1].opportunity,
        wire::rooms::DesignNoticeOpportunity::SessionResumed { .. }
    ));
    let restated = lanes
        .request(&config, FIRST)
        .expect("task next shows the restated request");
    let resumed =
        pending_catch_up(&lanes, &seated, &mut diagnostics).expect("the resumed occupant is told");
    assert_eq!(resumed.occupant, successor);
    let shown = spice::rooms::awareness::request_text(&restated);
    assert!(resumed.text.contains(&shown), "{}", resumed.text);
    restart(&seated);
    assert_eq!(lanes.noticed().len(), 2, "one occupant is told once");
    seated.release_all();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
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
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    assert!(lanes.post_listeners(70, &config, &design).is_empty());

    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(lanes.request(&config, FIRST).is_some());
    assert_eq!(
        lanes.post_listeners(71, &config, &design),
        vec![test_uuid(FIRST)]
    );

    lanes.leave(72, FIRST, &design);
    assert!(lanes.post_listeners(73, &config, &design).is_empty());
    let later = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&later, &format!("{MATCHING} Paged after leaving."));
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(lanes.request(&config, FIRST).is_none());
    assert!(lanes.post_listeners(74, &config, &design).is_empty());
    seated.release_all();
    lanes.finish();
}

#[test]
fn deferring_an_open_design_ends_its_request_and_its_listening() {
    let lanes = Lanes::new("design-request-deferred");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(lanes.request(&config, FIRST).is_some());
    assert_eq!(
        lanes.post_listeners(70, &config, &design),
        vec![test_uuid(FIRST)]
    );

    lanes.defer(71, &config, &design);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a deferred Design is owed to no lane"
    );
    assert!(
        lanes.post_listeners(72, &config, &design).is_empty(),
        "no lane keeps listening for a deferred Design"
    );

    let later = lanes.claimed("dVrD245Kp", FIRST);
    lanes.complete(&later, &format!("{MATCHING} Paged while deferred."));
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a deferred Design is never invited"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn a_design_edit_releases_the_request_without_reopening_it() {
    let lanes = Lanes::new("design-request-edit");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let task = lanes.claimed("dVrD245Kc", FIRST);
    let design = lanes.design("dVrD245Kd", "Page room catch-up", PITCH);
    let mut completions = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("completion cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.complete(&task, MATCHING);
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.request(&config, FIRST).is_some(),
        "the completion owes its Design"
    );

    lanes.retitle(&design, "Page room catch-up by cursor");
    lanes.reconcile_profiles(&seated, &mut completions, &profiles);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "a replaced Design statement releases the exact request"
    );
    assert_eq!(
        lanes.noticed().len(),
        1,
        "the same session and Design never reopen after an edit"
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn conversation_evidence_predating_a_design_is_selected_when_the_design_is_filed() {
    let lanes = Lanes::new("design-request-conversation-before-design");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());

    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    lanes.account(FIRST);
    let source = lanes
        .bus
        .read(|read| read.transcript_source_heads(&test_uuid(FIRST)))
        .expect("transcript sources read")
        .into_iter()
        .next()
        .expect("declared lane has a transcript source")
        .into_body()
        .source;
    let prose = "Windows native runner validates archive compression headers libraries toolchain \
                 manifests receipts deployment replay smoke";
    let conversation = spice::rooms::evidence::Conversation::read(prose)
        .expect("fixture prose is substantive evidence");
    {
        let mut state =
            crate::resident::message_surface::lock_state(&profiles).expect("profile state locks");
        assert_eq!(
            state.profiles.insert(
                test_uuid(FIRST),
                test_uuid(FIRST),
                spice::rooms::evidence::Evidence::conversation(
                    spice::rooms::profile::EvidenceKey::Conversation {
                        source,
                        member: 0,
                        offset: 0,
                        length: 1,
                    },
                    &conversation,
                ),
            ),
            spice::rooms::profile::Inserted::Stored
        );
    }
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);

    let design = lanes.design(
        "dVrD245Kd",
        "Validate Windows native runner archives",
        "Compression headers libraries toolchain manifests receipts deployment replay smoke.",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);

    let request = lanes
        .request(&config, FIRST)
        .expect("Design filing evaluates retained conversation evidence");
    assert_eq!(request.design, design);
    assert!(matches!(
        request.opportunity,
        wire::rooms::DesignNoticeOpportunity::DesignStatement
    ));
    assert!(matches!(
        request.evidence,
        wire::rooms::DesignNoticeEvidence::Conversation { .. }
    ));
    lanes.contribute(80, FIRST, &design);
    let message_design = lanes.design(
        "dVrD245Km",
        "Repair database migration replay",
        "Indexes transactions snapshots rollback consistency schema storage locks recovery checks.",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let message = spice::rooms::evidence::Conversation::read(
        "Database migration replay indexes transactions snapshots rollback consistency schema \
         storage locks recovery checks",
    )
    .expect("second fixture prose is substantive evidence");
    {
        let mut state =
            crate::resident::message_surface::lock_state(&profiles).expect("profile state locks");
        assert_eq!(
            state.profiles.insert(
                test_uuid(FIRST),
                test_uuid(FIRST),
                spice::rooms::evidence::Evidence::conversation(
                    spice::rooms::profile::EvidenceKey::Conversation {
                        source,
                        member: 0,
                        offset: 1,
                        length: 1,
                    },
                    &message,
                ),
            ),
            spice::rooms::profile::Inserted::Stored
        );
    }
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    let request = lanes
        .request(&config, FIRST)
        .expect("settled conversation evaluates the session profile");
    assert_eq!(request.design, message_design);
    assert!(matches!(
        request.opportunity,
        wire::rooms::DesignNoticeOpportunity::Conversation { .. }
    ));
    assert!(matches!(
        request.evidence,
        wire::rooms::DesignNoticeEvidence::Conversation { .. }
    ));
    assert_eq!(lanes.noticed().len(), 2);
    seated.release_all();
    lanes.finish();
}

fn retain_conversation(
    profiles: &std::sync::Mutex<crate::resident::message_surface::MessageState>,
    source: CanonicalUuid,
    offset: u64,
    prose: &str,
) {
    let conversation = spice::rooms::evidence::Conversation::read(prose)
        .expect("fixture prose is substantive evidence");
    let mut state =
        crate::resident::message_surface::lock_state(profiles).expect("profile state locks");
    let evidence = spice::rooms::evidence::Evidence::conversation(
        spice::rooms::profile::EvidenceKey::Conversation {
            source,
            member: 0,
            offset,
            length: 1,
        },
        &conversation,
    );
    assert_eq!(
        state
            .profiles
            .insert(test_uuid(FIRST), test_uuid(FIRST), evidence),
        spice::rooms::profile::Inserted::Stored
    );
}

#[test]
fn replaced_conversation_source_releases_delivery_and_selection() {
    let lanes = Lanes::new("design-request-conversation-source-replaced");
    align_completion_lane(&lanes);
    let config = surface(&lanes);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = claimant_seats();
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    let mut diagnostics = Vec::new();
    assert!(
        pending_catch_up(&lanes, &seated, &mut diagnostics).is_none(),
        "nothing is owed before a Design is selected"
    );

    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    lanes.account(FIRST);
    let source = lanes
        .bus
        .read(|read| read.transcript_source_heads(&test_uuid(FIRST)))
        .expect("transcript sources read")
        .into_iter()
        .next()
        .expect("declared lane has a transcript source")
        .into_body()
        .source;
    let prose = "Windows native runner validates archive compression headers libraries toolchain \
                 manifests receipts deployment replay smoke";
    retain_conversation(&profiles, source, 0, prose);
    let first = lanes.design(
        "dVrD245Kd",
        "Validate Windows native runner archives",
        "Compression headers libraries toolchain manifests receipts deployment replay smoke.",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("the current conversation source owes its Design")
            .design,
        first
    );

    let second = lanes.design(
        "dVrD245Km",
        "Validate another Windows native runner",
        "Compression headers libraries toolchain manifests receipts deployment replay smoke.",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("an unrelated write leaves the current request owed")
            .design,
        first
    );
    assert_eq!(
        lanes.noticed().len(),
        1,
        "a match no better than the owed Design leaves it owed"
    );
    let shown = spice::rooms::awareness::request_text(
        &lanes
            .request(&config, FIRST)
            .expect("task next shows the conversation request"),
    );
    let notice = pending_catch_up(&lanes, &seated, &mut diagnostics)
        .expect("room catch-up delivers the request while its source is current");
    assert!(
        notice.text.contains(&shown),
        "catch-up carries the request task next shows: {}",
        notice.text
    );
    for expected in [
        format!(
            "Supporting work: you explained this in conversation: \"{prose}\" (transcript \
             {source} member 0 byte 0 length 1). Read it with `spice session context "
        ),
        format!(
            "--lane {} --vendor omp --stream session --source {source} --member 0 --offset 0`",
            test_uuid(FIRST)
        ),
    ] {
        assert!(shown.contains(&expected), "{expected} is missing: {shown}");
    }

    let replacement = lanes.replace_transcript_source(FIRST);
    assert!(
        lanes.request(&config, FIRST).is_none(),
        "task next drops evidence from the replaced source"
    );
    assert!(
        pending_catch_up(&lanes, &seated, &mut diagnostics).is_none(),
        "room catch-up drops evidence from the replaced source"
    );
    retain_conversation(
        &profiles,
        replacement,
        0,
        "A second Windows native runner validates archive compression headers libraries \
         toolchain manifests receipts deployment replay smoke",
    );
    lanes.reconcile_profiles(&seated, &mut cursor, &profiles);
    assert_eq!(
        lanes
            .request(&config, FIRST)
            .expect("later evidence may select another matching Design")
            .design,
        second
    );
    assert_eq!(lanes.noticed().len(), 2);
    seated.release_all();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    lanes.finish();
}

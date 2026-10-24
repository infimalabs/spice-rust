//! An operator nudge messages the one free lane whose retained session best
//! matches the Design, queueing for a vacant lane when no seated lane is free.

use super::*;

const QUESTION: &str = "Which archive formats must the runner accept?";
const ROOM_POST: &str = "Operator input for the room.";

impl Lanes {
    /// Give `lane` a fresh occupant born on it, with a declared session whose
    /// transcript source the sweep has accounted.
    fn session(&self, lane: u8) -> CanonicalUuid {
        self.vacate(lane);
        self.renew(lane, test_uuid(lane));
        self.declare(lane);
        self.account(lane);
        self.bus
            .read(move |read| read.transcript_source_heads(&test_uuid(lane)))
            .expect("transcript sources read")
            .into_iter()
            .next()
            .expect("declared lane has a transcript source")
            .into_body()
            .source
    }

    /// A Design standing in an exploring working group, asking [`QUESTION`].
    fn open_design(&self, config: &SurfaceConfig) -> wire::TaskHandle {
        let design = self.design_with_questions(
            "dVrD245Kd",
            "Validate Windows native runner archives",
            "Compression headers libraries toolchain manifests receipts deployment replay smoke.",
            &[QUESTION],
        );
        self.group(
            60,
            config,
            &design,
            wire::rooms::WorkingGroupStance::Explore,
        );
        design
    }

    /// The operator's nudge of `design` under command `id`.
    fn nudge(
        &self,
        id: u8,
        config: &SurfaceConfig,
        profiles: &std::sync::Mutex<crate::resident::message_surface::MessageState>,
        design: &wire::TaskHandle,
    ) -> (Outcome<CommandAnswer>, Option<CanonicalUuid>) {
        let task = stamp(design);
        let (statement, workflow) = self
            .bus
            .read(move |read| Ok((read.task_head(&task)?, read.workflow_head(&task)?)))
            .expect("Design heads read");
        let (statement, workflow) = (
            statement.expect("the Design exists"),
            workflow.expect("the Design has a workflow"),
        );
        let request = wire::rooms::DesignNudge {
            design: design.clone(),
        };
        let command = self.command(
            id,
            Actor::Operator,
            CommandKind::DesignNudge,
            vec![
                Witness::Task {
                    task: stamp(design),
                    head: statement.position(),
                },
                Witness::Workflow {
                    task: stamp(design),
                    head: workflow.position(),
                    phase: workflow.body().phase.clone(),
                },
            ],
            &request,
        );
        crate::resident::design_nudge::nudge(&self.bus, config, Some(profiles), &command, &request)
    }

    /// `lane`'s capture of `design` by its seated occupant.
    fn capture(
        &self,
        id: u8,
        config: &SurfaceConfig,
        lane: u8,
        design: &wire::TaskHandle,
    ) -> Outcome<CommandAnswer> {
        let task = stamp(design);
        let (statement, workflow) = self
            .bus
            .read(move |read| Ok((read.task_head(&task)?, read.workflow_head(&task)?)))
            .expect("Design heads read");
        let (statement, workflow) = (
            statement.expect("the Design exists"),
            workflow.expect("the Design has a workflow"),
        );
        let rule: wire::Name = "capture".parse().expect("capture rule");
        let command = self.command(
            id,
            Actor::Lane(test_uuid(lane)),
            CommandKind::ClaimWin,
            vec![
                Witness::Task {
                    task: stamp(design),
                    head: statement.position(),
                },
                Witness::Workflow {
                    task: stamp(design),
                    head: workflow.position(),
                    phase: workflow.body().phase.clone(),
                },
                Witness::Claim {
                    task: stamp(design),
                    head: None,
                },
                Witness::Eligibility { rule: rule.clone() },
                Witness::Residency {
                    lane: test_uuid(lane),
                    occupant: Some(test_uuid(lane)),
                    epoch: self.epoch,
                },
            ],
            &wire::intent::ClaimWin {
                rule,
                recovery: None,
            },
        );
        crate::board_command::dispatch_with_repository(
            &self.bus,
            &command,
            None,
            &config.projects,
            &config.allocation,
            None,
        )
        .expect("capture dispatches")
    }
}

/// Retain one conversation turn of `prose` for `lane`'s session.
fn retain(
    profiles: &std::sync::Mutex<crate::resident::message_surface::MessageState>,
    lane: u8,
    source: CanonicalUuid,
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
            offset: 0,
            length: 1,
        },
        &conversation,
    );
    assert_eq!(
        state
            .profiles
            .insert(test_uuid(lane), test_uuid(lane), evidence),
        spice::rooms::profile::Inserted::Stored
    );
}

fn answer_text(outcome: &Outcome<CommandAnswer>) -> &str {
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(_),
        body: CommandAnswer::Text(answer),
    }) = outcome
    else {
        panic!("the nudge commits a text answer: {outcome:?}");
    };
    &answer.text
}

#[test]
fn a_nudge_messages_the_best_matching_free_seated_lane_whose_capture_then_wins() {
    let lanes = Lanes::new("design-nudge-seated");
    let config = surface(&lanes);
    let first = lanes.session(FIRST);
    let second = lanes.session(SECOND);
    let design = lanes.open_design(&config);
    lanes.claimed("dVrD245Kp", SECOND);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = seats(&[(FIRST, test_uuid(FIRST)), (SECOND, test_uuid(SECOND))]);
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.reconcile_profiles(&config, &seated, &mut cursor, &profiles);
    retain(
        &profiles,
        SECOND,
        second,
        "Windows native runner archives compression headers libraries toolchain manifests \
         receipts deployment replay smoke",
    );
    retain(
        &profiles,
        FIRST,
        first,
        "Windows native runner archives compression headers checked against the database \
         storage locks recovery",
    );
    lanes.post_listeners(70, &config, &design);

    let (outcome, notify) = lanes.nudge(80, &config, &profiles, &design);
    assert_eq!(
        notify,
        Some(test_uuid(FIRST)),
        "a better match holding another claim never takes the nudge"
    );
    let text = answer_text(&outcome);
    assert!(text.contains(&test_uuid(FIRST).to_string()), "{text}");
    assert!(text.contains("match score"), "{text}");
    assert!(lanes.pending(SECOND).is_empty());
    assert!(lanes.pending(SENDER).is_empty());
    let [message] = lanes.pending(FIRST).try_into().expect("one nudge message");
    assert_eq!(message.room, Some(design.clone()));
    for carried in [
        design.to_string().as_str(),
        "Validate Windows native runner archives",
        QUESTION,
        ROOM_POST,
        &format!("spice task capture {design}"),
        "spice preparation propose",
    ] {
        assert!(
            message.text.contains(carried),
            "{carried}: {}",
            message.text
        );
    }

    let captured = lanes.capture(90, &config, FIRST, &design);
    assert!(
        matches!(
            &captured,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                ..
            })
        ),
        "{captured:?}"
    );
    assert_eq!(
        lanes.claim_head(&design).map(|claim| claim.actor),
        Some(Actor::Lane(test_uuid(FIRST)))
    );
    seated.release_all();
    lanes.finish();
}

#[test]
fn with_every_seated_lane_busy_the_nudge_waits_to_wake_the_best_retained_vacant_lane() {
    let lanes = Lanes::new("design-nudge-vacant");
    let config = surface(&lanes);
    lanes.seat(THIRD);
    let third = lanes.session(THIRD);
    let mut cursor = crate::resident::design_awareness::CompletionCursor::start(&lanes.bus)
        .expect("awareness cursor starts");
    let mut seated = seats(&[(THIRD, test_uuid(THIRD))]);
    let profiles = std::sync::Mutex::new(crate::resident::message_surface::MessageState::default());
    lanes.reconcile_profiles(&config, &seated, &mut cursor, &profiles);
    retain(
        &profiles,
        THIRD,
        third,
        "Windows native runner archives compression headers libraries toolchain manifests \
         receipts deployment replay smoke",
    );
    seated.release_all();
    lanes.vacate(THIRD);
    let design = lanes.open_design(&config);
    for (stamp, lane) in [
        ("dVrD245Kp", SENDER),
        ("dVrD245Kq", FIRST),
        ("dVrD245Kr", SECOND),
    ] {
        lanes.claimed(stamp, lane);
    }

    let (outcome, notify) = lanes.nudge(80, &config, &profiles, &design);
    assert_eq!(notify, Some(test_uuid(THIRD)));
    let text = answer_text(&outcome);
    assert!(text.contains(&test_uuid(THIRD).to_string()), "{text}");
    let [queued] = lanes.pending(THIRD).try_into().expect("one queued nudge");
    assert!(queued.wakes, "operator direction wakes the vacant lane");

    let successor = test_uuid(50);
    lanes.renew(THIRD, successor);
    let [delivered] = lanes
        .pending(THIRD)
        .try_into()
        .expect("the first seat reads the nudge");
    assert_eq!(delivered.directive, queued.directive);
    assert!(
        delivered
            .text
            .contains(&format!("spice task capture {design}")),
        "{}",
        delivered.text
    );
    lanes.finish();
}

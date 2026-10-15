use super::*;

#[test]
fn iced_claim_suppresses_work_wakes_but_not_operator_direction_and_grant_thaws_held_work() {
    let digest = digest();
    let (bus, projects, mut projection) = iced_fixture(&digest);
    let snapshot = bus.task_snapshot().expect("iced snapshot reads");
    let iced_target = target(&snapshot, &projection);
    assert!(!iced_target.pending_inbox);
    assert_eq!(iced_target.held_claim, Available::None);
    assert_eq!(iced_target.drain_queue, Available::None);
    assert_eq!(iced_target.available, Available::None);
    assert_eq!(
        resolve(
            &[iced_target],
            &Backoff::default(),
            instant(50),
            lane::WakeOrder::default()
        ),
        Resolution::Idle
    );

    committed(bus.commit(grant_write(&digest)).expect("grant commits"));
    let granted = bus.task_snapshot().expect("granted snapshot reads");
    projection
        .advance(&granted, &projects)
        .expect("grant projection advances");
    assert_eq!(
        resolve(
            &[target(&granted, &projection)],
            &Backoff::default(),
            instant(50),
            lane::WakeOrder::default()
        ),
        Resolution::Attempt {
            trigger: Trigger::HeldClaim,
            lane: lane(),
        }
    );
    bus.shutdown().expect("grant fixture stops");

    let (bus, projects, mut projection) = iced_fixture(&digest);
    committed(
        bus.commit(operator_direction_write())
            .expect("operator direction commits"),
    );
    let directed = bus.task_snapshot().expect("directed snapshot reads");
    projection
        .advance(&directed, &projects)
        .expect("direction projection advances");
    assert_eq!(
        resolve(
            &[target(&directed, &projection)],
            &Backoff::default(),
            instant(50),
            lane::WakeOrder::default()
        ),
        Resolution::Attempt {
            trigger: Trigger::PendingInbox,
            lane: lane(),
        },
        "message-driven wake remains available while work-driven wake is iced"
    );
    bus.shutdown().expect("direction fixture stops");
}

#[test]
fn held_recovery_preserves_the_ready_deadline_across_projection_restart() {
    let digest = digest();
    let (bus, projects, mut projection) = iced_fixture(&digest);
    committed(bus.commit(grant_write(&digest)).expect("grant commits"));
    assert_recovery_deadline(&bus, &projects, &mut projection, 4);
    bus.shutdown().expect("held recovery fixture stops");
}

#[test]
fn reclaiming_a_released_task_preserves_its_new_ready_origin() {
    let digest = digest();
    let (bus, projects, mut projection) = iced_fixture(&digest);
    committed(bus.commit(grant_write(&digest)).expect("grant commits"));
    let snapshot = bus.task_snapshot().expect("held snapshot reads");
    let heads = &snapshot.tasks()[0];
    let task = heads.statement().body().born.clone();
    let claim = heads.claim().expect("active claim").body().clone();
    let mut release = JournalWrite::new(
        Actor::Lane(lane()),
        format!("task:{}", handle(TASK_MOMENT))
            .parse()
            .expect("task origin"),
        instant(5),
        Producer::CommandDoor,
    );
    let freed = release
        .fact(
            body::ClaimFreed {
                task: task.clone(),
                actor: claim.actor.clone(),
                parked: None,
            },
            FactCause::Root,
        )
        .expect("release fact builds");
    release.claim_head(task, freed, move |position| wire::ClaimHead {
        fact: position,
        disposition: wire::ClaimDisposition::Freed,
        ..claim
    });
    committed(bus.commit(release).expect("release commits"));
    projection
        .advance(&bus.task_snapshot().expect("released snapshot"), &projects)
        .expect("release projects");
    committed(
        bus.commit(iced_claim_write(&bus, &digest, instant(6)))
            .expect("new claim commits"),
    );
    assert_recovery_deadline(&bus, &projects, &mut projection, 8);
    bus.shutdown().expect("reclaimed fixture stops");
}

fn assert_recovery_deadline(
    bus: &Bus,
    projects: &ProjectPolicy,
    projection: &mut Projection,
    deadline: u8,
) {
    let snapshot = bus.task_snapshot().expect("held snapshot reads");
    projection
        .advance(&snapshot, projects)
        .expect("grant projection advances");
    let mut restarted = Projection::new();
    restarted
        .advance(&snapshot, projects)
        .expect("restarted projection rehydrates");

    for projection in [&*projection, &restarted] {
        for second in [deadline - 1, deadline, 50] {
            let now = instant(second);
            let targets = project_targets(
                &snapshot,
                projection,
                now,
                &Holds::new(3),
                &board::NextConfig::default(),
            )
            .expect("held recovery target projects");
            let expected = if second < deadline {
                Resolution::Wait {
                    trigger: Trigger::HeldClaim,
                    lane: lane(),
                    deadline: instant(deadline),
                }
            } else {
                Resolution::Attempt {
                    trigger: Trigger::HeldClaim,
                    lane: lane(),
                }
            };
            assert_eq!(
                resolve(
                    &targets,
                    &Backoff::default(),
                    now,
                    lane::WakeOrder::default()
                ),
                expected
            );
        }
    }
}

fn iced_fixture(digest: &wire::Digest) -> (Bus, ProjectPolicy, Projection) {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    committed(
        bus.commit(iced_claim_write(&bus, digest, instant(2)))
            .expect("iced claim commits"),
    );
    projection
        .advance(
            &bus.task_snapshot().expect("iced snapshot reads"),
            &projects,
        )
        .expect("iced projection advances");
    (bus, projects, projection)
}

pub(super) fn target(snapshot: &bus::TaskSnapshot, projection: &Projection) -> Target {
    let targets = project_targets(
        snapshot,
        projection,
        instant(50),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("ice targets project");
    assert_eq!(targets.len(), 1);
    targets[0]
}

fn iced_claim_write(bus: &Bus, digest: &wire::Digest, at: Instant) -> JournalWrite {
    held_claim_write(bus, at, |statement, _| {
        statement.requirements = wire::EnvironmentRequirements::one(
            wire::EnvironmentRequirement::trust_grant(digest.clone()),
        );
    })
}

/// Claim the seeded task for `lane()` on an indefinite lease after `edit`
/// revises its statement and workflow.
pub(super) fn held_claim_write(
    bus: &Bus,
    at: Instant,
    edit: impl FnOnce(&mut TaskHead, &mut wire::WorkflowHead),
) -> JournalWrite {
    let snapshot = bus.task_snapshot().expect("seed snapshot reads");
    let heads = &snapshot.tasks()[0];
    let task = heads.statement().body().born.clone();
    let mut statement = heads.statement().body().clone();
    let original_workflow = heads.workflow().body().clone();
    let mut workflow = original_workflow.clone();
    edit(&mut statement, &mut workflow);
    let workflow_changed = workflow != original_workflow;
    let phase = workflow.phase.clone();
    let horizon = spice::claim_lease::indefinite_horizon();
    let occupant = uuid(9);
    let mut write = JournalWrite::new(
        Actor::Lane(lane()),
        format!("task:{}", handle(TASK_MOMENT))
            .parse()
            .expect("fixture task origin"),
        at,
        Producer::CommandDoor,
    );
    let edited = write
        .fact(
            body::TaskEdited {
                intent: statement
                    .intent
                    .map(|intent| wire::TaskIntentEdit::Set { intent }),
                evidence: None,
                task: task.clone(),
                revision: 2,
                dependencies: None,
            },
            FactCause::Root,
        )
        .expect("requirement edit builds");
    let won = write
        .fact(
            body::ClaimWon {
                task: task.clone(),
                actor: Actor::Lane(lane()),
                lane: lane(),
                occupant,
                epoch: epoch(),
                cell: wire::ClaimCell {
                    project: PROJECT.to_owned(),
                    phase: phase.clone(),
                },
                until: horizon,
                git: None,
                recovery: None,
                source: body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Pending(edited),
        )
        .expect("claim win builds");
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    if workflow_changed {
        write.workflow_head(task.clone(), edited, move |_| workflow);
    }
    let claim_task = task.clone();
    write.claim_head(claim_task, won, move |position| wire::ClaimHead {
        actor: Actor::Lane(lane()),
        fact: position,
        claimed: position,
        cell: wire::ClaimCell {
            project: PROJECT.to_owned(),
            phase,
        },
        lane: lane(),
        occupant,
        epoch: epoch(),
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
        acknowledged: 0,
    });
    write.lease_head(task, won, move |_| wire::LeaseHead {
        start: at,
        renewal: at,
        expiry: horizon,
        maximum: horizon,
        epoch: epoch(),
    });
    write
}

fn grant_write(digest: &wire::Digest) -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("fixture origin"),
        instant(3),
        Producer::CommandDoor,
    );
    let pended = write
        .fact(
            body::TrustPended {
                surface: "spice".to_owned(),
                digest: digest.clone(),
                worktree: Some("/producer/worktree".to_owned()),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("trust pend builds");
    let granted = write
        .fact(
            body::TrustGranted {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Pending(pended),
        )
        .expect("trust grant builds");
    let digest = digest.clone();
    write.trust_head(granted, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Granted,
        worktree: Some("/producer/worktree".to_owned()),
    });
    write
}

fn digest() -> wire::Digest {
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest")
}

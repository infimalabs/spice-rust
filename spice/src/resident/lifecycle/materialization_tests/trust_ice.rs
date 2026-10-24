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
fn a_turn_end_parks_held_work_on_its_ungranted_configuration_until_the_grant_lands() {
    let digest = digest();
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    assert!(
        !reconcile(&bus, PRODUCER, Some(&digest)),
        "a lane without a claim parks nothing"
    );
    assert_eq!(trust(&bus, &digest), None, "nor pends for it");
    // A finite lease that ends before the wake decision below: only the ice
    // keeps the claim held, so the grant can wake it.
    committed(
        bus.commit(claim_write_until(&bus, instant(3), instant(40), |_, _| {}))
            .expect("claim commits"),
    );
    assert!(
        !reconcile(&bus, PRODUCER, None),
        "a configuration without pre-commit commands parks nothing"
    );

    assert!(reconcile(&bus, PRODUCER, Some(&digest)));
    assert_eq!(
        trust(&bus, &digest),
        Some((wire::TrustState::Pended, Some(PRODUCER.to_owned()))),
        "an unseen digest is pended for the operator"
    );
    assert_eq!(required(&bus), Some(digest.clone()));
    let pends = pended_count(&bus);
    assert!(
        !reconcile(&bus, PRODUCER, Some(&digest)),
        "a repeated turn end records nothing more"
    );
    assert_eq!(pended_count(&bus), pends);
    let parked = bus.task_snapshot().expect("parked snapshot reads");
    let lease = parked.tasks()[0].lease().expect("held lease").body();
    assert!(spice::claim_lease::is_indefinite(lease));
    assert_eq!(lease.start, instant(3));
    projection
        .advance(&parked, &projects)
        .expect("parked projection advances");
    assert_eq!(wake(&parked, &projection), Resolution::Idle);

    committed(bus.commit(grant_write(&digest)).expect("grant commits"));
    let granted = bus.task_snapshot().expect("granted snapshot reads");
    projection
        .advance(&granted, &projects)
        .expect("grant projection advances");
    assert_eq!(wake(&granted, &projection), held_wake());
    assert!(
        !reconcile(&bus, PRODUCER, Some(&digest)),
        "a granted configuration changes nothing"
    );
    bus.verify().expect("the parked authority verifies");
    bus.shutdown().expect("parking fixture stops");
}

#[test]
fn a_turn_end_drops_a_park_its_worktree_no_longer_needs() {
    let (abandoned, granted) = (digest(), other_digest());
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    committed(
        bus.commit(claim_write_until(&bus, instant(3), instant(40), |_, _| {}))
            .expect("claim commits"),
    );
    assert!(reconcile(&bus, PRODUCER, Some(&abandoned)));
    committed(bus.commit(grant_write(&granted)).expect("grant commits"));

    let before = super::reaped_seat::fact_kinds(&bus).len();
    assert!(
        reconcile(&bus, PRODUCER, Some(&granted)),
        "a configuration backed out to a granted digest drops the park"
    );
    assert_eq!(required(&bus), None);
    assert_eq!(
        super::reaped_seat::fact_kinds(&bus)[before..],
        [wire::FactKind::TaskEdited],
        "the drop writes no operator item or steering"
    );
    let dropped = bus.task_snapshot().expect("dropped snapshot reads");
    projection
        .advance(&dropped, &projects)
        .expect("dropped projection advances");
    assert_eq!(wake(&dropped, &projection), held_wake());

    assert!(reconcile(&bus, PRODUCER, Some(&abandoned)));
    assert_eq!(required(&bus), Some(abandoned.clone()));
    assert!(
        reconcile(&bus, PRODUCER, None),
        "a configuration that no longer needs trust drops the park"
    );
    assert_eq!(required(&bus), None);
    bus.verify().expect("the dropped authority verifies");
    bus.shutdown().expect("drop fixture stops");
}

#[test]
fn a_turn_end_replaces_its_worktrees_older_park_but_never_another_worktrees() {
    let (older, newer) = (digest(), other_digest());
    let (bus, _, _) = policy_fixture(wire::intent::TeamLife::Drive, false);
    committed(
        bus.commit(held_claim_write(&bus, instant(3), |_, _| {}))
            .expect("claim commits"),
    );
    assert!(reconcile(&bus, PRODUCER, Some(&older)));
    assert!(reconcile(&bus, PRODUCER, Some(&newer)));
    assert_eq!(required(&bus), Some(newer.clone()));
    bus.shutdown().expect("replacement fixture stops");

    let (bus, _, _) = policy_fixture(wire::intent::TeamLife::Drive, false);
    committed(
        bus.commit(pend_write(&older, "/other/worktree", instant(2)))
            .expect("foreign pend commits"),
    );
    committed(
        bus.commit(iced_claim_write(&bus, &older, instant(3)))
            .expect("foreign-parked claim commits"),
    );
    assert!(!reconcile(&bus, PRODUCER, Some(&newer)));
    assert!(!reconcile(&bus, PRODUCER, None));
    assert_eq!(required(&bus), Some(older));
    assert_eq!(
        trust(&bus, &newer),
        None,
        "nothing is pended while another worktree's park stands"
    );
    bus.verify().expect("the foreign park verifies");
    bus.shutdown().expect("foreign fixture stops");
}

/// Worktree `grant_write` names as the digest's producer.
const PRODUCER: &str = "/producer/worktree";

fn reconcile(bus: &Bus, worktree: &str, need: Option<&wire::Digest>) -> bool {
    let need = || Ok(need.cloned());
    crate::resident::lane_surface::reconcile_trust_park(
        bus,
        epoch(),
        lane(),
        worktree,
        need,
        instant(10),
    )
    .expect("turn-end reconciliation decides")
}

fn trust(bus: &Bus, digest: &wire::Digest) -> Option<(wire::TrustState, Option<String>)> {
    bus.trust_head(digest)
        .expect("trust head reads")
        .map(|head| (head.body().state, head.body().worktree.clone()))
}

fn pended_count(bus: &Bus) -> usize {
    super::reaped_seat::fact_kinds(bus)
        .into_iter()
        .filter(|kind| *kind == wire::FactKind::TrustPended)
        .count()
}

fn wake(snapshot: &bus::TaskSnapshot, projection: &Projection) -> Resolution {
    resolve(
        &[target(snapshot, projection)],
        &Backoff::default(),
        instant(50),
        lane::WakeOrder::default(),
    )
}

fn held_wake() -> Resolution {
    Resolution::Attempt {
        trigger: Trigger::HeldClaim,
        lane: lane(),
    }
}

fn required(bus: &Bus) -> Option<wire::Digest> {
    let snapshot = bus.task_snapshot().expect("requirement snapshot reads");
    snapshot.tasks()[0]
        .statement()
        .body()
        .requirements
        .as_slice()
        .iter()
        .find_map(|requirement| match requirement {
            wire::EnvironmentRequirement::TrustGrant { value } => Some(value.clone()),
            _ => None,
        })
}

fn pend_write(digest: &wire::Digest, worktree: &str, at: Instant) -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("fixture origin"),
        at,
        Producer::CommandDoor,
    );
    let pended = write
        .fact(
            body::TrustPended {
                surface: "spice".to_owned(),
                digest: digest.clone(),
                worktree: Some(worktree.to_owned()),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("trust pend builds");
    let (digest, worktree) = (digest.clone(), worktree.to_owned());
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Pended,
        worktree: Some(worktree),
    });
    write
}

fn other_digest() -> wire::Digest {
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        .parse()
        .expect("fixture digest")
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
                None,
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

pub(super) fn target<'a>(snapshot: &bus::TaskSnapshot, projection: &'a Projection) -> Target<'a> {
    let targets = project_targets(
        snapshot,
        projection,
        instant(50),
        &Holds::new(3),
        &board::NextConfig::default(),
        None,
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
    claim_write_until(bus, at, spice::claim_lease::indefinite_horizon(), edit)
}

/// Claim the seeded task for `lane()` on a lease expiring at `horizon` after
/// `edit` revises its statement and workflow.
fn claim_write_until(
    bus: &Bus,
    at: Instant,
    horizon: Instant,
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

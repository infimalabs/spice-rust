use super::*;

#[test]
fn drive_guidance_preserves_creator_team_on_claim_release() {
    let (root, bus, epoch, seats) = fixture("drive-continuation", 1);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    subscription::configure_subscription(&bus, epoch, &seats[0], wire::intent::LineageLife::Drive);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        lane.clone(),
        "DRIVE-first001",
        "Drive continuation",
        Some("todo"),
    );

    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let held = board(&bus, epoch, lane.clone());
    let held_heads = at(&held, &task);
    let released = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        vec![
            claim_witness(held_heads),
            Witness::Lease {
                task: held_heads.statement.body.born.clone(),
                head: held_heads.lease.as_ref().expect("lease head").position,
            },
            residency(&held, &seats[0]),
        ],
        &TaskRef { task },
    ));
    let CommandAnswer::Claim(released) = released else {
        panic!("claim release did not return a claim answer");
    };
    assert_eq!(released.next.as_deref(), Some("spice task next"));
    clean(&root, bus);
}

#[test]
fn drain_guidance_follows_lane_ownership_across_add_release_and_wake() {
    let (root, bus, epoch, seats) = fixture("drain-continuation", 1);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let lane = Actor::Lane(seats[0].lane);
    let first_stamp = stamp("DRAIN-first001");
    let first = TaskHandle::for_project("board.test", &first_stamp);
    let origin: wire::Origin = "fixture:drain-continuation"
        .parse()
        .expect("fixture origin is valid");
    let add = |title: &str| TaskAdd {
        title: title.to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some(origin.clone()),
        flow: Some("todo".to_owned()),
        depends: Vec::new(),
    };
    let idle_add = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: first_stamp.clone(),
        }],
        &add("Idle lane creates durable work"),
    ));
    let CommandAnswer::Task(idle_add) = idle_add else {
        panic!("task creation did not return a task answer");
    };
    assert_eq!(idle_add.next.as_deref(), Some("spice task next"));

    claim_task(&bus, epoch, &seats[0], &lane, &first);
    let second_stamp = stamp("DRAIN-second01");
    let second = TaskHandle::for_project("board.test", &second_stamp);
    let claimed_add = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: second_stamp.clone(),
        }],
        &add("Claimed lane captures follow-up work"),
    ));
    let CommandAnswer::Task(claimed_add) = claimed_add else {
        panic!("claimed task creation did not return a task answer");
    };
    assert_eq!(claimed_add.next, None);

    let held = board(&bus, epoch, lane.clone());
    let held_heads = at(&held, &first);
    let released = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        vec![
            claim_witness(held_heads),
            Witness::Lease {
                task: held_heads.statement.body.born.clone(),
                head: held_heads.lease.as_ref().expect("lease head").position,
            },
            residency(&held, &seats[0]),
        ],
        &TaskRef {
            task: first.clone(),
        },
    ));
    let CommandAnswer::Claim(released) = released else {
        panic!("claim release did not return a claim answer");
    };
    assert_eq!(released.next.as_deref(), Some("spice task next"));

    let before_defer = board(&bus, epoch, Actor::Operator);
    let second_heads = at(&before_defer, &second);
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    let deferred_answer = assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDefer,
        vec![
            workflow_witness(second_heads),
            deferral_witness(second_heads),
        ],
        &TaskDefer {
            task: second.clone(),
            wait: until,
        },
    ));
    let CommandAnswer::Task(deferred_answer) = deferred_answer else {
        panic!("task deferral did not return a task answer");
    };
    assert_eq!(deferred_answer.next, None);
    let deferred = board(&bus, epoch, lane.clone());
    let woken = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskWake,
        vec![
            workflow_witness(at(&deferred, &second)),
            deferral_witness(at(&deferred, &second)),
        ],
        &TaskWake {
            task: second,
            into: None,
        },
    ));
    let CommandAnswer::Task(woken) = woken else {
        panic!("task wake did not return a task answer");
    };
    assert_eq!(woken.next.as_deref(), Some("spice task next"));
    clean(&root, bus);
}

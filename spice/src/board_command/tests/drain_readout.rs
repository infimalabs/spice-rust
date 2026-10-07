use super::*;

use wire::intent::TeamLife;

// The empty-drain readout counts the parked board with the definitions
// task status uses, so one board holding one row of every parked kind has
// to read the same on both surfaces.

fn parked_board(seats: &[Seat], bus: &Bus, epoch: CanonicalUuid) -> (BoardAnswer, TaskHandle) {
    let held = todo_task(bus, epoch, "DRAIN-held0001", "Held by a peer");
    claim_task(bus, epoch, &seats[1], &Actor::Lane(seats[1].lane), &held);
    let stale = todo_task(bus, epoch, "DRAIN-stale001", "Lapsed peer claim");
    claim_task(bus, epoch, &seats[2], &Actor::Lane(seats[2].lane), &stale);
    let prerequisite = todo_task(bus, epoch, "DRAIN-first001", "Open prerequisite");
    let blocked = todo_task(bus, epoch, "DRAIN-block001", "Blocked on the prerequisite");
    let before = board(bus, epoch, Actor::Operator);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &blocked, &[&prerequisite], None, false),
        &TaskDepends {
            task: blocked.clone(),
            after: vec![prerequisite],
        },
    ));
    let waiting = todo_task(bus, epoch, "DRAIN-wait0001", "Deferred row");
    let before = board(bus, epoch, Actor::Operator);
    let heads = at(&before, &waiting);
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: waiting,
            wait: until,
        },
    ));
    add_oops(
        bus,
        epoch,
        Actor::Operator,
        "DRAIN-oops0001",
        "Parked triage row",
    );
    let mut answer = board(bus, epoch, Actor::Lane(seats[0].lane));
    // The lease has not elapsed in fixture time, so the lapse is planted on
    // the snapshot exactly as the resident would have reported it.
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == stale)
        .expect("lapsed fixture row")
        .claim
        .as_mut()
        .expect("lapsed fixture claim")
        .body
        .disposition = ClaimDisposition::Lapsed;
    (answer, stale)
}

fn steered(mut answer: BoardAnswer, team: CanonicalUuid) -> BoardAnswer {
    for subscription in &mut answer.residency.body.subscriptions {
        if subscription.team == team {
            subscription.policy = TeamLife::Steer;
        }
    }
    answer
}

#[test]
fn empty_drain_readout_counts_the_parked_board_like_task_status() {
    let (root, bus, epoch, seats) = fixture("drain-readout", 3);
    let (answer, _) = parked_board(&seats, &bus, epoch);
    let answer = steered(answer, seats[0].team);
    let entries = actor_entries(&answer).expect("planted rows validate");
    assert!(
        board::select(
            &entries,
            &answer.principal,
            &allocation_subscription(&answer.residency.body, &answer.principal)
                .expect("steered subscription"),
            answer.allocation_band,
            answer.recent_cell.as_ref(),
            &answer.environment(),
        )
        .is_none(),
        "a steered team drains empty by construction",
    );

    assert!(
        existing_claim_reply(&answer)
            .expect("peer-only snapshot validates")
            .is_none(),
        "peer claims do not replace the allocator drain"
    );

    let reply = drain_reply(&answer).expect("drained snapshot renders");
    assert_eq!(reply.position, ReplyPosition::Snapshot(answer.cursor));
    let readout = reply.body;
    assert_eq!(
        readout.board,
        ParkedCounts {
            active: 1,
            stale: 1,
            blocked: 1,
            waiting: 1,
            oops: 1,
        }
    );
    assert_eq!(readout.team, TeamLife::Steer);
    assert_eq!(
        readout.text,
        concat!(
            "no available tasks\n",
            "board: 1 active (held by other lanes), ",
            "1 stale (lapsed claims whose takeover did not land this pass), ",
            "1 blocked (waiting on their dependencies), ",
            "1 waiting (deferred until their scheduled time), ",
            "1 oops (parked on the deferred triage board)\n",
            "team policy: steer (the operator hands out work explicitly; ",
            "the allocator selects nothing on its own)\n",
            "allocator scope: no allocator-selected task is available to this lane. ",
            "Parked rows are expected, not missed work. This result does not decide ",
            "whether the current operator request or turn is complete.\n",
            "operator continuity: continue the current prompt and steering ",
            "independently. Perform immediate directions now; capture durable work ",
            "as a task, then run spice task next again.\n",
            "parked-board boundary: do not hunt the parked board for something to ",
            "unstick or unblock. A quiet board or low pending count is not evidence ",
            "that operator work is complete."
        )
    );

    let status = task_status(&answer).expect("same snapshot renders status");
    for line in ["active 1", "stale 1", "blocked 1", "waiting 1", "oops 1"] {
        assert!(status.lines().any(|seen| seen == line), "{line}: {status}");
    }
    clean(&root, bus);
}

#[test]
fn task_next_returns_the_callers_active_claim_before_allocator_drain() {
    let (root, bus, epoch, seats) = fixture("task-next-existing-claim", 1);
    let held = todo_task(&bus, epoch, "DRAIN-own00001", "Continue owned work");
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &held);
    let mut answer = board(&bus, epoch, Actor::Lane(seats[0].lane));
    answer.actor_route.project = "another.private.route".to_owned();
    answer.actor_route.origin_thread = "another-origin-thread".to_owned();
    answer.project_policy.public_stems.clear();
    assert!(
        actor_rows(
            &answer,
            &actor_entries(&answer).expect("owned claim validates")
        )
        .is_empty(),
        "the claim remains possession even when it is outside allocator routing",
    );

    let reply = existing_claim_reply(&answer)
        .expect("owned claim snapshot validates")
        .expect("owned claim returns a continuation");
    assert_eq!(reply.position, ReplyPosition::Snapshot(answer.cursor));
    let CommandAnswer::Text(text) = reply.body else {
        panic!("owned claim continuation is not text")
    };
    assert_eq!(
        text.text,
        format!(
            "claim {held} phase:todo\n\
             assigned to this lane: Continue owned work\n\
             continuity: task next retained the existing assignment; no claim changed.\n\
             next: spice task done --validation \"...\""
        )
    );
    clean(&root, bus);
}

#[test]
fn empty_drain_readout_omits_zero_categories_and_names_each_policy() {
    let (root, bus, epoch, seats) = fixture("drain-readout-quiet", 1);
    let quiet = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let readout = drain_reply(&quiet).expect("quiet snapshot renders").body;
    assert_eq!(readout.team, TeamLife::Drain);
    let mut lines = readout.text.lines();
    assert_eq!(lines.next(), Some("no available tasks"));
    assert_eq!(
        lines.next(),
        Some("board: nothing else visible to this lane")
    );
    assert_eq!(
        lines.next(),
        Some("team policy: drain (the allocator selects from all assignable work)")
    );
    assert_eq!(lines.count(), 3, "three boundary sentences follow");

    let mut lanes = quiet.clone();
    for subscription in &mut lanes.residency.body.subscriptions {
        subscription.policy = TeamLife::Drive;
    }
    let driven = drain_reply(&lanes).expect("driven snapshot renders").body;
    assert!(
        driven.text.lines().any(|line| {
            line == "team policy: drive (the allocator selects only work created for this team)"
        }),
        "{}",
        driven.text
    );

    let waiting = todo_task(&bus, epoch, "DRAIN-quiet001", "The only parked row");
    let before = board(&bus, epoch, Actor::Operator);
    let heads = at(&before, &waiting);
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: waiting,
            wait: until,
        },
    ));
    let one = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let single = drain_reply(&one).expect("one parked row renders").body;
    assert!(
        single
            .text
            .lines()
            .any(|line| line == "board: 1 waiting (deferred until their scheduled time)"),
        "{}",
        single.text
    );
    assert_eq!(
        serde_json::to_value(&single).expect("readout encodes")["board"],
        serde_json::json!({"active": 0, "stale": 0, "blocked": 0, "waiting": 1, "oops": 0})
    );
    clean(&root, bus);
}

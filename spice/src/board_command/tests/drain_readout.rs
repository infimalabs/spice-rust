use super::*;

use wire::intent::LineageLife;

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

fn steered(mut answer: BoardAnswer, lane: CanonicalUuid) -> BoardAnswer {
    for subscription in &mut answer.residency.body.subscriptions {
        if subscription.lineage_id == lane {
            subscription.policy = LineageLife::Steer;
        }
    }
    answer
}

#[test]
fn empty_drain_readout_counts_the_parked_board_like_task_status() {
    let (root, bus, epoch, seats) = fixture("drain-readout", 3);
    let (answer, _) = parked_board(&seats, &bus, epoch);
    let answer = steered(answer, seats[0].lane);
    let entries = actor_entries(&answer).expect("planted rows validate");
    assert!(
        board::select(
            &entries,
            &answer.principal,
            allocation_subscription(&answer.residency.body, &answer.principal)
                .expect("steered subscription"),
            answer.allocation_band,
            answer.recent_cell.as_ref(),
            &answer.environment(),
        )
        .is_none(),
        "a steered lineage drains empty by construction",
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
    assert_eq!(readout.lineage, LineageLife::Steer);
    assert_eq!(
        readout.text,
        concat!(
            "no available tasks\n",
            "board: 1 active (held by other lanes), ",
            "1 stale (lapsed claims whose takeover did not land this pass), ",
            "1 blocked (waiting on their dependencies), ",
            "1 waiting (deferred until their scheduled time), ",
            "1 oops (parked on the deferred triage board)\n",
            "lineage policy: steer (the operator hands out work explicitly; ",
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
fn empty_drain_readout_omits_zero_categories_and_names_each_policy() {
    let (root, bus, epoch, seats) = fixture("drain-readout-quiet", 1);
    let quiet = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let readout = drain_reply(&quiet).expect("quiet snapshot renders").body;
    assert_eq!(readout.lineage, LineageLife::Drain);
    let mut lines = readout.text.lines();
    assert_eq!(lines.next(), Some("no available tasks"));
    assert_eq!(
        lines.next(),
        Some("board: nothing else visible to this lane")
    );
    assert_eq!(
        lines.next(),
        Some("lineage policy: drain (the allocator selects from all assignable work)")
    );
    assert_eq!(lines.count(), 3, "three boundary sentences follow");

    let mut lanes = quiet.clone();
    for subscription in &mut lanes.residency.body.subscriptions {
        subscription.policy = LineageLife::Drive;
    }
    let driven = drain_reply(&lanes).expect("driven snapshot renders").body;
    assert!(
        driven.text.lines().any(|line| {
            line == "lineage policy: drive (the allocator selects only work created for this lineage)"
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

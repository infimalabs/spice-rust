use super::*;

use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("fixture Git text")
        .trim()
        .to_owned()
}

fn issue_git<B: Serialize>(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, kind, expected, body);
    let projects = ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy");
    dispatch_with_repository(
        bus,
        &command,
        Some(repository),
        &projects,
        &board::NextConfig::default(),
    )
    .expect("fixture command belongs to board")
}

/// Bind the seat's lane to a fresh one-commit repository and return it with its HEAD.
fn bind_lane(
    root: &Path,
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
) -> (PathBuf, Repository, String) {
    let lane = root.join("lane");
    fs::create_dir(&lane).expect("lane directory");
    git(&lane, &["init", "-b", "main"]);
    git(&lane, &["config", "user.name", "Spice Test"]);
    git(&lane, &["config", "user.email", "spice@example.invalid"]);
    fs::write(lane.join("tracked.txt"), "opening\n").expect("opening file");
    git(&lane, &["add", "."]);
    git(&lane, &["commit", "-m", "opening"]);
    let head = git(&lane, &["rev-parse", "HEAD"]);
    let repository = Repository::discover(&lane).expect("lane repository");
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    residency.lanes[0].worktree = repository.worktree_root().to_string_lossy().into_owned();
    residency.topology_revision += 1;
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        current_instant().expect("clock"),
        Producer::ResidentRecognition,
    );
    let moved = write
        .fact(
            body::LaneStarted {
                lane: seat.lane,
                occupant: seat.occupant,
                epoch,
                group: 1,
                fresh_session: None,
            },
            FactCause::Existing(before.residency.position),
        )
        .expect("lane worktree fact");
    write.residency_head(moved, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("bind lane worktree"),
        CommitOutcome::Committed(_)
    ));
    (lane, repository, head)
}

fn claim_next(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> CommandAnswer {
    let available = board(bus, epoch, Actor::Lane(seat.lane));
    assert_committed(issue_git(
        bus,
        repository,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("next rule"),
            recovery: None,
        },
    ))
}

/// Release the held task through `route`, leaving it claimable again.
fn release(
    route: &str,
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) {
    let actor = Actor::Lane(seat.lane);
    let held = board(bus, epoch, actor.clone());
    let heads = at(&held, task);
    let lease = Witness::Lease {
        task: heads.statement.body.born.clone(),
        head: heads.lease.as_ref().expect("lease").position,
    };
    let residency = residency_witness(&held, seat.lane, seat.occupant);
    let outcome = match route {
        "free" => issue_git(
            bus,
            repository,
            epoch,
            actor.clone(),
            CommandKind::ClaimFree,
            vec![claim_witness(heads), lease, residency],
            &TaskRef { task: task.clone() },
        ),
        "defer" => issue_git(
            bus,
            repository,
            epoch,
            actor.clone(),
            CommandKind::TaskDefer,
            vec![
                workflow_witness(heads),
                deferral_witness(heads),
                claim_witness(heads),
                lease,
                residency,
            ],
            &TaskDefer {
                task: task.clone(),
                wait: instant_after(current_instant().expect("clock"), 300).expect("deferral"),
            },
        ),
        "plan" => issue_git(
            bus,
            repository,
            epoch,
            actor.clone(),
            CommandKind::TaskDone,
            progress_witnesses(&held, heads, seat.lane, seat.occupant, false)
                .expect("release witnesses"),
            &TaskDone {
                task: task.clone(),
                validation: "plan contract ready".to_owned(),
                operation: Some(completion_operation()),
            },
        ),
        _ => unreachable!("closed release routes"),
    };
    assert_committed(outcome);
    let released = board(bus, epoch, actor);
    let heads = at(&released, task);
    if route == "defer" {
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskWake,
            vec![workflow_witness(heads), deferral_witness(heads)],
            &TaskWake {
                task: task.clone(),
                into: None,
                claim: false,
            },
        ));
    }
}

#[test]
fn free_self_defer_and_plan_completion_keep_dirty_work_on_the_task() {
    for route in ["free", "defer", "plan"] {
        let (root, bus, epoch, seats) = fixture(route, 1);
        let seat = &seats[0];
        let (lane, repository, head) = bind_lane(&root, &bus, epoch, seat);
        let flow = if route == "plan" { "plan,todo" } else { "todo" };
        let task = add_project_with_acceptance(
            &bus,
            epoch,
            Actor::Operator,
            route,
            "Preserve this task's draft",
            "board.test",
            Some(flow),
            &["draft retained"],
        );
        claim_next(&bus, &repository, epoch, seat, &task);
        fs::write(lane.join("tracked.txt"), "working edit\n").expect("tracked edit");
        fs::write(lane.join("staged.txt"), "staged edit\n").expect("staged file");
        git(&lane, &["add", "staged.txt"]);
        fs::write(lane.join("untracked.bin"), [0, 255, 128]).expect("untracked binary");
        release(route, &bus, &repository, epoch, seat, &task);

        let released = board(&bus, epoch, Actor::Lane(seat.lane));
        let heads = at(&released, &task);
        let parked = heads
            .parked_commits
            .as_ref()
            .expect("release retained snapshot on task");
        let snapshot = parked.snapshot.clone().expect("snapshot coordinate");
        assert_eq!(
            heads.claim.as_ref().expect("claim").body.disposition,
            ClaimDisposition::Freed
        );
        assert_eq!(git(&lane, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&lane, &["status", "--porcelain"]), "");
        assert_eq!(git(&lane, &["for-each-ref", "--contains", &snapshot]), "");
        assert_eq!(heads.workflow.body.phase.as_str(), "todo");

        let CommandAnswer::Claim(answer) = claim_next(&bus, &repository, epoch, seat, &task) else {
            panic!("claim answer");
        };
        assert_eq!(
            answer
                .transplant
                .expect("claim transplant")
                .snapshot
                .as_deref(),
            Some(snapshot.as_str())
        );
        assert_eq!(git(&lane, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            fs::read_to_string(lane.join("tracked.txt")).expect("tracked edit"),
            "working edit\n"
        );
        assert_eq!(
            fs::read_to_string(lane.join("staged.txt")).expect("staged edit"),
            "staged edit\n"
        );
        assert_eq!(
            fs::read(lane.join("untracked.bin")).expect("binary draft"),
            [0, 255, 128]
        );
        assert_eq!(
            git(&lane, &["ls-tree", "--name-only", "HEAD"]),
            "tracked.txt"
        );
        clean(&root, bus);
    }
}

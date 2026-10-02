use super::*;

use crate::test_repository;

fn open(lane: &Path) -> gix::Repository {
    gix::open_opts(lane, gix::open::Options::isolated()).expect("open lane repository")
}

fn head_commit(lane: &Path) -> String {
    open(lane).head_id().expect("lane HEAD").to_string()
}

fn head_parent(lane: &Path) -> String {
    open(lane)
        .head_commit()
        .expect("lane HEAD commit")
        .parent_ids()
        .next()
        .expect("lane HEAD parent")
        .to_string()
}

/// Names at the root of the lane's HEAD tree.
fn head_names(lane: &Path) -> Vec<String> {
    let repository = open(lane);
    let tree = repository
        .head_commit()
        .expect("lane HEAD commit")
        .tree()
        .expect("lane HEAD tree");
    tree.iter()
        .map(|entry| entry.expect("tree entry").filename().to_string())
        .collect()
}

/// Whether any reference's history reaches the commit.
fn referenced(lane: &Path, commit: &str) -> bool {
    let repository = open(lane);
    let target = gix::ObjectId::from_hex(commit.as_bytes()).expect("commit id");
    let tips = repository
        .references()
        .expect("lane references")
        .all()
        .expect("reference iterator")
        .filter_map(|reference| reference.ok()?.into_fully_peeled_id().ok())
        .map(gix::Id::detach)
        .collect::<Vec<_>>();
    repository
        .rev_walk(tips)
        .all()
        .expect("reference history")
        .any(|info| info.expect("history commit").id == target)
}

/// HEAD's tree, the index and the opening worktree file all agree.
fn assert_clean(lane: &Path) {
    let tree = open(lane)
        .head_commit()
        .expect("lane HEAD commit")
        .tree_id()
        .expect("lane HEAD tree")
        .to_string();
    assert_eq!(test_repository::write_tree(lane, None), tree);
    assert_eq!(
        fs::read_to_string(lane.join("tracked.txt")).expect("tracked file"),
        "opening\n"
    );
    assert!(!lane.join("staged.txt").exists());
    assert!(!lane.join("untracked.bin").exists());
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
    dispatch_with_repository(
        bus,
        &command,
        Some(repository),
        &projects(),
        &board::NextConfig::default(),
    )
    .expect("fixture command belongs to board")
}

fn projects() -> ProjectPolicy {
    ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy")
}

/// Record the seat's occupant as exited, leaving the lane vacant for the resident.
fn vacate(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) {
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    residency.lanes[0].occupant = None;
    residency.lanes[0].disposition = "vacant".to_owned();
    residency.topology_revision += 1;
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        current_instant().expect("clock"),
        Producer::ResidentRecognition,
    );
    let exited = write
        .fact(
            body::LaneExited {
                lane: seat.lane,
                occupant: seat.occupant,
                code: 0,
            },
            FactCause::Existing(before.residency.position),
        )
        .expect("lane exit fact");
    write.residency_head(exited, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("vacate lane"),
        CommitOutcome::Committed(_)
    ));
}

fn claim_vacant(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    seat: &Seat,
    occupant: CanonicalUuid,
) -> crate::board_command::AutomaticClaim {
    let snapshot = bus.task_snapshot().expect("task snapshot");
    match claim_automatic(
        bus,
        &snapshot,
        repository,
        epoch,
        seat.lane,
        occupant,
        &projects(),
        &board::NextConfig::default(),
    ) {
        Ok(claim) => claim,
        Err(outcome) => panic!("automatic claim: {outcome:?}"),
    }
}

/// Local commits the task's current claim boundary records.
fn claimed_commits(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> (String, u32) {
    let claimed = board(bus, epoch, Actor::Operator);
    let boundary = &at(&claimed, task)
        .git_context
        .as_ref()
        .expect("claim git context")
        .boundary;
    (boundary.head.clone(), boundary.local_commits)
}

/// Give the lane's main branch a fetchable local bare `origin` at its HEAD.
///
/// The bare repository borrows the lane's objects through alternates, so
/// nothing is copied and no Git process publishes: the result is what
/// `push -u origin HEAD` to a fresh bare remote leaves behind.
fn track_origin(lane: &Path, origin: &Path) {
    gix::ThreadSafeRepository::init_opts(
        origin,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )
    .expect("initialize fixture origin");
    let head = head_commit(lane);
    let alternates = origin.join("objects/info");
    fs::create_dir_all(&alternates).expect("create origin alternates parent");
    fs::write(
        alternates.join("alternates"),
        format!("{}\n", lane.join(".git/objects").display()),
    )
    .expect("share lane objects with origin");
    fs::write(origin.join("HEAD"), "ref: refs/heads/main\n").expect("select origin main");
    for reference in [
        origin.join("refs/heads/main"),
        lane.join(".git/refs/remotes/origin/main"),
    ] {
        fs::create_dir_all(reference.parent().expect("reference parent"))
            .expect("create reference parent");
        fs::write(reference, format!("{head}\n")).expect("write fixture reference");
    }
    let mut config = fs::read_to_string(lane.join(".git/config")).expect("read lane config");
    config.push_str(&format!(
        "[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n\
         [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
        origin.display()
    ));
    fs::write(lane.join(".git/config"), config).expect("configure origin tracking");
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
    test_repository::initialize(&lane);
    fs::write(lane.join("tracked.txt"), "opening\n").expect("opening file");
    test_repository::stage_all(&lane);
    let head = test_repository::commit(&lane, "opening");
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
        test_repository::stage(&lane, &["staged.txt"]);
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
        assert_eq!(head_commit(&lane), head);
        assert_clean(&lane);
        assert!(referenced(&lane, &head), "the walk reaches the branch head");
        assert!(
            !referenced(&lane, &snapshot),
            "the snapshot stays unreferenced"
        );
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
        assert_eq!(head_commit(&lane), head);
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
        assert_eq!(head_names(&lane), ["tracked.txt"]);
        clean(&root, bus);
    }
}

/// A resident-minted claim inherits released commits like an allocator claim,
/// and a launch that never starts parks them again rather than stranding them
/// on the lane, so each claim carries the work exactly once.
#[test]
fn automatic_claims_replay_released_commits_once_across_a_failed_launch() {
    let (root, bus, epoch, seats) = fixture("automatic-replay", 1);
    let seat = &seats[0];
    let (lane, repository, opening) = bind_lane(&root, &bus, epoch, seat);
    track_origin(&lane, &root.join("origin.git"));
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "automatic",
        "Keep this task's committed work",
        "board.test",
        Some("todo"),
        &["work retained"],
    );
    claim_next(&bus, &repository, epoch, seat, &task);
    fs::write(lane.join("work.txt"), "committed work\n").expect("work file");
    test_repository::stage_all(&lane);
    test_repository::commit(&lane, "task work");
    release("free", &bus, &repository, epoch, seat, &task);
    assert_eq!(head_commit(&lane), opening);
    vacate(&bus, epoch, seat);

    let launch = claim_vacant(&bus, &repository, epoch, seat, uuid(9_001));
    let replayed = head_commit(&lane);
    assert_eq!(head_parent(&lane), opening);
    assert_eq!(head_names(&lane), ["tracked.txt", "work.txt"]);
    assert_eq!(claimed_commits(&bus, epoch, &task), (replayed.clone(), 1));

    release_automatic(&bus, &repository, &launch).expect("unstarted launch releases");
    assert_eq!(head_commit(&lane), opening);
    assert_clean(&lane);
    let released = board(&bus, epoch, Actor::Operator);
    let parked = at(&released, &task)
        .parked_commits
        .as_ref()
        .expect("failed launch parks the replayed work");
    assert_eq!(parked.commits, [replayed]);

    claim_vacant(&bus, &repository, epoch, seat, uuid(9_002));
    assert_eq!(head_parent(&lane), opening);
    assert_eq!(head_names(&lane), ["tracked.txt", "work.txt"]);
    assert_eq!(claimed_commits(&bus, epoch, &task), (head_commit(&lane), 1));
    clean(&root, bus);
}

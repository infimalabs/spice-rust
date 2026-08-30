use super::*;

use std::process::Command as ProcessCommand;

struct StaleFixture {
    root: PathBuf,
    bus: Bus,
    epoch: CanonicalUuid,
    seat: Seat,
    occupant: CanonicalUuid,
    repository: Repository,
    task: TaskHandle,
    available: TaskHandle,
    implementation: String,
    claimed: Seq,
}

#[test]
fn commit_bearing_stale_claim_reclaims_and_completes_without_expiry() {
    let fixture = stale_fixture();
    assert_recovery_directions(&fixture);
    let before = current_task(&fixture);
    reclaim_stale(&fixture, &before);
    let rebound = assert_rebound(&fixture, &before);
    assert_eq!(
        git_output(&fixture.repository, &["rev-parse", "HEAD"]),
        fixture.implementation
    );
    complete_rebound(&fixture, &rebound);
    clean(&fixture.root, fixture.bus);
}

#[test]
fn stale_claim_does_not_cross_an_unrelated_occupant() {
    let fixture = stale_fixture_with_lineage(false);
    let before = current_task(&fixture);
    let before_horizon = fixture
        .bus
        .fact_snapshot()
        .expect("facts read before refused rebound")
        .horizon();
    let outcome = issue_repository(
        &fixture.bus,
        &reclaim_command(&fixture, &before),
        &fixture.repository,
    );
    assert_direction(outcome, "claim::residency-moved", &fixture.task);
    assert_eq!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read after refused rebound")
            .horizon(),
        before_horizon
    );
    assert_eq!(current_task(&fixture).claim, before.claim);
    clean(&fixture.root, fixture.bus);
}

fn stale_fixture() -> StaleFixture {
    stale_fixture_with_lineage(true)
}

fn stale_fixture_with_lineage(preserve_ancestor: bool) -> StaleFixture {
    let (root, bus, old_epoch, seats) = fixture("stale-reclaim", 1);
    let repository = repository_fixture(&root);
    point_seat_at(&bus, old_epoch, &seats[0], repository.worktree_root());
    let actor = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        old_epoch,
        Actor::Operator,
        "RECLAIM-stale1",
        "Recover committed stale authority",
        Some("todo"),
    );
    let available = add(
        &bus,
        old_epoch,
        Actor::Operator,
        "RECLAIM-stale2",
        "Remain available during stale recovery",
        Some("todo"),
    );
    claim_with_repository(&bus, old_epoch, &seats[0], &actor, &task, &repository);
    fs::write(
        repository.worktree_root().join("implementation.txt"),
        "landed work\n",
    )
    .expect("implementation writes");
    git(&repository, &["add", "implementation.txt"]);
    git(
        &repository,
        &["commit", "-m", "implement stale claim recovery fixture"],
    );
    let implementation = git_output(&repository, &["rev-parse", "HEAD"]);
    let held = board(&bus, old_epoch, actor);
    let claimed = at(&held, &task)
        .claim
        .as_ref()
        .expect("fixture claim exists")
        .body
        .claimed;
    let epoch = uuid(980_001);
    let occupant = uuid(980_002);
    move_residency(&bus, old_epoch, epoch, occupant, preserve_ancestor);
    StaleFixture {
        root,
        bus,
        epoch,
        seat: seats[0].clone(),
        occupant,
        repository,
        task,
        available,
        implementation,
        claimed,
    }
}

fn repository_fixture(root: &Path) -> Repository {
    let remote = root.join("upstream.git");
    let lane = root.join("lane");
    git_at(
        root,
        &["init", "--bare", remote.to_str().expect("remote path")],
    );
    git_at(
        root,
        &["init", "-b", "main", lane.to_str().expect("lane path")],
    );
    git_at(&lane, &["config", "user.email", "fixture@example.invalid"]);
    git_at(&lane, &["config", "user.name", "Fixture"]);
    fs::write(lane.join("README.md"), "base\n").expect("base file writes");
    git_at(&lane, &["add", "README.md"]);
    git_at(&lane, &["commit", "-m", "base"]);
    git_at(
        &lane,
        &[
            "remote",
            "add",
            "origin",
            remote.to_str().expect("remote path"),
        ],
    );
    git_at(&lane, &["push", "-u", "origin", "main"]);
    Repository::discover(lane).expect("lane repository resolves")
}

fn point_seat_at(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, worktree: &Path) {
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    residency.lanes[0].worktree = worktree.display().to_string();
    let mut write = resident_write(epoch);
    let started = write
        .fact(
            body::LaneStarted {
                lane: seat.lane,
                occupant: seat.occupant,
                epoch,
                group: 0,
            },
            FactCause::Existing(before.residency.position),
        )
        .expect("lane worktree fact builds");
    write.residency_head(started, move |_| residency);
    assert!(matches!(bus.commit(write), Ok(CommitOutcome::Committed(_))));
}

fn move_residency(
    bus: &Bus,
    old_epoch: CanonicalUuid,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
    preserve_ancestor: bool,
) {
    let before = board(bus, old_epoch, Actor::Operator);
    let mut residency = before.residency.body;
    residency.epoch = epoch;
    let row = &mut residency.lanes[0];
    row.occupant = Some(occupant);
    if preserve_ancestor {
        row.lineage.push(occupant);
    } else {
        row.lineage = vec![occupant];
    }
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
        Producer::CommandDoor,
    );
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("successor resident fact builds");
    write.residency_head(seated, move |_| residency);
    assert!(matches!(bus.commit(write), Ok(CommitOutcome::Committed(_))));
}

fn resident_write(epoch: CanonicalUuid) -> JournalWrite {
    JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
        Producer::ResidentRecognition,
    )
}

fn claim_with_repository(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    actor: &Actor,
    task: &TaskHandle,
    repository: &Repository,
) {
    let available = board(bus, epoch, actor.clone());
    let command = fixture_command(
        epoch,
        actor.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    );
    assert_committed(issue_repository(bus, &command, repository));
}

fn assert_recovery_directions(fixture: &StaleFixture) {
    let current = board(&fixture.bus, fixture.epoch, Actor::Lane(fixture.seat.lane));
    let held = at(&current, &fixture.task);
    let done = TaskDone {
        task: fixture.task.clone(),
        validation: "stale completion direction".to_owned(),
        operation: Some(TaskDone::operation_for_claim(fixture.claimed)),
    };
    let command = fixture_command(
        fixture.epoch,
        Actor::Lane(fixture.seat.lane),
        CommandKind::TaskDone,
        progress_witnesses(&current, held, fixture.seat.lane, fixture.occupant, false)
            .expect("stale completion witnesses"),
        &done,
    );
    assert_direction(
        issue_repository(&fixture.bus, &command, &fixture.repository),
        "claim::epoch-moved",
        &fixture.task,
    );
    assert_capture_direction(fixture, &current);
    let recover = match recovery_preflight(&current, &fixture.available) {
        Err(outcome) => outcome,
        Ok(_) => panic!("active stale claim admitted dirty recovery"),
    };
    assert_direction(recover, "claim::active-exists", &fixture.task);
    let free = spice::task_git::prepare_free(&fixture.repository, fixture.task.as_str())
        .expect_err("implementation commit refuses free");
    assert_direction(
        task_git_outcome(free),
        "git::free-loose-commits",
        &fixture.task,
    );
}

fn assert_capture_direction(fixture: &StaleFixture, current: &BoardAnswer) {
    let candidate = at(current, &fixture.available);
    let rule: Name = "capture".parse().expect("capture rule");
    let command = fixture_command(
        fixture.epoch,
        Actor::Lane(fixture.seat.lane),
        CommandKind::ClaimWin,
        vec![
            statement_witness(candidate),
            workflow_witness(candidate),
            claim_witness(candidate),
            Witness::Eligibility { rule: rule.clone() },
            residency_witness(current, fixture.seat.lane, fixture.occupant),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    );
    assert_direction(
        issue_repository(&fixture.bus, &command, &fixture.repository),
        "claim::active-exists",
        &fixture.task,
    );
}

fn assert_direction(outcome: Outcome<CommandAnswer>, reason: &str, task: &TaskHandle) {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("expected {reason} refusal, found {outcome:?}");
    };
    assert_eq!(refusal.reason(), reason);
    assert!(
        refusal
            .complete_explanation()
            .contains(&format!("spice task reclaim {task}")),
        "{reason} did not name exact recovery: {refusal:?}"
    );
}

fn current_task(fixture: &StaleFixture) -> BoardTask {
    at(
        &board(&fixture.bus, fixture.epoch, Actor::Lane(fixture.seat.lane)),
        &fixture.task,
    )
    .clone()
}

fn reclaim_stale(fixture: &StaleFixture, before: &BoardTask) {
    let answer = assert_committed(issue_repository(
        &fixture.bus,
        &reclaim_command(fixture, before),
        &fixture.repository,
    ));
    let CommandAnswer::Claim(answer) = answer else {
        panic!("stale reclaim returned a non-claim answer")
    };
    let after = current_task(fixture);
    assert_eq!(
        answer.expiry.as_deref(),
        Some(
            &after
                .lease
                .as_ref()
                .expect("rebound lease exists")
                .body
                .expiry
        )
    );
}

fn reclaim_command(fixture: &StaleFixture, before: &BoardTask) -> wire::AdmittedCommand {
    fixture_command(
        fixture.epoch,
        Actor::Lane(fixture.seat.lane),
        CommandKind::ClaimRenew,
        vec![
            claim_witness(before),
            Witness::Lease {
                task: before.statement.body.born.clone(),
                head: before.lease.as_ref().expect("stale lease").position,
            },
            residency_witness(
                &board(&fixture.bus, fixture.epoch, Actor::Lane(fixture.seat.lane)),
                fixture.seat.lane,
                fixture.occupant,
            ),
        ],
        &ClaimRenew {
            task: fixture.task.clone(),
            lease_seconds: 60 * 60,
        },
    )
}

fn assert_rebound(fixture: &StaleFixture, before: &BoardTask) -> BoardTask {
    let after = current_task(fixture);
    assert_eq!(after.statement, before.statement);
    assert_eq!(after.workflow, before.workflow);
    let before_claim = before.claim.as_ref().expect("stale claim");
    let claim = after.claim.as_ref().expect("rebound claim");
    assert_ne!(claim.position, before_claim.position);
    assert_eq!(claim.body.claimed, before_claim.body.claimed);
    assert_eq!(claim.body.occupant, fixture.occupant);
    assert_eq!(claim.body.epoch, fixture.epoch);
    let lease = after.lease.as_ref().expect("rebound lease");
    assert_eq!(lease.position, claim.position);
    assert_eq!(lease.body.epoch, fixture.epoch);
    assert!(lease.body.expiry >= before.lease.as_ref().expect("stale lease").body.expiry);
    let facts = fixture.bus.fact_snapshot().expect("rebound facts read");
    let rebound = facts
        .facts()
        .iter()
        .find_map(|fact| fact.fact.read::<body::ClaimRebound>())
        .expect("rebound fact exists");
    assert_eq!(rebound.task, after.statement.body.born);
    assert_eq!(rebound.lane, fixture.seat.lane);
    assert_eq!(rebound.occupant, fixture.occupant);
    assert_eq!(rebound.ancestor, before_claim.body.occupant);
    assert_eq!(rebound.epoch, fixture.epoch);
    assert_eq!(rebound.until, lease.body.expiry);
    assert_eq!(rebound.maximum, lease.body.maximum);
    assert_eq!(
        rebound.source,
        fixture
            .bus
            .source_boundary(&fixture.seat.lane)
            .expect("current source boundary reads")
    );
    after
}

fn complete_rebound(fixture: &StaleFixture, rebound: &BoardTask) {
    let current = board(&fixture.bus, fixture.epoch, Actor::Lane(fixture.seat.lane));
    let done = TaskDone {
        task: fixture.task.clone(),
        validation: "rebound claim preserves committed implementation".to_owned(),
        operation: Some(TaskDone::operation_for_claim(fixture.claimed)),
    };
    let command = fixture_command(
        fixture.epoch,
        Actor::Lane(fixture.seat.lane),
        CommandKind::TaskDone,
        progress_witnesses(
            &current,
            rebound,
            fixture.seat.lane,
            fixture.occupant,
            false,
        )
        .expect("rebound completion witnesses"),
        &done,
    );
    let CommandAnswer::Task(answer) = assert_committed(issue_repository(
        &fixture.bus,
        &command,
        &fixture.repository,
    )) else {
        panic!("task completion returned another answer")
    };
    assert_eq!(answer.status, "completed");
    let published = fixture
        .bus
        .fact_snapshot()
        .expect("completion facts read")
        .facts()
        .iter()
        .rev()
        .find_map(|fact| fact.fact.read::<body::TaskDone>())
        .and_then(|done| done.git)
        .expect("completion records Git evidence");
    assert_eq!(published.head, fixture.implementation);
    assert_eq!(published.local_commits, 1);
}

fn issue_repository(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: &Repository,
) -> Outcome<CommandAnswer> {
    dispatch_with_repository(
        bus,
        command,
        Some(repository),
        &guard_projects(),
        &board::NextConfig::default(),
    )
    .expect("fixture command is owned")
}

fn git(repository: &Repository, arguments: &[&str]) {
    git_at(repository.worktree_root(), arguments);
}

fn git_output(repository: &Repository, arguments: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .args(arguments)
        .current_dir(repository.worktree_root())
        .output()
        .expect("git fixture command starts");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
}

fn git_at(worktree: &Path, arguments: &[&str]) {
    let output = ProcessCommand::new("git")
        .args(arguments)
        .current_dir(worktree)
        .output()
        .expect("git fixture command starts");
    assert!(
        output.status.success(),
        "git {arguments:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

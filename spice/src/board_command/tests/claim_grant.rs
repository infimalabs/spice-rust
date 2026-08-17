use super::*;

// `ClaimGrant` documents its resolution order as a contract: each step can
// refuse, the caller sees the first refusal only, and a seat that cannot be read
// is reported before a boundary that cannot be captured. Only `claim_source_
// boundary` needs the lane, so swapping `claim_git_boundary` ahead of
// `claim_residency` compiles and changes which refusal a caller receives while
// every other test stays green. These two tests pin both sides of that order.
//
// The fixture refuses at both steps for unrelated reasons rather than one shared
// one: the witness names the seat beside the actor's own, which only the seat
// read compares, and that seat's worktree is `/fixture`, which only the boundary
// resolves. So the order under test is a choice between two live refusals.

/// The reason carried by a refusal or a fault, whichever the step raised.
///
/// The two steps here raise different `Outcome` variants -- a lane mismatch is a
/// refusal the caller could have avoided, an unresolvable worktree is an
/// environment fault -- so matching on one variant would decide the answer
/// before reading it.
fn grant_reason<T>(result: Result<T, Outcome<CommandAnswer>>) -> String {
    match result {
        Err(Outcome::Refusal(refusal)) => refusal.reason().to_owned(),
        Err(Outcome::Fault(Fault { reason, .. })) => reason,
        Err(Outcome::Reply(_)) => panic!("a refused claim step answered with a reply"),
        Ok(_) => panic!("expected the claim step to refuse, found it resolved"),
    }
}

/// A board, a repository, and a claim naming `seat` as its residency witness.
///
/// The repository is the resident's own, so `task_repository` gets past its
/// witness and lane checks and reaches the seated worktree before refusing.
fn grant_fixture(
    name: &str,
    seat: usize,
) -> (Bus, Repository, ValidatedSnapshot, wire::AdmittedCommand) {
    let (root, bus, epoch, seats) = fixture(name, 2);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);

    fs::create_dir_all(root.join(".git")).expect("claim grant fixture is a repository");
    let resident = Repository::discover(&root).expect("claim grant fixture repository resolves");

    let snapshot = validated_snapshot(
        &bus,
        actor.clone(),
        &guard_projects(),
        &board::NextConfig::default(),
    )
    .expect("claim grant fixture snapshot validates");

    let winner = guard_winner(&snapshot.answer, &actor);
    let expected = claim_expected(&snapshot.answer, &winner, &seats[seat]);
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::ClaimWin,
        expected,
        &ClaimWin {
            rule: "next".parse().expect("claim grant fixture rule"),
            recovery: None,
        },
    );
    (bus, resident, snapshot, command)
}

/// The seat read is reported ahead of the boundary capture, both being live.
///
/// Severing the order -- moving `claim_git_boundary` ahead of `claim_residency`
/// in `claim_grant` -- reds this test alone, because the same inputs then reach
/// the boundary first and the caller receives `git::worktree-unresolved`.
#[test]
fn a_seat_that_cannot_be_read_is_reported_before_a_boundary_that_cannot_be_captured() {
    let (bus, resident, snapshot, command) = grant_fixture("claim-grant-order", 1);

    assert_eq!(
        grant_reason(claim_git_boundary(
            Some(&resident),
            &snapshot,
            &command,
            &snapshot.entries[0],
            Possession::Next,
        )),
        "git::worktree-unresolved",
        "the boundary step must refuse on these inputs, or the order is a choice of one",
    );

    assert_eq!(
        grant_reason(claim_grant(
            &bus,
            &command,
            Some(&resident),
            &snapshot,
            &snapshot.entries[0],
            Possession::Next,
        )),
        "claim::lane-mismatch",
        "the seat read is reported before the boundary that cannot be captured",
    );
}

/// The boundary refusal is reachable through `claim_grant`, not merely shadowed.
///
/// Without this, the test above would still pass if the boundary step were
/// unreachable from `claim_grant` for any reason: a refusal that never fires
/// cannot lose a race. Here the witness names the actor's own seat, so the seat
/// read resolves and the boundary refusal is the one that arrives.
#[test]
fn a_readable_seat_lets_the_boundary_refusal_reach_the_caller() {
    let (bus, resident, snapshot, command) = grant_fixture("claim-grant-boundary", 0);

    assert_eq!(
        grant_reason(claim_grant(
            &bus,
            &command,
            Some(&resident),
            &snapshot,
            &snapshot.entries[0],
            Possession::Next,
        )),
        "git::worktree-unresolved",
        "a seat that reads clean leaves the boundary refusal as the first one",
    );
}

#[test]
fn ordinary_next_capture_and_steal_possessions_remain_dirty_fail_closed() {
    let (root, bus, epoch, seats) = fixture("claim-dirty-possession-regression", 1);
    let lane_root = root.join("lane");
    fs::create_dir(&lane_root).expect("lane worktree exists");
    git(&lane_root, &["init", "-b", "main"]);
    git(&lane_root, &["config", "user.email", "fixture@example.com"]);
    git(&lane_root, &["config", "user.name", "Fixture"]);
    fs::write(lane_root.join("README.md"), "base\n").expect("write lane base");
    git(&lane_root, &["add", "README.md"]);
    git(&lane_root, &["commit", "-m", "base"]);
    let repository = Repository::discover(&lane_root).expect("discover lane repository");
    point_seat_at(&bus, epoch, &seats[0], repository.worktree_root());
    let actor = Actor::Lane(seats[0].lane);
    add(
        &bus,
        epoch,
        Actor::Operator,
        "DIRTY-possession",
        "Keep ordinary claims fail closed",
        Some("todo"),
    );
    let snapshot = validated_snapshot(
        &bus,
        actor.clone(),
        &guard_projects(),
        &board::NextConfig::default(),
    )
    .expect("dirty possession snapshot validates");
    let winner = guard_winner(&snapshot.answer, &actor);
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::ClaimWin,
        claim_expected(&snapshot.answer, &winner, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("ordinary next rule"),
            recovery: None,
        },
    );
    fs::write(lane_root.join("handoff.txt"), "ordinary dirty bytes\n")
        .expect("ordinary dirty handoff writes");

    for (label, possession) in [
        ("next", Possession::Next),
        ("capture", Possession::Capture),
        ("steal", Possession::Steal),
    ] {
        assert_eq!(
            grant_reason(claim_git_boundary(
                Some(&repository),
                &snapshot,
                &command,
                &snapshot.entries[0],
                possession,
            )),
            "git::worktree-dirty",
            "ordinary {label} acquired a dirty lane without explicit recovery",
        );
        assert_eq!(
            fs::read_to_string(lane_root.join("handoff.txt")).expect("dirty bytes remain"),
            "ordinary dirty bytes\n",
        );
    }
    clean(&root, bus);
}

#[test]
fn recovery_claim_uses_the_sealed_synchronized_boundary_before_replay() {
    let (root, bus, epoch, seats) = fixture("claim-recovery-boundary", 1);
    let lane_root = root.join("lane");
    fs::create_dir(&lane_root).expect("lane worktree exists");
    git(&lane_root, &["init", "-b", "main"]);
    git(&lane_root, &["config", "user.email", "fixture@example.com"]);
    git(&lane_root, &["config", "user.name", "Fixture"]);
    fs::write(lane_root.join("README.md"), "base\n").expect("write lane base");
    git(&lane_root, &["add", "README.md"]);
    git(&lane_root, &["commit", "-m", "base"]);
    let repository = Repository::discover(&lane_root).expect("discover lane repository");
    point_seat_at(&bus, epoch, &seats[0], repository.worktree_root());

    let actor = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-boundary",
        "Recover at a sealed boundary",
        Some("todo"),
    );
    let available = board(&bus, epoch, actor.clone());
    fs::write(lane_root.join("handoff.txt"), "handoff\n").expect("write dirty handoff");
    let recovery =
        spice::task_git::quarantine_for_recovery(&repository).expect("quarantine dirty handoff");
    let boundary = spice::task_git::synchronize_recovery(&repository, &recovery)
        .expect("synchronize clean lane");
    let sealed = recovery_preflight(&available, &task)
        .expect("ready task admits recovery")
        .seal(boundary.clone());
    spice::task_git::bind_recovery_claim(&repository, &recovery, sealed)
        .expect("seal recovery claim");

    let heads = at(&available, &task);
    let rule: Name = "recover".parse().expect("recovery rule");
    let transaction = recovery.transaction().to_owned();
    let command = fixture_command(
        epoch,
        actor.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: Some(transaction.clone()),
        },
    );
    assert_committed(claim_win(
        &bus,
        &command,
        match command.body() {
            wire::intent::Intent::ClaimWin(win) => win,
            _ => unreachable!("fixture command is a claim win"),
        },
        Some(&repository),
        &guard_projects(),
        &board::NextConfig::default(),
    ));

    let claimed = board(&bus, epoch, actor);
    let claim = at(&claimed, &task)
        .claim
        .as_ref()
        .expect("recovery claim committed");
    assert_eq!(claim.body.recovery.as_deref(), Some(transaction.as_str()));
    let facts = bus.fact_snapshot().expect("claim facts read");
    let won = facts
        .facts()
        .iter()
        .find(|fact| fact.seq == claim.position)
        .expect("claim head names its fact");
    assert_eq!(
        won.fact.body().get("git"),
        Some(&serde_json::to_value(&boundary).expect("boundary encodes")),
    );
    assert_eq!(
        won.fact
            .body()
            .get("recovery")
            .and_then(|value| value.as_str()),
        Some(transaction.as_str()),
    );

    spice::task_git::replay_recovery(&repository, &recovery).expect("replay under active claim");
    assert_eq!(
        fs::read_to_string(lane_root.join("handoff.txt")).expect("read replayed handoff"),
        "handoff\n"
    );
    assert!(
        spice::task_git::pending_recovery(&repository)
            .expect("inspect retired recovery")
            .is_none()
    );
    clean(&root, bus);
}

fn point_seat_at(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, worktree: &Path) {
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    let row = residency
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane exists");
    row.worktree = worktree.display().to_string();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("fixture resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
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
        .expect("fixture lane movement fact builds");
    write.residency_head(started, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("fixture lane movement commits"),
        CommitOutcome::Committed(_)
    ));
}

fn git(worktree: &Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .args(arguments)
        .current_dir(worktree)
        .output()
        .expect("git fixture command starts");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

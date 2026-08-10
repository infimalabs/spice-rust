use super::*;

use std::process::Command as ProcessCommand;

/// Available-work capacity mints one exact claim for its future occupant and
/// the launch compensation releases only that claim.
#[test]
fn vacant_lane_claims_once_and_exact_launch_cleanup_frees_it() {
    let (root, bus, epoch, seats) = fixture("automatic-lifecycle", 1);
    let seat = &seats[0];
    let lane_root = root.join("lane");
    git(
        &root,
        &["init", "-b", "main", lane_root.to_str().expect("lane path")],
    );
    git(
        &lane_root,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(&lane_root, &["config", "user.name", "Fixture"]);
    fs::write(lane_root.join("README.md"), "fixture\n").expect("fixture file writes");
    git(&lane_root, &["add", "README.md"]);
    git(&lane_root, &["commit", "-m", "fixture"]);
    let repository = Repository::discover(&lane_root).expect("lane repository resolves");

    vacate_into(&bus, epoch, seat, repository.worktree_root());
    let actor = Actor::Operator;
    let first = add(&bus, epoch, actor.clone(), "AUTO-1", "first", None);
    let stale = bus.task_snapshot().expect("pre-burst snapshot reads");
    let second = add(&bus, epoch, actor, "AUTO-2", "second", None);
    let future = uuid(900_001);
    let moved = claim_automatic(
        &bus,
        &stale,
        &repository,
        epoch,
        seat.lane,
        future,
        &guard_projects(),
        &board::NextConfig::default(),
    )
    .expect_err("a moved decision snapshot refuses before claim minting");
    assert!(matches!(moved, Outcome::Refusal(_)));
    assert!(
        bus.task_snapshot()
            .expect("refused snapshot reads")
            .tasks()
            .iter()
            .all(|heads| heads.claim().is_none()),
        "a moved lifecycle decision minted no claim"
    );
    let snapshot = bus
        .task_snapshot()
        .expect("automatic decision snapshot reads");

    let claimed = claim_automatic(
        &bus,
        &snapshot,
        &repository,
        epoch,
        seat.lane,
        future,
        &guard_projects(),
        &board::NextConfig::default(),
    )
    .expect("automatic capacity claims");
    assert!(claimed.task == first || claimed.task == second);
    assert_eq!(claimed.lane, seat.lane);
    assert_eq!(claimed.occupant, future);
    let active = bus
        .task_snapshot()
        .expect("claimed snapshot reads")
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().handle == claimed.task)
        .and_then(journal::TaskHeads::claim)
        .expect("automatic claim head exists")
        .body()
        .clone();
    assert_eq!(active.disposition, ClaimDisposition::Active);
    assert_eq!(active.lane, seat.lane);
    assert_eq!(active.occupant, future);

    release_automatic(&bus, &claimed).expect("exact launch compensation frees claim");
    let freed = bus
        .task_snapshot()
        .expect("freed snapshot reads")
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().handle == claimed.task)
        .and_then(journal::TaskHeads::claim)
        .expect("freed claim head remains")
        .body()
        .disposition;
    assert_eq!(freed, ClaimDisposition::Freed);

    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture removes");
}

fn vacate_into(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, lane_root: &Path) {
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    let row = residency
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane exists");
    row.worktree = lane_root.display().to_string();
    row.occupant = None;
    row.disposition = "vacant".to_owned();
    row.session = Some(wire::LaneSession {
        lane: seat.lane,
        occupant: seat.occupant,
        worktree: lane_root.display().to_string(),
        vendor: row.vendor.clone(),
        launch: None,
        epoch,
        stream: "stdout".to_owned(),
        identity: uuid(900_000).to_string(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    });
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("fixture resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    let drained = write
        .fact(
            wire::body::LaneDrained {
                lane: seat.lane,
                occupant: seat.occupant,
            },
            FactCause::Existing(before.residency.position),
        )
        .expect("fixture drain fact builds");
    write.residency_head(drained, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("fixture vacancy commits"),
        CommitOutcome::Committed(_)
    ));
}

fn git(working: &Path, arguments: &[&str]) {
    let output = ProcessCommand::new("git")
        .args(arguments)
        .current_dir(working)
        .output()
        .expect("fixture git starts");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

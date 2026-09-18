use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use journal::{FactCause, Journal, JournalWrite};
use wire::body::{ClaimWon, SourceBoundary, TaskAdded};
use wire::{
    Actor, ClaimCell, ClaimDisposition, ClaimHead, LeaseHead, Producer, TaskHandle, TaskHead,
    TaskStamp, WorkflowHead,
};

use super::*;

struct SeededClaim {
    handle: TaskHandle,
    task: TaskStamp,
    claimed: wire::Seq,
    expiry: wire::Instant,
    occupant: wire::CanonicalUuid,
}

#[test]
fn installed_wrapper_preserves_replaced_claims_and_reclaims_one_twice() {
    for phase in ["todo", "review"] {
        exercise_claim_replacement(phase);
    }
}

fn exercise_claim_replacement(phase: &str) {
    let (fixture, resident, lane, seeded, installed) = claimed_fixture(phase);
    assert_carried_claim(&fixture, lane, &seeded);
    prime_installed_meter(&fixture, &installed, lane);
    let resident = replace_while_wrapper_waits(&fixture, resident, &installed, lane, phase);
    assert_repeated_status_and_authority(&fixture, &installed, lane, &seeded);
    if phase == "todo" {
        assert_successive_reclaims(&fixture, &installed, lane, &seeded);
    }
    stop(&fixture, resident);
}

fn assert_successive_reclaims(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    seeded: &SeededClaim,
) {
    let before_claim = claim_head(fixture, &seeded.task);
    let before_lease = lease_head(fixture, &seeded.task);
    let (first_started, first_finished) = invoke_reclaim(fixture, installed, lane, seeded);
    let first_lease = lease_head(fixture, &seeded.task);
    assert_one_hour_expiry(&first_lease, first_started, first_finished);
    assert!(first_lease.body().expiry > before_lease.body().expiry);
    assert_ne!(first_lease.position(), before_lease.position());

    std::thread::sleep(Duration::from_millis(5));
    let (second_started, second_finished) = invoke_reclaim(fixture, installed, lane, seeded);
    let second_lease = lease_head(fixture, &seeded.task);
    assert_one_hour_expiry(&second_lease, second_started, second_finished);
    assert!(second_lease.body().expiry > first_lease.body().expiry);
    assert_ne!(second_lease.position(), first_lease.position());
    assert_eq!(second_lease.body().maximum, second_lease.body().expiry);
    assert_eq!(second_lease.body().start, before_lease.body().start);
    assert_eq!(second_lease.body().epoch, before_lease.body().epoch);

    let after_claim = claim_head(fixture, &seeded.task);
    assert_eq!(after_claim.position(), before_claim.position());
    assert_eq!(after_claim.body(), before_claim.body());
    assert_eq!(after_claim.body().actor, Actor::Lane(lane));
    assert_eq!(after_claim.body().claimed, seeded.claimed);
    assert_eq!(after_claim.body().cell.phase.as_str(), "todo");
    assert_eq!(active_claim_count(fixture), 1);
}

fn invoke_reclaim(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    seeded: &SeededClaim,
) -> (jiff::Timestamp, jiff::Timestamp) {
    let started = jiff::Timestamp::now();
    let output = invoke_installed_as_lane(
        fixture,
        installed,
        lane,
        &["task", "reclaim", seeded.handle.as_str()],
    );
    let finished = jiff::Timestamp::now();
    assert!(
        output.status.success(),
        "task reclaim failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(seeded.handle.as_str()),
        "task reclaim answered for another task: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    (started, finished)
}

fn assert_one_hour_expiry(
    lease: &journal::JournalHead<LeaseHead>,
    started: jiff::Timestamp,
    finished: jiff::Timestamp,
) {
    let duration = jiff::SignedDuration::from_hours(1);
    let earliest = started
        .checked_add(duration)
        .expect("earliest expiry is in range");
    let latest = finished
        .checked_add(duration)
        .expect("latest expiry is in range");
    assert!(lease.body().expiry.get() >= earliest);
    assert!(lease.body().expiry.get() <= latest);
}

fn claimed_fixture(
    phase: &str,
) -> (
    Fixture,
    RunningResident,
    wire::CanonicalUuid,
    SeededClaim,
    PathBuf,
) {
    let fixture = Fixture::new();
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    stop(&fixture, initial);

    let lane = if phase == "todo" {
        uuid("00000000-0000-4000-8000-000000000071")
    } else {
        uuid("00000000-0000-4000-8000-000000000081")
    };
    let ancestor = if phase == "todo" {
        uuid("00000000-0000-4000-8000-000000000072")
    } else {
        uuid("00000000-0000-4000-8000-000000000082")
    };
    seed_occupied_lane(
        &fixture,
        lane,
        if phase == "todo" {
            uuid("00000000-0000-4000-8000-000000000073")
        } else {
            uuid("00000000-0000-4000-8000-000000000083")
        },
        ancestor,
    );
    let seeded = seed_claim(&fixture, lane, ancestor, phase);
    let installed = install_current_appliance(&fixture);

    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let first = invoke_installed_as_lane(&fixture, &installed, lane, &["task", "status"]);
    assert_status_claim(&first, &seeded.handle);
    (fixture, resident, lane, seeded, installed)
}

fn assert_carried_claim(fixture: &Fixture, lane: wire::CanonicalUuid, seeded: &SeededClaim) {
    let restored = claim_head(fixture, &seeded.task);
    assert_eq!(restored.body().actor, Actor::Lane(lane));
    assert_eq!(restored.body().claimed, seeded.claimed);
    assert_ne!(restored.body().occupant, seeded.occupant);
    assert_eq!(
        lease_head(fixture, &seeded.task).body().expiry,
        seeded.expiry
    );
}

fn prime_installed_meter(fixture: &Fixture, installed: &Path, lane: wire::CanonicalUuid) {
    let primed = invoke_installed_run(fixture, installed, lane, &["/usr/bin/true"]);
    assert!(
        primed.status.success(),
        "initial installed wrapper failed: stdout={} stderr={}",
        String::from_utf8_lossy(&primed.stdout),
        String::from_utf8_lossy(&primed.stderr),
    );
    assert!(!String::from_utf8_lossy(&primed.stderr).contains("Claim moved"));
}

fn replace_while_wrapper_waits(
    fixture: &Fixture,
    resident: RunningResident,
    installed: &Path,
    lane: wire::CanonicalUuid,
    phase: &str,
) -> RunningResident {
    stop(fixture, resident);
    replace_installed_appliance(installed);
    let ready = fixture.root.join(format!("wrapper-{phase}.ready"));
    let release = fixture.root.join(format!("wrapper-{phase}.release"));
    let mut wrapper = spawn_waiting_wrapper(fixture, installed, lane, &ready, &release);
    match readiness::wait_until_ready(
        &mut wrapper,
        &fixture.runtime,
        readiness::ARRIVAL_HORIZON,
        "installed wrapper child",
        || ready.is_file(),
    ) {
        Readiness::Ready => {}
        Readiness::Exited => panic!("installed wrapper exited before its child rendezvous"),
    }

    let mut resident = RunningResident::spawn(fixture);
    resident.assert_ready(fixture);
    fs::write(&release, b"continue\n").expect("release installed wrapper child");
    let wrapped = wrapper
        .wait_with_output()
        .expect("installed wrapper exits after replacement");
    assert!(
        wrapped.status.success(),
        "replacement wrapper failed: stdout={} stderr={}",
        String::from_utf8_lossy(&wrapped.stdout),
        String::from_utf8_lossy(&wrapped.stderr),
    );
    assert!(
        !String::from_utf8_lossy(&wrapped.stderr).contains("Claim moved"),
        "resident dark interval was misreported as claim loss: {}",
        String::from_utf8_lossy(&wrapped.stderr),
    );
    resident
}

fn assert_repeated_status_and_authority(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    seeded: &SeededClaim,
) {
    let status = invoke_installed_as_lane(fixture, installed, lane, &["task", "status"]);
    assert_status_claim(&status, &seeded.handle);
    let repeated = invoke_installed_as_lane(fixture, installed, lane, &["task", "status"]);
    assert_status_claim(&repeated, &seeded.handle);
    let current = claim_head(fixture, &seeded.task);
    let current_lease = lease_head(fixture, &seeded.task);
    assert_eq!(current.body().actor, Actor::Lane(lane));
    assert_eq!(current.body().lane, lane);
    assert_eq!(current.body().claimed, seeded.claimed);
    assert_eq!(current_lease.body().expiry, seeded.expiry);
    assert_eq!(current_lease.position(), current.position());
    assert_eq!(active_claim_count(fixture), 1);
}

fn seed_claim(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    phase: &str,
) -> SeededClaim {
    let mut journal = Journal::open(fixture.journal()).expect("claim fixture journal opens");
    let residency = journal
        .read(|read| read.residency_head())
        .expect("claim fixture residency reads")
        .expect("claim fixture residency exists");
    let epoch = residency.body().epoch;
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("claim clock is canonical");
    let expiry = wire::Instant::new(
        now.get()
            .checked_add(jiff::SignedDuration::from_hours(1))
            .expect("claim expiry remains in range"),
    )
    .expect("claim expiry is canonical");
    let task = TaskStamp::for_micros(if phase == "todo" { 71 } else { 81 })
        .expect("claim fixture stamp builds");
    let phase: wire::Phase = phase.parse().expect("claim fixture phase parses");
    let actor = Actor::Lane(lane);
    let origin: wire::Origin = format!("task:{task}")
        .parse()
        .expect("claim fixture origin parses");
    seed_task_head(&mut journal, &task, &phase, &actor, &origin, now);
    seed_claim_head(
        &mut journal,
        task,
        phase,
        actor,
        origin,
        lane,
        occupant,
        epoch,
        now,
        expiry,
    )
}

fn seed_task_head(
    journal: &mut Journal,
    task: &TaskStamp,
    phase: &wire::Phase,
    actor: &Actor,
    origin: &wire::Origin,
    now: wire::Instant,
) {
    let mut addition =
        JournalWrite::new(Actor::Operator, origin.clone(), now, Producer::CommandDoor);
    let added = addition
        .fact(
            TaskAdded {
                task: task.clone(),
                title: "Preserve active claim through refresh".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "taste.claim".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("claim fixture task fact builds");
    let statement_task = task.clone();
    let statement_origin = origin.clone();
    addition.task_head(added, move |revision| TaskHead {
        born: statement_task,
        queue_origin: revision,
        title: "Preserve active claim through refresh".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "taste.claim".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: statement_origin,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision,
    });
    let workflow_task = task.clone();
    let workflow_phase = phase.clone();
    let workflow_actor = actor.clone();
    addition.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![workflow_phase.clone()],
        phase: workflow_phase.clone(),
        status: "pending".to_owned(),
        review: if workflow_phase.as_str() == "review" {
            "assigned".to_owned()
        } else {
            "unassigned".to_owned()
        },
        assignment: (workflow_phase.as_str() == "review").then_some(workflow_actor),
        deferral: None,
        completion: None,
    });
    journal.apply(addition).expect("claim fixture task commits");
}

#[allow(
    clippy::too_many_arguments,
    reason = "one fixture claim carries its complete authority boundary"
)]
fn seed_claim_head(
    journal: &mut Journal,
    task: TaskStamp,
    phase: wire::Phase,
    actor: Actor,
    origin: wire::Origin,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    now: wire::Instant,
    expiry: wire::Instant,
) -> SeededClaim {
    let cell = ClaimCell {
        project: "taste.claim".to_owned(),
        phase,
    };
    let mut claim = JournalWrite::new(actor.clone(), origin, now, Producer::CommandDoor);
    let won = claim
        .fact(
            ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(cell.clone()),
                until: expiry,
                git: None,
                recovery: None,
                source: SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("claim fixture claim fact builds");
    let claim_task = task.clone();
    claim.claim_head(claim_task, won, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    let lease_task = task.clone();
    claim.lease_head(lease_task, won, move |_| LeaseHead {
        start: now,
        renewal: now,
        maximum: expiry,
        expiry,
        epoch,
    });
    journal.apply(claim).expect("claim fixture claim commits");
    let claimed = journal
        .read(|read| read.claim_head(&task))
        .expect("seeded claim head reads")
        .expect("seeded claim head exists")
        .body()
        .claimed;
    SeededClaim {
        handle: TaskHandle::for_project("taste.claim", &task),
        task,
        claimed,
        expiry,
        occupant,
    }
}

fn install_current_appliance(fixture: &Fixture) -> PathBuf {
    let installed = fixture.root.join(".git/.spice-install/bin/spice");
    fs::create_dir_all(installed.parent().expect("installed binary parent"))
        .expect("installed binary parent creates");
    fs::copy(env!("CARGO_BIN_EXE_spice"), &installed).expect("current appliance installs");
    installed
}

fn replace_installed_appliance(installed: &Path) {
    let staged = installed.with_extension("replacement");
    fs::copy(env!("CARGO_BIN_EXE_spice"), &staged).expect("replacement appliance stages");
    fs::rename(&staged, installed).expect("replacement appliance publishes atomically");
}

fn invoke_installed_run(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    command: &[&str],
) -> Output {
    installed_command(fixture, installed, lane)
        .args(["run", "--"])
        .args(command)
        .output()
        .expect("installed run launches")
}

fn spawn_waiting_wrapper(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    ready: &Path,
    release: &Path,
) -> Child {
    installed_command(fixture, installed, lane)
        .args(["run", "--", "/bin/sh", "-c"])
        .arg("printf 'ready\\n' > \"$1\"; while [ ! -f \"$2\" ]; do sleep 0.05; done")
        .arg("claim-refresh")
        .arg(ready)
        .arg(release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("installed wrapper launches")
}

fn invoke_installed_as_lane(
    fixture: &Fixture,
    installed: &Path,
    lane: wire::CanonicalUuid,
    arguments: &[&str],
) -> Output {
    installed_command(fixture, installed, lane)
        .arg("--json")
        .args(arguments)
        .output()
        .expect("installed lane command launches")
}

fn installed_command(fixture: &Fixture, installed: &Path, lane: wire::CanonicalUuid) -> Command {
    let mut command = Command::new(installed);
    command
        .current_dir(&fixture.root)
        .env(steer::hook::LANE, lane.to_string())
        .env(vendor::CLAUDE.thread_id_environment(), SESSION_ID)
        .env_remove(vendor::CODEX.thread_id_environment());
    command
}

fn assert_status_claim(output: &Output, expected: &TaskHandle) {
    assert!(
        output.status.success(),
        "task status failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task status answer decodes");
    let text = document["body"]["text"]
        .as_str()
        .expect("task status reply carries text");
    assert!(
        text.starts_with(&format!("claim {expected} ")),
        "task status lost the active claim: {}",
        text,
    );
}

fn claim_head(fixture: &Fixture, task: &TaskStamp) -> journal::JournalHead<wire::ClaimHead> {
    Journal::open(fixture.journal())
        .expect("claim journal opens")
        .read(|read| read.claim_head(task))
        .expect("claim head reads")
        .expect("claim head exists")
}

fn lease_head(fixture: &Fixture, task: &TaskStamp) -> journal::JournalHead<wire::LeaseHead> {
    Journal::open(fixture.journal())
        .expect("lease journal opens")
        .read(|read| read.lease_head(task))
        .expect("lease head reads")
        .expect("lease head exists")
}

fn active_claim_count(fixture: &Fixture) -> usize {
    Journal::open(fixture.journal())
        .expect("claim journal opens")
        .read(|read| {
            let page = read.task_head_page(None, journal::FactReadBudget::RETAINED)?;
            assert!(page.continuation.is_none(), "fixture fits one task page");
            Ok(page.rows)
        })
        .expect("task heads read")
        .into_iter()
        .filter(|heads| {
            heads
                .claim()
                .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active)
        })
        .count()
}

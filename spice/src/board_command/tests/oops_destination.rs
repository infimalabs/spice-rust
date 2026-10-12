use super::*;

const RECOMMENDED: &str = "board.cli";
const STALE: &str = "rim.cli";
const EXPLICIT: &str = "rim.edge";
const DERIVED: &str = "task.oops";

fn policy(public: &[&str]) -> ProjectPolicy {
    ProjectPolicy::new(
        public.iter().map(|stem| (*stem).to_owned()),
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("oops destination test project policy")
}

fn fact_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len()
}

fn file(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    project: Option<&str>,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence { task: stamp(name) }],
        &TaskOopsAdd {
            title: format!("Triage {name}"),
            description: format!("Evidence for {name}"),
            origin: Some("fixture:oops-destination".parse().expect("fixture origin")),
            project: project.map(str::to_owned),
        },
    )
}

/// File one parked row and return its handle with its stored project.
fn parked(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    project: Option<&str>,
) -> (TaskHandle, String) {
    let CommandAnswer::Task(answer) = assert_committed(file(bus, epoch, name, project)) else {
        panic!("oops filing returned another answer shape");
    };
    let stored = at(&board(bus, epoch, Actor::Operator), &answer.task)
        .statement
        .body
        .project
        .clone();
    (answer.task, stored)
}

fn wake(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskHandle,
    into: Option<&str>,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let visible = board(bus, epoch, Actor::Operator);
    let heads = at(&visible, task);
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            workflow_witness(heads),
            deferral_witness(heads),
            statement_witness(heads),
        ],
        &TaskWake {
            task: task.clone(),
            into: into.map(str::to_owned),
            preparation: None,
            claim: false,
        },
    );
    dispatch_with_repository(bus, &command, None, projects, &board::NextConfig::default())
        .expect("board command owns fixture kind")
}

fn promoted_project(bus: &Bus, epoch: CanonicalUuid, outcome: Outcome<CommandAnswer>) -> String {
    let CommandAnswer::Task(answer) = assert_committed(outcome) else {
        panic!("promotion returned another answer shape");
    };
    assert_eq!(answer.status, "promoted");
    let heads = at(&board(bus, epoch, Actor::Operator), &answer.task).clone();
    assert!(!heads.visibility.is_hidden());
    assert_eq!(
        answer.task,
        TaskHandle::for_project(&heads.statement.body.project, &heads.statement.body.born)
    );
    heads.statement.body.project
}

fn assert_refused_without_facts(
    bus: &Bus,
    outcome: Outcome<CommandAnswer>,
    reason: &str,
    facts: usize,
) {
    assert!(
        matches!(&outcome, Outcome::Refusal(refusal) if refusal.reason() == reason),
        "expected {reason}: {outcome:?}",
    );
    assert_eq!(fact_count(bus), facts, "{reason} wrote facts");
}

#[test]
fn bare_wake_lands_in_the_recommendation_or_the_derived_task_project() {
    let (root, bus, epoch, _seats) = fixture("oops-destination-bare", 1);
    let current = policy(&["board", "rim"]);
    let (hinted, hinted_parked) = parked(&bus, epoch, "OOPS-hinted01", Some(RECOMMENDED));
    let (stale, _) = parked(&bus, epoch, "OOPS-stale001", Some(STALE));
    let (missing, missing_parked) = parked(&bus, epoch, "OOPS-missing1", None);
    assert_eq!(hinted_parked, format!(".oops.{RECOMMENDED}"));
    assert_eq!(missing_parked, ".oops");
    assert!(
        hinted.as_str().starts_with("OOPS-"),
        "parked family drifted: {hinted}"
    );

    assert_eq!(
        promoted_project(&bus, epoch, wake(&bus, epoch, &hinted, None, &current)),
        RECOMMENDED
    );
    let narrowed = policy(&["board"]);
    assert_eq!(
        promoted_project(&bus, epoch, wake(&bus, epoch, &stale, None, &narrowed)),
        DERIVED
    );
    assert_eq!(
        promoted_project(&bus, epoch, wake(&bus, epoch, &missing, None, &current)),
        DERIVED
    );
    clean(&root, bus);
}

#[test]
fn explicit_into_wins_and_every_refused_destination_writes_nothing() {
    let (root, bus, epoch, _seats) = fixture("oops-destination-explicit", 1);
    let current = policy(&["board", "rim"]);
    let facts = fact_count(&bus);
    for refused in ["board", "nowhere.cli"] {
        let outcome = file(&bus, epoch, &format!("OOPS-{refused}"), Some(refused));
        assert_refused_without_facts(&bus, outcome, "task::oops-project-refused", facts);
    }

    let (overridden, _) = parked(&bus, epoch, "OOPS-override", Some(RECOMMENDED));
    let (kept, kept_parked) = parked(&bus, epoch, "OOPS-keptpark", Some(RECOMMENDED));
    let facts = fact_count(&bus);
    for refused in [".sandbox.cli", "board", "nowhere.cli"] {
        let outcome = wake(&bus, epoch, &kept, Some(refused), &current);
        assert_refused_without_facts(&bus, outcome, "task::promotion-project-refused", facts);
    }
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &kept)
            .statement
            .body
            .project,
        kept_parked,
    );
    assert_eq!(
        promoted_project(
            &bus,
            epoch,
            wake(&bus, epoch, &overridden, Some(EXPLICIT), &current)
        ),
        EXPLICIT,
    );
    clean(&root, bus);
}

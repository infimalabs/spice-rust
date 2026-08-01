use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::apply::{ApplyError, AuthorityTransaction};
use board::claim::ClaimRefusal;
use board::oops::{Friction, Report, Routed};
use board::phase::PhaseRefusal;
use board::{
    Active, Entry, Preview, Snapshot, Transition, Urgency, View, apply, graph, handout, select,
};
use fold::Cursor;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Digest, Instant, LeaseHead, Origin, Phase,
    Seq, TaskHandle, TaskHead, WorkflowHead,
};

fn handle(text: &str) -> TaskHandle {
    text.parse().expect("task handle")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("phase")
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical UUID")
}

fn instant(text: &str) -> Instant {
    text.parse().expect("canonical instant")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero sequence")
}

fn statement(name: &str) -> TaskHead {
    TaskHead {
        handle: handle(name),
        uuid: uuid("019fbbde-5105-75a3-a76a-722afde1e407"),
        title: format!("Implement {name}"),
        project: "board.kernel".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:BOARD-root".parse::<Origin>().expect("origin"),
        revision: seq(1),
    }
}

fn workflow(current: &str) -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase(current),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn claim(disposition: ClaimDisposition) -> ClaimHead {
    ClaimHead {
        actor: Actor::Spice,
        fact: seq(2),
        lane: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        occupant: uuid("019fbbde-5105-75a3-a76a-722afde1e409"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
        disposition,
    }
}

fn lease() -> LeaseHead {
    LeaseHead {
        start: instant("2026-08-01T06:00:00.000Z"),
        renewal: instant("2026-08-01T06:01:00.000Z"),
        expiry: instant("2026-08-01T06:10:00.000Z"),
        maximum: instant("2026-08-01T06:20:00.000Z"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
    }
}

fn entry(name: &str) -> Entry {
    Entry::new(statement(name), workflow("todo"), None, None).expect("valid heads")
}

fn claimed_entry(name: &str, current: &str) -> Entry {
    Entry::new(
        statement(name),
        workflow(current),
        Some(claim(ClaimDisposition::Active)),
        Some(lease()),
    )
    .expect("valid claimed heads")
}

#[test]
fn statement_views_preserve_project_family_handle_and_dependency_heads() {
    let mut head = statement("BOARD-a1");
    head.dependencies = vec![handle("WIRE-b2"), handle("JOURNAL-c3")];
    let entry = Entry::new(head, workflow("todo"), None, None).expect("valid heads");

    assert_eq!(entry.project().as_str(), "board.kernel");
    assert_eq!(entry.family().as_str(), "BOARD");
    assert!(".oops.workflow".parse::<board::Project>().is_ok());
    assert!(
        "agent.019fbbde510575a3a76a722afde1e407.task"
            .parse::<board::Project>()
            .is_ok()
    );
    assert_eq!(
        entry.dependencies().collect::<Vec<_>>(),
        vec![
            board::Dependency {
                task: handle("BOARD-a1"),
                after: handle("WIRE-b2"),
            },
            board::Dependency {
                task: handle("BOARD-a1"),
                after: handle("JOURNAL-c3"),
            },
        ]
    );

    let mut self_dependent = statement("BOARD-a1");
    self_dependent.dependencies = vec![handle("BOARD-a1")];
    assert!(matches!(
        Entry::new(self_dependent, workflow("todo"), None, None),
        Err(board::TaskError::SelfDependency(_))
    ));

    let mut duplicate = statement("BOARD-a1");
    duplicate.dependencies = vec![handle("WIRE-b2"), handle("WIRE-b2")];
    assert!(matches!(
        Entry::new(duplicate, workflow("todo"), None, None),
        Err(board::TaskError::DuplicateDependency { .. })
    ));

    let mut malformed = workflow("todo");
    malformed.phase = phase("design");
    assert!(matches!(
        Entry::new(statement("BOARD-a1"), malformed, None, None),
        Err(board::TaskError::PhaseOutsideFlow(_))
    ));

    let mut empty = workflow("todo");
    empty.flow.clear();
    assert!(matches!(
        Entry::new(statement("BOARD-a1"), empty, None, None),
        Err(board::TaskError::EmptyFlow)
    ));

    let mut duplicate_phase = workflow("todo");
    duplicate_phase.flow.push(phase("todo"));
    assert!(matches!(
        Entry::new(statement("BOARD-a1"), duplicate_phase, None, None),
        Err(board::TaskError::DuplicatePhase(_))
    ));

    let mut unknown_status = workflow("todo");
    unknown_status.status = "mystery".to_owned();
    assert!(matches!(
        Entry::new(statement("BOARD-a1"), unknown_status, None, None),
        Err(board::TaskError::WorkflowStatus(_))
    ));
}

#[test]
fn snapshot_is_a_schema_and_cursor_bounded_head_answer() {
    let entries = [entry("BOARD-a1")];
    let cursor = Cursor::at(seq(9));
    let snapshot =
        Snapshot::new(journal::SCHEMA_VERSION, cursor, &entries).expect("current schema");

    assert_eq!(snapshot.schema_version(), journal::SCHEMA_VERSION);
    assert_eq!(snapshot.cursor(), cursor);
    assert_eq!(snapshot.entries(), entries.as_slice());
    assert!(Snapshot::new(journal::SCHEMA_VERSION + 1, cursor, &entries).is_err());
}

#[test]
fn active_claim_is_only_admitted_from_matching_coherent_live_heads() {
    let entry = claimed_entry("BOARD-a1", "todo");
    let active = Active::admit(&entry, &Actor::Spice, instant("2026-08-01T06:05:00.000Z"))
        .expect("live lease proves possession");

    assert_eq!(active.handle(), &handle("BOARD-a1"));
    assert_eq!(active.actor(), &Actor::Spice);
    assert_eq!(active.epoch(), lease().epoch);
    assert!(matches!(
        Active::admit(
            &entry,
            &Actor::Operator,
            instant("2026-08-01T06:05:00.000Z")
        ),
        Err(ClaimRefusal::WrongActor { .. })
    ));
    assert!(matches!(
        Active::admit(&entry, &Actor::Spice, instant("2026-08-01T06:10:00.000Z")),
        Err(ClaimRefusal::Expired { .. })
    ));

    let freed = Entry::new(
        statement("BOARD-a1"),
        workflow("todo"),
        Some(claim(ClaimDisposition::Freed)),
        Some(lease()),
    )
    .expect("valid heads");
    assert!(matches!(
        Active::admit(&freed, &Actor::Spice, instant("2026-08-01T06:05:00.000Z")),
        Err(ClaimRefusal::Inactive(ClaimDisposition::Freed))
    ));
}

#[test]
fn workflow_advances_only_one_declared_station_and_completes_only_the_last() {
    let todo = claimed_entry("BOARD-a1", "todo");
    let active = Active::admit(&todo, &Actor::Spice, instant("2026-08-01T06:05:00.000Z"))
        .expect("active claim");
    let advance = Transition::advance(&todo, &active, phase("review")).expect("next station");
    assert_eq!(advance.next().phase, phase("review"));
    assert!(matches!(
        Transition::advance(&todo, &active, phase("todo")),
        Err(PhaseRefusal::Illegal { .. })
    ));
    assert!(matches!(
        Transition::complete(&todo, &active, instant("2026-08-01T06:06:00.000Z")),
        Err(PhaseRefusal::NotFinal(_))
    ));

    let review = claimed_entry("BOARD-a1", "review");
    let active = Active::admit(&review, &Actor::Spice, instant("2026-08-01T06:05:00.000Z"))
        .expect("active claim");
    let complete = Transition::complete(&review, &active, instant("2026-08-01T06:06:00.000Z"))
        .expect("final station completes");
    assert_eq!(complete.next().status, "completed");
    assert_eq!(
        complete.next().completion,
        Some(instant("2026-08-01T06:06:00.000Z"))
    );
}

#[test]
fn urgency_is_a_pure_total_scalar_over_heads() {
    let completed = BTreeSet::new();
    let plain = entry("BOARD-a1");
    let mut important_head = statement("BOARD-b2");
    important_head.tags.push("priority:high".to_owned());
    let important = Entry::new(important_head, workflow("todo"), None, None).expect("valid heads");
    let claimed = claimed_entry("BOARD-c3", "todo");

    assert_eq!(
        Urgency::from_heads(&plain, &completed),
        Urgency::from_heads(&plain, &completed)
    );
    assert!(Urgency::from_heads(&important, &completed) > Urgency::from_heads(&plain, &completed));
    assert!(Urgency::from_heads(&claimed, &completed) < Urgency::from_heads(&plain, &completed));
}

#[test]
fn allocation_is_deterministic_and_has_one_eligibility_path() {
    let first = entry("BOARD-a1");
    let second = entry("BOARD-b2");

    let mut blocked_head = statement("BOARD-z9");
    blocked_head.tags.push("priority:critical".to_owned());
    blocked_head.dependencies.push(handle("WIRE-missing"));
    let blocked =
        Entry::new(blocked_head, workflow("todo"), None, None).expect("valid blocked heads");

    let mut assigned_workflow = workflow("todo");
    assigned_workflow.assignment = Some(Actor::Operator);
    let mut assigned_head = statement("BOARD-y8");
    assigned_head.tags.push("priority:critical".to_owned());
    let assigned =
        Entry::new(assigned_head, assigned_workflow, None, None).expect("valid assigned heads");

    let active = claimed_entry("BOARD-x7", "todo");
    let ordered = [
        blocked.clone(),
        second.clone(),
        assigned.clone(),
        first.clone(),
        active.clone(),
    ];
    let reversed = [active, first, assigned, second, blocked];

    let selected = select(&ordered, &Actor::Spice).expect("eligible task");
    let selected_again = select(&reversed, &Actor::Spice).expect("eligible task");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-a1"));
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle
    );
    assert_eq!(selected_again.urgency(), selected.urgency());
}

#[derive(Debug)]
struct TransactionFailure;

impl fmt::Display for TransactionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("injected transaction failure")
    }
}

impl Error for TransactionFailure {}

#[derive(Default)]
struct MemoryAuthority {
    calls: usize,
    tasks: Vec<String>,
    fail: bool,
}

impl AuthorityTransaction for MemoryAuthority {
    type Error = TransactionFailure;

    fn commit_document(&mut self, document: &dialect::Document) -> Result<(), Self::Error> {
        self.calls += 1;
        if self.fail {
            return Err(TransactionFailure);
        }
        self.tasks = document
            .nodes
            .iter()
            .map(|node| node.slug.clone())
            .collect();
        Ok(())
    }
}

fn digest(byte: char) -> Digest {
    std::iter::repeat_n(byte, 64)
        .collect::<String>()
        .parse()
        .expect("canonical digest")
}

#[test]
fn document_apply_is_digest_bound_and_crosses_one_transaction_seam() {
    let preview = Preview::classify("# Goal\n\n- First task\n- Second task\n", digest('a'))
        .expect("document previews");
    let mut authority = MemoryAuthority::default();

    let stale = apply(&mut authority, &preview, &digest('b')).expect_err("stale digest refuses");
    assert!(matches!(stale, ApplyError::StaleDigest { .. }));
    assert_eq!(authority.calls, 0);
    assert!(authority.tasks.is_empty());

    authority.fail = true;
    let failed = apply(&mut authority, &preview, &digest('a')).expect_err("transaction fails");
    assert!(matches!(failed, ApplyError::Transaction(_)));
    assert_eq!(authority.calls, 1);
    assert!(authority.tasks.is_empty());

    authority.fail = false;
    let receipt = apply(&mut authority, &preview, &digest('a')).expect("transaction commits");
    assert_eq!(authority.calls, 2);
    assert_eq!(receipt.nodes, preview.document().nodes.len());
    assert_eq!(authority.tasks.len(), receipt.nodes);
}

#[test]
fn diagrams_and_handout_are_stable_across_input_order() {
    let prerequisite = entry("BOARD-a1");
    let mut dependent_head = statement("BOARD-b2");
    dependent_head.dependencies.push(handle("BOARD-a1"));
    let dependent = Entry::new(dependent_head, workflow("todo"), None, None).expect("valid heads");
    let first = [dependent.clone(), prerequisite.clone()];
    let second = [prerequisite, dependent];

    for view in View::ALL {
        assert_eq!(graph::render(view, &first), graph::render(view, &second));
    }
    assert_eq!(handout::compose(&first), handout::compose(&second));
    assert!(handout::compose(&first).contains("BOARD-b2 -> BOARD-a1"));
}

#[test]
fn oops_routes_friction_to_work_and_observations_to_notes_only() {
    let friction = Friction::new(
        "Cargo metadata is blocked",
        "A duplicate table refuses every command",
        "tooling.manifest",
    )
    .expect("valid project");
    let Routed::Wakeable(task) = board::oops::route(Report::Friction(friction)) else {
        panic!("friction becomes wakeable work")
    };
    assert_eq!(task.project.as_str(), "tooling.manifest");

    assert_eq!(
        board::oops::route(Report::Observation("saw one retry".to_owned())),
        Routed::Note("saw one retry".to_owned())
    );
}

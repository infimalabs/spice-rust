use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::apply::{ApplyError, AuthorityTransaction};
use board::claim::ClaimRefusal;
use board::oops::{Friction, Report, Routed};
use board::phase::PhaseRefusal;
use board::{
    Active, Entry, Preview, Snapshot, SnapshotReadError, Transition, Urgency, View, apply,
    digest_of, graph, handout, select,
};
use fold::Cursor;
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Digest, Domain, Instant, LeaseHead, Origin,
    Phase, Producer, Seq, TaskHandle, TaskHead, WorkflowHead, body,
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

fn all_projects() -> wire::LineageSubscription {
    wire::LineageSubscription {
        lineage_id: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        policy: wire::intent::LineageLife::Drain,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
    }
}

fn projects() -> board::ProjectPolicy {
    board::ProjectPolicy::new(
        ["board".to_owned(), "agent".to_owned()],
        Vec::new(),
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy")
}

fn admitted(
    statement: TaskHead,
    workflow: WorkflowHead,
    claim: Option<ClaimHead>,
    lease: Option<LeaseHead>,
) -> Result<Entry, board::TaskError> {
    Entry::new(statement, workflow, claim, lease, &projects())
}

fn statement(name: &str) -> TaskHead {
    TaskHead {
        handle: handle(name),
        uuid: uuid("019fbbde-5105-75a3-a76a-722afde1e407"),
        title: format!("Implement {name}"),
        description: None,
        acceptance: Vec::new(),
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
    admitted(statement(name), workflow("todo"), None, None).expect("valid heads")
}

fn claimed_entry(name: &str, current: &str) -> Entry {
    admitted(
        statement(name),
        workflow(current),
        Some(claim(ClaimDisposition::Active)),
        Some(lease()),
    )
    .expect("valid claimed heads")
}

fn completed_entry(name: &str) -> Entry {
    let mut completed = workflow("review");
    completed.status = "completed".to_owned();
    completed.completion = Some(instant("2026-08-01T06:06:00.000Z"));
    admitted(
        statement(name),
        completed,
        Some(claim(ClaimDisposition::Active)),
        Some(lease()),
    )
    .expect("valid completed heads")
}

fn journal_with_heads(mut statement: TaskHead, workflow: WorkflowHead) -> Journal {
    let mut journal = Journal::in_memory().expect("board journal fixture");
    let task = statement.handle.clone();
    let mut write = JournalWrite::new(
        Actor::Spice,
        statement.provenance.clone(),
        instant("2026-08-01T06:00:00.000Z"),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                uuid: statement.uuid,
                title: statement.title.clone(),
                description: statement.description.clone(),
                acceptance: statement.acceptance.clone(),
                project: statement.project.clone(),
                tags: statement.tags.clone(),
                depends: statement.dependencies.clone(),
                flow: workflow.flow.clone(),
                phase: workflow.phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("registered board fixture fact");
    write.task_head(fact, move |position| {
        statement.revision = position;
        statement
    });
    write.workflow_head(task, fact, move |_| workflow);
    journal.apply(write).expect("commit board fixture heads");
    journal
}

#[test]
fn statement_views_preserve_project_family_handle_and_dependency_heads() {
    let mut head = statement("BOARD-a1");
    head.dependencies = vec![handle("WIRE-b2"), handle("JOURNAL-c3")];
    let entry = admitted(head, workflow("todo"), None, None).expect("valid heads");

    assert_eq!(entry.project().as_str(), "board.kernel");
    assert_eq!(entry.family().as_str(), "BOARD");
    let projects = board::ProjectPolicy::new(
        ["board".to_owned(), "agent".to_owned()],
        Vec::new(),
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy");
    assert!(projects.admit(".oops.workflow").is_ok());
    assert!(
        projects
            .admit("agent.019fbbde510575a3a76a722afde1e407.task")
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
        admitted(self_dependent, workflow("todo"), None, None),
        Err(board::TaskError::SelfDependency(_))
    ));

    let mut duplicate = statement("BOARD-a1");
    duplicate.dependencies = vec![handle("WIRE-b2"), handle("WIRE-b2")];
    assert!(matches!(
        admitted(duplicate, workflow("todo"), None, None),
        Err(board::TaskError::DuplicateDependency { .. })
    ));

    let mut malformed = workflow("todo");
    malformed.phase = phase("design");
    assert!(matches!(
        admitted(statement("BOARD-a1"), malformed, None, None),
        Err(board::TaskError::PhaseOutsideFlow(_))
    ));

    let mut empty = workflow("todo");
    empty.flow.clear();
    assert!(matches!(
        admitted(statement("BOARD-a1"), empty, None, None),
        Err(board::TaskError::EmptyFlow)
    ));

    let mut duplicate_phase = workflow("todo");
    duplicate_phase.flow.push(phase("todo"));
    assert!(matches!(
        admitted(statement("BOARD-a1"), duplicate_phase, None, None),
        Err(board::TaskError::DuplicatePhase(_))
    ));

    let mut unknown_status = workflow("todo");
    unknown_status.status = "mystery".to_owned();
    assert!(matches!(
        admitted(statement("BOARD-a1"), unknown_status, None, None),
        Err(board::TaskError::WorkflowStatus(_))
    ));
}

#[test]
fn project_policy_admits_only_resolved_hidden_roots_without_broadening_public_law() {
    let projects = board::ProjectPolicy::new(
        ["board".to_owned()],
        [
            "sandbox".to_owned(),
            "oops".to_owned(),
            "sandbox".to_owned(),
        ],
        2,
        3,
    )
    .expect("hidden additions resolve");
    assert_eq!(
        projects.hidden_stems(),
        ["oops", "maxim_proposal", "sandbox"]
    );

    for (project, stem) in [
        (".oops", "oops"),
        (".oops.correctness", "oops"),
        (".maxim_proposal", "maxim_proposal"),
        (".maxim_proposal.theme_2", "maxim_proposal"),
        (".sandbox.deep.audit_1", "sandbox"),
    ] {
        let admitted = projects.admit(project).expect("resolved hidden project");
        assert_eq!(admitted.as_str(), project);
        assert_eq!(admitted.visibility().hidden_stem(), Some(stem));
        assert!(matches!(
            projects.admit_manual_creation(project),
            Err(board::ProjectError::SystemReserved(ref reserved)) if reserved == stem
        ));
    }
    for (route, expected) in [
        (board::SystemTaskRoute::oops(), ".oops"),
        (
            board::SystemTaskRoute::proposal(["theme_2".to_owned()]),
            ".maxim_proposal.theme_2",
        ),
        (
            board::SystemTaskRoute::configured(
                "sandbox",
                ["deep".to_owned(), "audit_1".to_owned()],
            ),
            ".sandbox.deep.audit_1",
        ),
    ] {
        assert_eq!(
            projects
                .admit_system_creation(&route)
                .expect("server route derives one resolved hidden project")
                .as_str(),
            expected,
        );
    }
    for stem in ["oops", "maxim_proposal"] {
        assert!(matches!(
            projects.admit_system_creation(&board::SystemTaskRoute::configured(
                stem,
                ["cross_target".to_owned()],
            )),
            Err(board::ProjectError::SystemRouteCollision(ref owner)) if owner == stem
        ));
    }
    assert!(matches!(
        projects.admit(".unknown.work"),
        Err(board::ProjectError::UnknownHiddenStem(stem)) if stem == "unknown"
    ));
    for malformed in [".", ".oops.", ".oops..work", ".oops.bad-stem", ".oops.Bad"] {
        assert!(matches!(
            projects.admit(malformed),
            Err(board::ProjectError::HiddenSpelling(_))
        ));
    }

    for public in ["board.work", "board.deep_work.unit2"] {
        let admitted = projects.admit(public).expect("current public project");
        assert!(!admitted.visibility().is_hidden());
        assert_eq!(
            projects
                .admit_manual_creation(public)
                .expect("manual creation keeps public admission"),
            admitted,
        );
    }
    for refused in ["board", "board.one.two.three", "board.bad-stem"] {
        assert!(projects.admit(refused).is_err(), "{refused}");
    }
    assert!(matches!(
        board::ProjectPolicy::new(["board".to_owned()], ["board".to_owned()], 2, 3),
        Err(board::ProjectPolicyError::HiddenPublicCollision {
            index: Some(0),
            stem,
        }) if stem == "board"
    ));
}

#[test]
fn allocator_and_inventory_share_the_stored_visibility_for_every_actor_route() {
    let projects = board::ProjectPolicy::new(["board".to_owned()], ["sandbox".to_owned()], 2, 3)
        .expect("fixture policy");
    let admitted = |name: &str, project: &str, critical: bool| {
        let mut head = statement(name);
        head.project = project.to_owned();
        if critical {
            head.tags.push("priority:critical".to_owned());
        }
        Entry::new(head, workflow("todo"), None, None, &projects).expect("fixture entry")
    };
    let public = admitted("BOARD-public", "board.visibility", false);
    let hidden_oops = admitted("BOARD-oops", ".oops.correctness", true);
    let hidden_maxim = admitted("BOARD-maxim", ".maxim_proposal.theme_2", true);
    let hidden_sandbox = admitted("BOARD-sandbox", ".sandbox.audit", true);
    let entries = vec![hidden_sandbox, hidden_oops, public, hidden_maxim];

    let lane = uuid("019fbbde-5105-75a3-a76a-722afde1e408");
    for actor in [Actor::Spice, Actor::Operator, Actor::Lane(lane)] {
        assert_eq!(
            select(&entries, &actor, &all_projects(), 2)
                .expect("public task remains eligible")
                .entry()
                .statement()
                .handle,
            handle("BOARD-public")
        );
        for hidden in [&entries[0], &entries[1], &entries[3]] {
            assert!(!board::eligible_entry(&entries, hidden, &actor));
        }
    }

    let inventory = board::Inventory::new(&entries);
    assert_eq!(inventory.public().len(), 1);
    assert_eq!(
        inventory.public()[0].statement().handle,
        handle("BOARD-public")
    );
    assert_eq!(
        inventory
            .hidden()
            .map(|group| (group.stem().to_owned(), group.entries().len()))
            .collect::<Vec<_>>(),
        vec![
            ("maxim_proposal".to_owned(), 1),
            ("oops".to_owned(), 1),
            ("sandbox".to_owned(), 1),
        ]
    );
}

#[test]
fn stealing_waives_possession_alone_and_waits_on_no_lease() {
    let actor = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e408"));
    let dependent = {
        let mut head = statement("BOARD-blocked");
        head.dependencies.push(handle("BOARD-absent"));
        head
    };
    let deferred = {
        let mut head = workflow("todo");
        head.deferral = Some(instant("2026-08-01T07:00:00.000Z"));
        head
    };
    let entries = vec![
        claimed_entry("BOARD-held", "todo"),
        entry("BOARD-free"),
        admitted(
            dependent,
            workflow("todo"),
            Some(claim(ClaimDisposition::Active)),
            Some(lease()),
        )
        .expect("claimed heads awaiting a dependency"),
        admitted(
            statement("BOARD-deferred"),
            deferred,
            Some(claim(ClaimDisposition::Active)),
            Some(lease()),
        )
        .expect("claimed heads sleeping until later"),
    ];
    let [held, free, blocked, sleeping] = &entries[..] else {
        panic!("four possession fixtures");
    };

    // lease() runs to 06:10 and is nowhere near spent here.
    assert!(!board::eligible_entry(&entries, held, &actor));
    assert!(board::stealable_entry(&entries, held, &actor));

    // Possession is the only clause a steal waives.
    assert!(board::eligible_entry(&entries, free, &actor));
    assert!(board::stealable_entry(&entries, free, &actor));
    assert!(!board::stealable_entry(&entries, blocked, &actor));
    assert!(!board::stealable_entry(&entries, sleeping, &actor));
}

#[test]
fn policy_snapshot_retains_hidden_rows_as_explicit_evidence() {
    let projects =
        board::ProjectPolicy::new(["board".to_owned()], Vec::new(), 2, 3).expect("fixture policy");
    let mut hidden = statement("BOARD-hidden");
    hidden.project = ".oops.correctness".to_owned();
    let journal = journal_with_heads(hidden, workflow("todo"));
    let snapshot =
        Snapshot::read(&journal, &projects).expect("hidden evidence remains in the snapshot");

    assert_eq!(snapshot.entries().len(), 1);
    let entry = &snapshot.entries()[0];
    assert_eq!(entry.project().as_str(), ".oops.correctness");
    assert_eq!(entry.project().visibility().hidden_stem(), Some("oops"));
}

#[test]
fn snapshot_is_a_schema_and_cursor_bounded_head_answer() {
    let entries = [entry("BOARD-a1")];
    let cursor = Cursor::at(seq(9));
    let snapshot =
        Snapshot::new(journal::SCHEMA_VERSION, cursor, entries.to_vec()).expect("current schema");

    assert_eq!(snapshot.schema_version(), journal::SCHEMA_VERSION);
    assert_eq!(snapshot.cursor(), cursor);
    assert_eq!(snapshot.entries(), entries.as_slice());
    assert!(Snapshot::new(journal::SCHEMA_VERSION + 1, cursor, entries.to_vec()).is_err());
}

#[test]
fn durable_snapshot_owns_one_validated_journal_answer() {
    let snapshot = {
        let journal = journal_with_heads(statement("BOARD-a1"), workflow("todo"));
        Snapshot::read(&journal, &projects()).expect("authoritative board snapshot")
    };

    assert_eq!(snapshot.schema_version(), journal::SCHEMA_VERSION);
    assert_eq!(snapshot.cursor(), Cursor::at(seq(1)));
    assert_eq!(snapshot.entries().len(), 1);
    assert_eq!(snapshot.entries()[0].statement().handle, handle("BOARD-a1"));
}

#[test]
fn durable_snapshot_names_the_task_whose_heads_violate_board_policy() {
    let mut invalid = workflow("todo");
    invalid.flow.clear();
    let journal = journal_with_heads(statement("BOARD-a1"), invalid);

    let error =
        Snapshot::read(&journal, &projects()).expect_err("empty flow refuses board snapshot");
    assert!(matches!(
        error,
        SnapshotReadError::Task {
            handle: ref observed,
            source: board::TaskError::EmptyFlow,
        } if observed == &handle("BOARD-a1")
    ));
    assert!(
        error
            .to_string()
            .contains("board snapshot task BOARD-a1 is invalid")
    );
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

    let freed = admitted(
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

    // Possession is two heads, and admission names which one is absent rather
    // than reporting the pair as merely incoherent. Both arms are reachable
    // from the resident, which admits through this predicate before it renews
    // or frees, so a half-written pair has to refuse by name here.
    let unclaimed =
        admitted(statement("BOARD-a1"), workflow("todo"), None, None).expect("valid heads");
    assert!(matches!(
        Active::admit(
            &unclaimed,
            &Actor::Spice,
            instant("2026-08-01T06:05:00.000Z")
        ),
        Err(ClaimRefusal::MissingClaim)
    ));

    let leaseless = admitted(
        statement("BOARD-a1"),
        workflow("todo"),
        Some(claim(ClaimDisposition::Active)),
        None,
    )
    .expect("valid heads");
    assert!(matches!(
        Active::admit(
            &leaseless,
            &Actor::Spice,
            instant("2026-08-01T06:05:00.000Z")
        ),
        Err(ClaimRefusal::MissingLease)
    ));
}

#[test]
fn active_claim_refuses_a_lease_whose_start_has_not_arrived() {
    let entry = claimed_entry("BOARD-a1", "todo");

    // The sibling case bounds the lease from above. This one bounds it from
    // below, and the two are separate guards rather than one interval test: a
    // claim written ahead of its start is a lease nobody holds yet, and the
    // refusal carries the instant it begins so the caller need not read it back
    // off a head it was just refused possession of.
    assert!(matches!(
        Active::admit(&entry, &Actor::Spice, instant("2026-08-01T05:59:59.999Z")),
        Err(ClaimRefusal::NotStarted { start }) if start == instant("2026-08-01T06:00:00.000Z")
    ));

    // The boundary is inclusive at the start instant, which fixes the
    // comparison in both directions. A guard reading `now <= lease.start` would
    // refuse this admission, and one reading `now < lease.renewal` would refuse
    // it as well.
    Active::admit(&entry, &Actor::Spice, instant("2026-08-01T06:00:00.000Z"))
        .expect("a lease is held from its start instant");
}

#[test]
fn active_claim_refuses_a_split_epoch_and_each_out_of_order_lease_bound() {
    let live = instant("2026-08-01T06:05:00.000Z");

    // The epoch pair is what binds a lease to the claim that opened it. A lease
    // renewed under one epoch cannot vouch for a claim recorded under another,
    // so the refusal names both sides rather than reporting a bare disagreement.
    let mut divergent = claim(ClaimDisposition::Active);
    divergent.epoch = uuid("019fbbde-5105-75a3-a76a-722afde1e411");
    let split = admitted(
        statement("BOARD-a1"),
        workflow("todo"),
        Some(divergent),
        Some(lease()),
    )
    .expect("valid heads");
    assert!(matches!(
        Active::admit(&split, &Actor::Spice, live),
        Err(ClaimRefusal::Epoch {
            claim: recorded,
            lease: vouching,
        }) if recorded == uuid("019fbbde-5105-75a3-a76a-722afde1e411")
            && vouching == uuid("019fbbde-5105-75a3-a76a-722afde1e410")
    ));

    // Coherence is a chain of three inequalities, and one case can only prove
    // one link. Each lease below breaks exactly one and satisfies the other two,
    // so no surviving comparison can carry the refusal for its neighbours.
    for (broken, incoherent) in [
        (
            "renewal before start",
            LeaseHead {
                renewal: instant("2026-08-01T05:59:00.000Z"),
                ..lease()
            },
        ),
        (
            "renewal after expiry",
            LeaseHead {
                renewal: instant("2026-08-01T06:11:00.000Z"),
                ..lease()
            },
        ),
        (
            "expiry past maximum",
            LeaseHead {
                expiry: instant("2026-08-01T06:21:00.000Z"),
                ..lease()
            },
        ),
    ] {
        let entry = admitted(
            statement("BOARD-a1"),
            workflow("todo"),
            Some(claim(ClaimDisposition::Active)),
            Some(incoherent),
        )
        .expect("valid heads");
        assert!(
            matches!(
                Active::admit(&entry, &Actor::Spice, live),
                Err(ClaimRefusal::IncoherentLease)
            ),
            "{broken} leaves the lease incoherent",
        );
    }
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
fn active_claim_for_another_task_cannot_advance_workflow() {
    let owned = claimed_entry("BOARD-a1", "todo");
    let active = Active::admit(&owned, &Actor::Spice, instant("2026-08-01T06:05:00.000Z"))
        .expect("active claim");
    let other = claimed_entry("BOARD-b2", "todo");

    assert_eq!(
        Transition::advance(&other, &active, phase("review")).expect_err("claim is task-bound"),
        PhaseRefusal::ClaimForAnotherTask
    );
}

#[test]
fn completed_workflow_refuses_both_advance_and_completion() {
    let completed = completed_entry("BOARD-a1");
    let active = Active::admit(
        &completed,
        &Actor::Spice,
        instant("2026-08-01T06:05:00.000Z"),
    )
    .expect("active claim");

    assert_eq!(
        Transition::advance(&completed, &active, phase("todo"))
            .expect_err("completed workflow cannot advance"),
        PhaseRefusal::Completed
    );
    assert_eq!(
        Transition::complete(&completed, &active, instant("2026-08-01T06:07:00.000Z"))
            .expect_err("completed workflow cannot complete again"),
        PhaseRefusal::Completed
    );
}

#[test]
fn advance_target_outside_declared_flow_is_named() {
    let entry = claimed_entry("BOARD-a1", "todo");
    let active = Active::admit(&entry, &Actor::Spice, instant("2026-08-01T06:05:00.000Z"))
        .expect("active claim");
    let outside = phase("verify");

    assert_eq!(
        Transition::advance(&entry, &active, outside.clone()).expect_err("outside target refuses"),
        PhaseRefusal::TargetOutsideFlow(outside)
    );
}

#[test]
fn urgency_is_a_pure_total_scalar_over_heads() {
    let completed = BTreeSet::new();
    let plain = entry("BOARD-a1");
    let mut important_head = statement("BOARD-b2");
    important_head.tags.push("priority:high".to_owned());
    let important = admitted(important_head, workflow("todo"), None, None).expect("valid heads");
    let claimed = claimed_entry("BOARD-c3", "todo");

    assert_eq!(
        Urgency::from_heads(&plain, &completed),
        Urgency::from_heads(&plain, &completed)
    );
    assert!(Urgency::from_heads(&important, &completed) > Urgency::from_heads(&plain, &completed));
    assert!(Urgency::from_heads(&claimed, &completed) < Urgency::from_heads(&plain, &completed));
}

#[test]
fn urgency_band_changes_only_the_winner_inside_one_priority_class() {
    let mut assigned_workflow = workflow("todo");
    assigned_workflow.assignment = Some(Actor::Spice);
    let lower_handle =
        admitted(statement("BOARD-a1"), assigned_workflow, None, None).expect("assigned candidate");
    let higher_handle = entry("BOARD-z1");
    let ordered = [lower_handle.clone(), higher_handle.clone()];
    let reversed = [higher_handle, lower_handle];

    let outside = select(&ordered, &Actor::Spice, &all_projects(), 99).expect("eligible task");
    assert_eq!(outside.entry().statement().handle, handle("BOARD-z1"));
    let inside = select(&ordered, &Actor::Spice, &all_projects(), 100).expect("eligible task");
    let reversed_inside =
        select(&reversed, &Actor::Spice, &all_projects(), 100).expect("eligible task");
    assert_eq!(inside.entry().statement().handle, handle("BOARD-a1"));
    assert_eq!(
        reversed_inside.entry().statement().handle,
        inside.entry().statement().handle,
    );
}

#[test]
fn self_review_is_a_last_resort_class_instead_of_an_exclusion() {
    let mut self_review_head = statement("BOARD-a1");
    self_review_head.tags.push("priority:critical".to_owned());
    let self_review = admitted(
        self_review_head,
        workflow("review"),
        Some(claim(ClaimDisposition::Freed)),
        Some(lease()),
    )
    .expect("self-authored review");
    let peer_work = entry("BOARD-z1");

    let entries = [self_review.clone(), peer_work];
    let selected =
        select(&entries, &Actor::Spice, &all_projects(), 2).expect("peer work remains eligible");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z1"));
    let fallback_entries = [self_review];
    let fallback = select(&fallback_entries, &Actor::Spice, &all_projects(), 2)
        .expect("own review remains eligible on a quiet board");
    assert_eq!(fallback.entry().statement().handle, handle("BOARD-a1"));
}

#[test]
fn allocation_is_deterministic_and_has_one_eligibility_path() {
    let first = entry("BOARD-a1");
    let second = entry("BOARD-b2");

    let mut blocked_head = statement("BOARD-z9");
    blocked_head.tags.push("priority:critical".to_owned());
    blocked_head.dependencies.push(handle("WIRE-missing"));
    let blocked =
        admitted(blocked_head, workflow("todo"), None, None).expect("valid blocked heads");

    let mut assigned_workflow = workflow("todo");
    assigned_workflow.assignment = Some(Actor::Operator);
    let mut assigned_head = statement("BOARD-y8");
    assigned_head.tags.push("priority:critical".to_owned());
    let assigned =
        admitted(assigned_head, assigned_workflow, None, None).expect("valid assigned heads");

    let active = claimed_entry("BOARD-x7", "todo");
    let mut deferred_workflow = workflow("todo");
    deferred_workflow.deferral = Some(instant("2026-08-01T07:00:00.000Z"));
    let mut deferred_head = statement("BOARD-w6");
    deferred_head.tags.push("priority:critical".to_owned());
    let deferred =
        admitted(deferred_head, deferred_workflow, None, None).expect("valid deferred heads");

    let mut completed_workflow = workflow("todo");
    completed_workflow.status = "completed".to_owned();
    completed_workflow.completion = Some(instant("2026-08-01T06:30:00.000Z"));
    let mut completed_head = statement("BOARD-v5");
    completed_head.tags.push("priority:critical".to_owned());
    let completed =
        admitted(completed_head, completed_workflow, None, None).expect("valid completed heads");
    let ordered = [
        blocked.clone(),
        second.clone(),
        assigned.clone(),
        first.clone(),
        active.clone(),
        deferred.clone(),
        completed.clone(),
    ];
    let reversed = [
        completed, deferred, active, first, assigned, second, blocked,
    ];

    let selected = select(&ordered, &Actor::Spice, &all_projects(), 2).expect("eligible task");
    let selected_again =
        select(&reversed, &Actor::Spice, &all_projects(), 2).expect("eligible task");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-a1"));
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle
    );
    assert_eq!(selected_again.urgency(), selected.urgency());
}

#[test]
fn prerequisite_inherits_the_critical_priority_it_directly_unblocks() {
    let prerequisite = entry("BOARD-z1");
    let mut critical_head = statement("BOARD-c1");
    critical_head.tags.push("priority:critical".to_owned());
    critical_head.dependencies.push(handle("BOARD-z1"));
    let critical = admitted(critical_head, workflow("todo"), None, None).expect("critical task");
    let mut medium_head = statement("BOARD-m1");
    medium_head.tags.push("priority:medium".to_owned());
    let medium = admitted(medium_head, workflow("todo"), None, None).expect("medium task");
    let ordered = [prerequisite.clone(), critical.clone(), medium.clone()];
    let reversed = [medium, critical, prerequisite];

    let selected = select(&ordered, &Actor::Spice, &all_projects(), 2).expect("eligible task");
    let selected_again =
        select(&reversed, &Actor::Spice, &all_projects(), 2).expect("eligible task");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z1"));
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle,
    );
    assert_eq!(selected_again.urgency(), selected.urgency());
}

#[test]
fn prerequisite_inherits_priority_through_a_two_edge_chain() {
    let first = entry("BOARD-z1");
    let mut second_head = statement("BOARD-z2");
    second_head.dependencies.push(handle("BOARD-z1"));
    let second = admitted(second_head, workflow("todo"), None, None).expect("second prerequisite");
    let mut critical_head = statement("BOARD-c1");
    critical_head.tags.push("priority:critical".to_owned());
    critical_head.dependencies.push(handle("BOARD-z2"));
    let critical = admitted(critical_head, workflow("todo"), None, None).expect("critical task");
    let mut medium_head = statement("BOARD-m1");
    medium_head.tags.push("priority:medium".to_owned());
    let medium = admitted(medium_head, workflow("todo"), None, None).expect("medium task");
    let entries = [first, second, critical, medium];

    let selected = select(&entries, &Actor::Spice, &all_projects(), 2).expect("eligible task");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z1"));
}

#[test]
fn priority_inheritance_terminates_on_a_dependency_cycle() {
    let mut first_head = statement("BOARD-z1");
    first_head.tags.push("priority:critical".to_owned());
    first_head.dependencies.push(handle("BOARD-z2"));
    let first = admitted(first_head, workflow("todo"), None, None).expect("first cycle entry");
    let mut second_head = statement("BOARD-z2");
    second_head.dependencies.push(handle("BOARD-z1"));
    let second = admitted(second_head, workflow("todo"), None, None).expect("second cycle entry");
    let medium = entry("BOARD-m1");

    let entries = [first, second, medium];
    let selected = select(&entries, &Actor::Spice, &all_projects(), 2)
        .expect("cycle leaves the independent task eligible");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-m1"));
}

#[test]
fn downstream_weight_precedes_native_urgency_and_handle_order() {
    let eager = entry("BOARD-a1");
    let mut weighted_workflow = workflow("todo");
    weighted_workflow.assignment = Some(Actor::Spice);
    let weighted = admitted(statement("BOARD-z1"), weighted_workflow, None, None)
        .expect("assigned weighted candidate");
    let mut direct_head = statement("BOARD-z2");
    direct_head.dependencies.push(handle("BOARD-z1"));
    let direct = admitted(direct_head, workflow("todo"), None, None).expect("direct dependent");
    let mut transitive_head = statement("BOARD-z3");
    transitive_head.dependencies.push(handle("BOARD-z2"));
    let transitive =
        admitted(transitive_head, workflow("todo"), None, None).expect("transitive dependent");

    assert!(
        Urgency::from_heads(&eager, &BTreeSet::new())
            > Urgency::from_heads(&weighted, &BTreeSet::new()),
        "native urgency favors the unweighted candidate",
    );
    let ordered = [
        eager.clone(),
        weighted.clone(),
        direct.clone(),
        transitive.clone(),
    ];
    let reversed = [transitive, direct, weighted, eager];

    // Removing the downstream rung makes urgency choose BOARD-a1; retaining
    // the rung but moving it above effective priority is caught separately.
    let selected =
        select(&ordered, &Actor::Spice, &all_projects(), 2).expect("weighted candidate is ready");
    let selected_again = select(&reversed, &Actor::Spice, &all_projects(), 2)
        .expect("reversed weighted candidate is ready");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z1"));
    assert_eq!(selected.downstream_weight(), 2);
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle,
    );
    assert_eq!(
        selected_again.downstream_weight(),
        selected.downstream_weight(),
    );
    assert_eq!(selected_again.urgency(), selected.urgency());
}

#[test]
fn effective_priority_precedes_greater_downstream_weight() {
    let mut priority_head = statement("BOARD-z9");
    priority_head.tags.push("priority:medium".to_owned());
    let priority =
        admitted(priority_head, workflow("todo"), None, None).expect("higher-priority candidate");
    let weighted = entry("BOARD-a1");
    let mut direct_head = statement("BOARD-a2");
    direct_head.dependencies.push(handle("BOARD-a1"));
    let direct = admitted(direct_head, workflow("todo"), None, None).expect("direct dependent");
    let mut transitive_head = statement("BOARD-a3");
    transitive_head.dependencies.push(handle("BOARD-a2"));
    let transitive =
        admitted(transitive_head, workflow("todo"), None, None).expect("transitive dependent");
    let entries = [weighted, direct, transitive, priority];

    // Moving downstream weight above effective priority makes BOARD-a1 win.
    let selected =
        select(&entries, &Actor::Spice, &all_projects(), 2).expect("priority candidate is ready");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z9"));
    assert_eq!(selected.downstream_weight(), 0);
}

#[test]
fn downstream_weight_ignores_hidden_and_settled_branches() {
    let root = entry("BOARD-a1");
    let mut hidden_head = statement("BOARD-h1");
    hidden_head.project = ".oops.correctness".to_owned();
    hidden_head.dependencies.push(handle("BOARD-a1"));
    let hidden = admitted(hidden_head, workflow("todo"), None, None).expect("hidden dependent");
    let mut after_hidden_head = statement("BOARD-z1");
    after_hidden_head.dependencies.push(handle("BOARD-h1"));
    let after_hidden = admitted(after_hidden_head, workflow("todo"), None, None)
        .expect("public task beyond hidden branch");

    let mut settled_head = statement("BOARD-s1");
    settled_head.dependencies.push(handle("BOARD-a1"));
    let mut settled_workflow = workflow("review");
    settled_workflow.status = "completed".to_owned();
    settled_workflow.completion = Some(instant("2026-08-01T06:06:00.000Z"));
    let settled = admitted(
        settled_head,
        settled_workflow,
        Some(claim(ClaimDisposition::Active)),
        Some(lease()),
    )
    .expect("settled dependent");
    let mut after_settled_head = statement("BOARD-z2");
    after_settled_head.dependencies.push(handle("BOARD-s1"));
    let after_settled = admitted(after_settled_head, workflow("todo"), None, None)
        .expect("public task beyond settled branch");
    let ordered = [
        root.clone(),
        hidden.clone(),
        after_hidden.clone(),
        settled.clone(),
        after_settled.clone(),
    ];
    let reversed = [after_settled, settled, after_hidden, hidden, root];

    let selected = select(&ordered, &Actor::Spice, &all_projects(), 2)
        .expect("root remains an unweighted candidate");
    let selected_again = select(&reversed, &Actor::Spice, &all_projects(), 2)
        .expect("reversed root remains an unweighted candidate");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-a1"));
    assert_eq!(selected.downstream_weight(), 0);
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle,
    );
    assert_eq!(selected_again.downstream_weight(), 0);
}

#[test]
fn downstream_weight_terminates_on_a_dependency_cycle() {
    let root = entry("BOARD-z1");
    let mut first_head = statement("BOARD-z2");
    first_head
        .dependencies
        .extend([handle("BOARD-z1"), handle("BOARD-z3")]);
    let first = admitted(first_head, workflow("todo"), None, None).expect("first cycle entry");
    let mut second_head = statement("BOARD-z3");
    second_head.dependencies.push(handle("BOARD-z2"));
    let second = admitted(second_head, workflow("todo"), None, None).expect("second cycle entry");
    let ordered = [root.clone(), first.clone(), second.clone()];
    let reversed = [second, first, root];

    let selected =
        select(&ordered, &Actor::Spice, &all_projects(), 2).expect("cycle root is ready");
    let selected_again =
        select(&reversed, &Actor::Spice, &all_projects(), 2).expect("reversed cycle root is ready");
    assert_eq!(selected.entry().statement().handle, handle("BOARD-z1"));
    assert_eq!(selected.downstream_weight(), 2);
    assert_eq!(
        selected_again.entry().statement().handle,
        selected.entry().statement().handle,
    );
    assert_eq!(selected_again.downstream_weight(), 2);
}

#[test]
fn task_subscription_scope_yields_exact_steer_drive_and_drain_sets() {
    let lane = uuid("019fbbde-5105-75a3-a76a-722afde1e408");
    let other = uuid("019fbbde-5105-75a3-a76a-722afde1e409");
    let actor = Actor::Lane(lane);
    let entry = |handle: &str, project: &str, assignment: Option<Actor>| {
        let mut statement = statement(handle);
        statement.project = project.to_owned();
        if project == "task.unrelated" {
            statement.tags.push("priority:critical".to_owned());
        }
        let mut workflow = workflow("todo");
        workflow.assignment = assignment;
        admitted(statement, workflow, None, None).expect("scoped fixture entry")
    };
    let entries = vec![
        entry("SCOPE-manual1", "task.manual", None),
        entry("SCOPE-auto001", "task.learned", None),
        entry("SCOPE-open001", "task.unrelated", None),
        entry(
            "SCOPE-closed1",
            "task.unassignable",
            Some(Actor::Lane(other)),
        ),
    ];
    let handles = |subscription: &wire::LineageSubscription| {
        entries
            .iter()
            .filter(|entry| board::selectable_entry(&entries, entry, &actor, subscription))
            .map(|entry| entry.statement().handle.to_string())
            .collect::<BTreeSet<_>>()
    };
    let subscription = |policy| wire::LineageSubscription {
        lineage_id: lane,
        policy,
        manual_projects: vec!["task.manual".to_owned()],
        auto_projects: vec!["task.learned".to_owned(), "task.unassignable".to_owned()],
    };

    assert_eq!(
        handles(&subscription(wire::intent::LineageLife::Steer)),
        BTreeSet::from(["SCOPE-manual1".to_owned()]),
    );
    assert_eq!(
        handles(&subscription(wire::intent::LineageLife::Drive)),
        BTreeSet::from(["SCOPE-auto001".to_owned(), "SCOPE-manual1".to_owned(),]),
    );
    assert_eq!(
        handles(&subscription(wire::intent::LineageLife::Drain)),
        BTreeSet::from([
            "SCOPE-auto001".to_owned(),
            "SCOPE-manual1".to_owned(),
            "SCOPE-open001".to_owned(),
        ]),
    );
    assert_eq!(
        select(
            &entries,
            &actor,
            &subscription(wire::intent::LineageLife::Steer),
            2,
        )
        .expect("steer has manual work")
        .entry()
        .statement()
        .handle,
        handle("SCOPE-manual1"),
        "a higher-urgency unrelated task must not cross the Steer boundary",
    );
    assert_eq!(
        select(
            &entries,
            &actor,
            &subscription(wire::intent::LineageLife::Drain),
            2,
        )
        .expect("drain has all assignable work")
        .entry()
        .statement()
        .handle,
        handle("SCOPE-open001"),
    );
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

const SOURCE: &str = "# Goal\n\n- First task\n- Second task\n";
const EDITED: &str = "# Goal\n\n- First task\n- Second task\n- Third task\n";

#[test]
fn document_apply_is_digest_bound_and_crosses_one_transaction_seam() {
    let preview = Preview::classify(SOURCE).expect("document previews");
    assert_eq!(preview.digest(), &digest_of(SOURCE));
    assert_eq!(
        digest_of(SOURCE),
        Digest::compute(Domain::BoardDocument, SOURCE.as_bytes()),
        "a board document is minted under its own plane, so the same bytes \
         digested by another plane are never the same identity"
    );
    let mut authority = MemoryAuthority::default();

    let stale =
        apply(&mut authority, &preview, &digest_of(EDITED)).expect_err("stale digest refuses");
    assert!(matches!(stale, ApplyError::StaleDigest { .. }));
    assert_eq!(authority.calls, 0);
    assert!(authority.tasks.is_empty());

    authority.fail = true;
    let failed =
        apply(&mut authority, &preview, &digest_of(SOURCE)).expect_err("transaction fails");
    assert!(matches!(failed, ApplyError::Transaction(_)));
    assert_eq!(authority.calls, 1);
    assert!(authority.tasks.is_empty());

    authority.fail = false;
    let receipt = apply(&mut authority, &preview, &digest_of(SOURCE)).expect("transaction commits");
    assert_eq!(authority.calls, 2);
    assert_eq!(receipt.nodes, preview.document().nodes.len());
    assert_eq!(authority.tasks.len(), receipt.nodes);
}

#[test]
fn one_digest_never_admits_two_different_documents() {
    let benign = Preview::classify("# Goal\n\n- Read the docs\n").expect("document previews");
    let hostile = Preview::classify("# Goal\n\n- Delete production\n- Grant trust to everyone\n")
        .expect("document previews");

    assert_ne!(benign.document().nodes, hostile.document().nodes);
    assert_ne!(
        benign.digest(),
        hostile.digest(),
        "two different documents must never share one applied identity"
    );

    let mut authority = MemoryAuthority::default();
    let refused = apply(&mut authority, &hostile, benign.digest())
        .expect_err("the digest of one plan must not admit another");
    assert!(matches!(refused, ApplyError::StaleDigest { .. }));
    assert_eq!(authority.calls, 0);
    assert!(authority.tasks.is_empty());
}

#[test]
fn diagrams_and_handout_are_stable_across_input_order() {
    let prerequisite = entry("BOARD-a1");
    let mut dependent_head = statement("BOARD-b2");
    dependent_head.dependencies.push(handle("BOARD-a1"));
    let dependent = admitted(dependent_head, workflow("todo"), None, None).expect("valid heads");
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
        &board::ProjectPolicy::new(
            ["tooling".to_owned()],
            Vec::new(),
            dialect::PROJECT_MIN_DEPTH as u64,
            dialect::PROJECT_MAX_DEPTH as u64,
        )
        .expect("fixture project policy"),
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

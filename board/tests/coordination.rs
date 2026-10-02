mod support;

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::apply::{ApplyError, AuthorityTransaction};
use board::claim::ClaimRefusal;
use board::oops::{Friction, Report, Routed};
use board::phase::PhaseRefusal;
use board::{
    Active, Entry, Preview, Snapshot, Transition, Urgency, View, apply, digest_of, graph, handout,
};
use journal::{FactCause, Journal, JournalWrite};
use wire::JournalCursor as Cursor;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Digest, Domain, Instant, LeaseHead, Origin,
    Phase, Producer, Seq, TaskHandle, TaskHead, TaskStamp, WorkflowHead, body,
};

fn stamp(text: &str) -> TaskStamp {
    let value = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    TaskStamp::for_micros(value).expect("fixture name fits the task stamp codec")
}

fn handle(text: &str) -> TaskHandle {
    TaskHandle::for_project("board.kernel", &stamp(text))
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

fn environment() -> wire::Environment {
    wire::Environment::journal_schema(journal::SCHEMA_VERSION)
        .expect("the compiled journal schema is positive")
}

fn all_projects() -> wire::TeamSubscription {
    wire::TeamSubscription {
        team: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        policy: wire::intent::TeamLife::Drain,
    }
}

fn select<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &wire::TeamSubscription,
    band: u64,
) -> Option<board::Selection<'a>> {
    board::select(entries, actor, subscription, band, None, &environment())
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

fn snapshot_for(
    journal: &Journal,
    projects: &board::ProjectPolicy,
    task: &TaskStamp,
) -> Result<Snapshot, board::TaskError> {
    support::bounded_snapshot(journal, projects, std::slice::from_ref(task))
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
    let born = stamp(name);
    TaskHead {
        born: born.clone(),
        queue_origin: seq(1),
        title: format!("Implement {name}"),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "board.kernel".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: format!("task:{born}").parse::<Origin>().expect("origin"),
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
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
        generation: 0,
    }
}

fn claim(disposition: ClaimDisposition) -> ClaimHead {
    ClaimHead {
        actor: Actor::Spice,
        fact: seq(2),
        claimed: seq(2),
        cell: wire::ClaimCell {
            project: "board.kernel".to_owned(),
            phase: phase("todo"),
        },
        lane: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        occupant: uuid("019fbbde-5105-75a3-a76a-722afde1e409"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
        recovery: None,
        disposition,
        acknowledged: 0,
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
    let task = statement.born.clone();
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
                title: statement.title.clone(),
                description: statement.description.clone(),
                acceptance: statement.acceptance.clone(),
                requirements: statement.requirements.clone(),
                project: statement.project.clone(),
                tags: statement.tags.clone(),
                depends: statement.dependencies.clone(),
                flow: workflow.flow.clone(),
                phase: workflow.phase.clone(),
                revision: 1,
                creation: statement.creation.clone(),
            },
            FactCause::Root,
        )
        .expect("registered board fixture fact");
    write.task_head(fact, move |position| {
        statement.queue_origin = position;
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
    head.dependencies = vec![stamp("WIRE-b2"), stamp("JOURNAL-c3")];
    let entry = admitted(head, workflow("todo"), None, None).expect("valid heads");

    assert_eq!(entry.project().as_str(), "board.kernel");
    assert_eq!(entry.family().as_str(), "KERNEL");
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
                task: stamp("BOARD-a1"),
                after: stamp("WIRE-b2"),
            },
            board::Dependency {
                task: stamp("BOARD-a1"),
                after: stamp("JOURNAL-c3"),
            },
        ]
    );

    let mut self_dependent = statement("BOARD-a1");
    self_dependent.dependencies = vec![stamp("BOARD-a1")];
    assert!(matches!(
        admitted(self_dependent, workflow("todo"), None, None),
        Err(board::TaskError::SelfDependency(_))
    ));

    let mut duplicate = statement("BOARD-a1");
    duplicate.dependencies = vec![stamp("WIRE-b2"), stamp("WIRE-b2")];
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
fn board_graph_views_render_independent_relations_in_stable_order() {
    let reviewer = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e411"));
    let author = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e412"));
    let mut alpha_head = statement("BOARD-a1");
    alpha_head.provenance = "task:BOARD-root".parse().expect("fixture origin");
    alpha_head.dependencies.push(stamp("BOARD-b2"));
    let mut alpha_workflow = workflow("review");
    alpha_workflow.review = "assigned".to_owned();
    alpha_workflow.assignment = Some(reviewer.clone());
    let alpha = admitted(
        alpha_head,
        alpha_workflow,
        Some(claim(ClaimDisposition::Active)),
        Some(lease()),
    )
    .expect("valid reviewed and claimed entry");
    let beta = entry("BOARD-b2");
    let entries = vec![beta.clone(), alpha.clone()];
    let stable_entries = vec![alpha.clone(), beta.clone()];
    let reviews = vec![graph::ReviewEdge::new(
        alpha.handle().clone(),
        phase("todo"),
        reviewer.clone(),
        author.clone(),
        body::Verdict::Clean,
        seq(50),
    )];

    for view in View::ALL {
        assert_eq!(
            graph::render(view, &entries, &reviews).text(),
            graph::render(view, &stable_entries, &reviews).text(),
            "{} graph depends on input order",
            view.as_str()
        );
    }
    assert_eq!(
        View::ALL.map(View::as_str),
        ["origin", "dependencies", "review", "workflow", "claims"]
    );

    let origin = graph::render(View::Origin, &entries, &reviews)
        .text()
        .to_owned();
    assert!(origin.contains(&format!("task:BOARD-root -> {}", alpha.handle())));
    assert!(origin.contains(&format!(
        "{} -> {}",
        beta.statement().provenance,
        beta.handle()
    )));
    assert!(!origin.contains("[assigned]"));

    let dependencies = graph::render(View::Dependencies, &entries, &reviews)
        .text()
        .to_owned();
    assert_eq!(
        dependencies,
        format!(
            "graph dependencies\n{} -> {}\n",
            alpha.handle(),
            beta.handle()
        )
    );

    let review = graph::render(View::Review, &entries, &reviews)
        .text()
        .to_owned();
    assert_eq!(
        review,
        format!(
            "graph review\n{} -> {} [{} todo clean]\n",
            reviewer,
            author,
            alpha.handle(),
        )
    );
    assert!(!review.contains("active"));
    assert!(!review.contains("assigned"));

    let workflow = graph::render(View::Workflow, &entries, &reviews)
        .text()
        .to_owned();
    assert!(workflow.contains(&format!("{}: review [pending]\n", alpha.handle())));
    assert!(workflow.contains(&format!("{}: todo [pending]\n", beta.handle())));
    assert!(!workflow.contains(&reviewer.to_string()));

    let claims = graph::render(View::Claims, &entries, &reviews)
        .text()
        .to_owned();
    assert!(claims.contains(&format!("{}: {} [active]\n", alpha.handle(), Actor::Spice)));
    assert!(!claims.contains("assigned"));

    let handout = handout::compose(&entries, &reviews);
    let sections = [
        "## origin",
        "## dependencies",
        "## review",
        "## workflow",
        "## claims",
    ];
    let positions = sections.map(|section| handout.find(section).expect("section is present"));
    assert!(positions.windows(2).all(|window| window[0] < window[1]));
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
        (board::SystemTaskRoute::oops(None), ".oops"),
        (
            board::SystemTaskRoute::oops(Some(
                &projects
                    .admit_manual_creation("board.cli")
                    .expect("recommended public project"),
            )),
            ".oops.board.cli",
        ),
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
    let public_handle = entries[2].handle().clone();

    let lane = uuid("019fbbde-5105-75a3-a76a-722afde1e408");
    for actor in [Actor::Spice, Actor::Operator, Actor::Lane(lane)] {
        assert_eq!(
            select(&entries, &actor, &all_projects(), 2)
                .expect("public task remains eligible")
                .entry()
                .handle(),
            &public_handle
        );
        for hidden in [&entries[0], &entries[1], &entries[3]] {
            assert!(!board::eligible_entry(
                &entries,
                hidden,
                &actor,
                &environment()
            ));
        }
    }

    let inventory = board::Inventory::new(&entries);
    assert_eq!(inventory.public().len(), 1);
    assert_eq!(inventory.public()[0].handle(), &public_handle);
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
        head.dependencies.push(stamp("BOARD-absent"));
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
    assert!(!board::eligible_entry(
        &entries,
        held,
        &actor,
        &environment()
    ));
    assert!(board::stealable_entry(
        &entries,
        held,
        &actor,
        &environment()
    ));

    // Possession is the only clause a steal waives.
    assert!(board::eligible_entry(
        &entries,
        free,
        &actor,
        &environment()
    ));
    assert!(board::stealable_entry(
        &entries,
        free,
        &actor,
        &environment()
    ));
    assert!(!board::stealable_entry(
        &entries,
        blocked,
        &actor,
        &environment()
    ));
    assert!(!board::stealable_entry(
        &entries,
        sleeping,
        &actor,
        &environment()
    ));
}

#[test]
fn environment_is_the_final_eligibility_filter_and_never_ranking_or_task_state() {
    let actor = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e408"));
    let current_schema = journal::SCHEMA_VERSION;
    let required_schema = current_schema
        .checked_add(1)
        .expect("fixture schema advances");
    let mut blocked_head = statement("BOARD-environment-blocked");
    blocked_head.tags.push("priority:critical".to_owned());
    blocked_head.requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::journal_schema(required_schema)
            .expect("future schema is positive"),
    );
    let blocked = admitted(blocked_head, workflow("todo"), None, None).expect("blocked entry");
    let ordinary = entry("BOARD-environment-ready");
    let entries = vec![blocked.clone(), ordinary.clone()];

    let observed_before =
        wire::Environment::journal_schema(current_schema).expect("current schema is positive");
    assert!(!board::environment_satisfied(&blocked, &observed_before));
    assert!(!board::eligible_entry(
        &entries,
        &blocked,
        &actor,
        &observed_before
    ));
    assert!(!board::capturable_entry(
        &entries,
        &blocked,
        &actor,
        &observed_before
    ));
    assert!(!board::selectable_entry(
        &entries,
        &blocked,
        &actor,
        &all_projects(),
        &observed_before
    ));
    assert!(!board::stealable_entry(
        &entries,
        &blocked,
        &actor,
        &observed_before
    ));
    assert_eq!(
        select(&entries, &actor, &all_projects(), 2)
            .expect("ordinary eligible work remains")
            .entry()
            .handle(),
        ordinary.handle(),
        "an environment-blocked critical task never enters ranking"
    );

    let observed_after =
        wire::Environment::journal_schema(required_schema).expect("future schema is positive");
    assert!(!board::environment_satisfied(&blocked, &observed_before));
    assert!(board::environment_satisfied(&blocked, &observed_after));
    assert_eq!(
        blocked.statement().requirements.as_slice(),
        entries[0].statement().requirements.as_slice(),
        "observation movement does not mutate task authority"
    );
}

#[test]
fn trust_grant_transition_changes_every_allocator_admission_without_rewriting_the_task() {
    let actor = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e408"));
    let digest: Digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest");
    let mut head = statement("BOARD-trust-blocked");
    head.requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::trust_grant(digest.clone()),
    );
    let entry = admitted(head, workflow("todo"), None, None).expect("trust-gated entry");
    let entries = vec![entry.clone()];
    let observed = |state, worktree: Option<&str>| {
        environment().with_trust_heads([wire::TrustHead {
            digest: digest.clone(),
            state,
            worktree: worktree.map(str::to_owned),
        }])
    };
    let missing = environment();
    let pended = observed(wire::TrustState::Pended, Some("/producer/worktree"));
    let revoked = observed(wire::TrustState::Revoked, Some("/producer/worktree"));
    let unbound = observed(wire::TrustState::Granted, None);
    for blocked in [&missing, &pended, &revoked, &unbound] {
        assert!(!board::environment_satisfied(&entry, blocked));
        assert!(!board::eligible_entry(&entries, &entry, &actor, blocked));
        assert!(!board::capturable_entry(&entries, &entry, &actor, blocked));
        assert!(!board::selectable_entry(
            &entries,
            &entry,
            &actor,
            &all_projects(),
            blocked,
        ));
        assert!(!board::stealable_entry(&entries, &entry, &actor, blocked));
    }

    let granted = observed(wire::TrustState::Granted, Some("/different/worktree"));
    assert!(board::environment_satisfied(&entry, &granted));
    assert!(board::eligible_entry(&entries, &entry, &actor, &granted));
    assert!(board::capturable_entry(&entries, &entry, &actor, &granted));
    assert!(board::selectable_entry(
        &entries,
        &entry,
        &actor,
        &all_projects(),
        &granted,
    ));
    assert!(board::stealable_entry(&entries, &entry, &actor, &granted));
    assert_eq!(
        entry.statement().requirements,
        entries[0].statement().requirements,
        "grant movement never rewrites task authority"
    );
}

#[test]
fn policy_snapshot_retains_hidden_rows_as_explicit_evidence() {
    let projects =
        board::ProjectPolicy::new(["board".to_owned()], Vec::new(), 2, 3).expect("fixture policy");
    let mut hidden = statement("BOARD-hidden");
    hidden.project = ".oops.correctness".to_owned();
    let journal = journal_with_heads(hidden, workflow("todo"));
    let snapshot = snapshot_for(&journal, &projects, &stamp("BOARD-hidden"))
        .expect("hidden evidence remains in the snapshot");

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
        snapshot_for(&journal, &projects(), &stamp("BOARD-a1"))
            .expect("authoritative board snapshot")
    };

    assert_eq!(snapshot.schema_version(), journal::SCHEMA_VERSION);
    assert_eq!(snapshot.cursor(), Cursor::at(seq(1)));
    assert_eq!(snapshot.entries().len(), 1);
    assert_eq!(snapshot.entries()[0].handle(), &handle("BOARD-a1"));
}

#[test]
fn durable_snapshot_names_the_task_whose_heads_violate_board_policy() {
    let mut invalid = workflow("todo");
    invalid.flow.clear();
    let journal = journal_with_heads(statement("BOARD-a1"), invalid);

    assert_eq!(
        snapshot_for(&journal, &projects(), &stamp("BOARD-a1")),
        Err(board::TaskError::EmptyFlow)
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

#[path = "coordination/late.rs"]
mod late;

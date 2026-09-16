use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use bus::{Bus, CommitOutcome};
use journal::crossing::{
    Actor, CanonicalUuid, Command, CommandKind, Digest, EnvironmentRequirements, Origin, Phase,
    Producer, Seq, TaskHandle, TaskHead, TrustHead, TrustState, WireVersion, Witness, WorkflowHead,
    body, intent,
};
use journal::{FactCause, GuardError, Journal, JournalGuard, JournalWrite};

const COMMAND_ONE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const COMMAND_TWO: &str = "018f82d4-6f59-7d06-92e3-bce9131163c5";

#[test]
fn steering_presentation_batches_refuse_before_crossing_the_fixed_key_ceiling() {
    let bus = Bus::start(Journal::in_memory().expect("in-memory authority"))
        .expect("dedicated writer starts");
    let admitted = bus
        .reader()
        .steering_presentations(vec![
            "unresolved".to_owned();
            bus::MAX_STEERING_PRESENTATION_KEYS
        ])
        .expect("the exact presentation ceiling is admitted");
    assert!(admitted.is_empty());
    let error = bus
        .reader()
        .steering_presentations(vec![
            "unresolved".to_owned();
            bus::MAX_STEERING_PRESENTATION_KEYS + 1
        ])
        .expect_err("oversized presentation batch refuses");
    assert!(
        error
            .to_string()
            .contains("steering presentation batch has 65 keys, maximum is 64")
    );
    bus.shutdown().expect("writer shuts down");
}

#[test]
fn one_writer_commits_then_broadcasts_monotone_cursors() {
    let caller = thread::current().id();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let bus = Bus::start(Journal::in_memory().expect("in-memory authority"))
        .expect("dedicated writer starts");
    let commits = bus.subscribe().expect("subscription registers");
    let empty = bus
        .task_snapshot()
        .expect("empty task snapshot initializes");
    assert!(empty.facts().is_empty());
    assert!(empty.tasks().is_empty());

    let first = committed(
        bus.commit(initial_write(uuid(COMMAND_ONE), Arc::clone(&observed)))
            .expect("writer answers"),
    );
    assert_eq!((first.first(), first.last()), (1, 1));
    assert_eq!(first.cursor().position().map(Seq::get), Some(1));
    assert_eq!(commits.recv().expect("post-durability signal"), first);
    let tasks = bus.task_snapshot().expect("incremental task head reads");
    assert_eq!(tasks.facts().len(), 1);
    assert_eq!(tasks.tasks().len(), 1);
    assert_eq!(tasks.tasks()[0].statement().body().born, stamp());
    assert_eq!(
        bus.fact_snapshot().expect("durable snapshot").facts().len(),
        1
    );

    let second = committed(
        bus.commit(note_write(uuid(COMMAND_TWO), 1, Arc::clone(&observed)))
            .expect("writer answers"),
    );
    assert_eq!((second.first(), second.last()), (2, 2));
    assert!(second.cursor() > first.cursor());
    assert_eq!(commits.recv().expect("second durable signal"), second);

    let verification = bus.verify().expect("authority verifies");
    assert_eq!(verification.facts, 2);
    assert_eq!(verification.receipts, 2);
    assert_eq!(verification.heads, 2);
    assert_eq!(verification.threads, 2);

    let writer_threads = observed.lock().expect("thread observations");
    assert_eq!(writer_threads.len(), 2);
    assert!(writer_threads.iter().all(|thread| *thread != caller));
    assert!(
        writer_threads
            .iter()
            .all(|thread| *thread == writer_threads[0])
    );
    drop(writer_threads);
    bus.shutdown().expect("writer shuts down");
}

#[test]
fn trust_only_commits_refresh_immutable_task_snapshots_at_the_commit_cursor() {
    let bus = Bus::start(Journal::in_memory().expect("in-memory authority"))
        .expect("dedicated writer starts");
    let digest: Digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest");
    let mut pended = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("origin"),
        instant("2030-04-12T18:44:09.271Z"),
        Producer::CommandDoor,
    );
    let fact = pended
        .fact(
            body::TrustPended {
                surface: "spice".to_owned(),
                digest: digest.clone(),
                worktree: Some("/producer/worktree".to_owned()),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("trust pend fact builds");
    let head_digest = digest.clone();
    pended.trust_head(fact, move |_| TrustHead {
        digest: head_digest,
        state: TrustState::Pended,
        worktree: Some("/producer/worktree".to_owned()),
    });
    append_trust_fleet(&mut pended);
    let first = committed(bus.commit(pended).expect("trust pend commits"));
    let before = bus.task_snapshot().expect("pended snapshot reads");
    assert_eq!(before.cursor(), first.cursor().position());
    assert_eq!(before.trust().len(), 300);
    assert!(before.materialization_steps() >= 5);
    let before_target = trust_head(&before, &digest);
    assert_eq!(before_target.body().state, TrustState::Pended);
    assert_eq!(before.facts().latest_trust_digest("spice"), Some(&digest));
    let requirements = EnvironmentRequirements::one(
        journal::crossing::EnvironmentRequirement::trust_grant(digest.clone()),
    );
    assert!(!requirements.are_satisfied_by(&before.environment()));

    let mut granted = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("origin"),
        instant("2030-04-12T18:44:10.271Z"),
        Producer::CommandDoor,
    );
    let fact = granted
        .fact(
            body::TrustGranted {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Existing(before_target.position()),
        )
        .expect("trust grant fact builds");
    let worktree = before_target.body().worktree.clone();
    let granted_digest = digest.clone();
    granted.trust_head(fact, move |_| TrustHead {
        digest: granted_digest,
        state: TrustState::Granted,
        worktree,
    });
    let second = committed(bus.commit(granted).expect("trust grant commits"));
    let after = bus.task_snapshot().expect("granted snapshot reads");
    assert_eq!(after.cursor(), second.cursor().position());
    let after_target = trust_head(&after, &digest);
    assert_eq!(after_target.body().state, TrustState::Granted);
    assert_eq!(after.facts().latest_trust_digest("spice"), Some(&digest));
    assert!(requirements.are_satisfied_by(&after.environment()));
    assert_eq!(
        before_target.body().state,
        TrustState::Pended,
        "the earlier snapshot remains immutable"
    );
    bus.shutdown().expect("writer shuts down");
}

#[test]
fn multi_page_task_snapshots_resume_then_decode_only_the_committed_tail() {
    const RETAINED_FACTS: usize = 300;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut journal = Journal::in_memory().expect("in-memory authority");
    journal
        .apply(initial_write(uuid(COMMAND_ONE), Arc::clone(&observed)))
        .expect("initial task commits");
    let mut bulk = JournalWrite::new(
        Actor::Operator,
        "task:BUS-a1".parse().expect("origin"),
        instant("2030-04-12T18:44:10.271Z"),
        Producer::CommandDoor,
    );
    let mut cause = FactCause::Existing(Seq::new(1).expect("initial cause"));
    for index in 1..RETAINED_FACTS {
        cause = FactCause::Pending(append_retained_task(&mut bulk, index, cause));
    }
    let applied = journal.apply(bulk).expect("fleet history commits");
    assert_eq!(applied.last().get(), RETAINED_FACTS as u64);

    let bus = Bus::start(journal).expect("dedicated writer starts");
    let cold = bus.fact_snapshot().expect("cold fact snapshot reads");
    let before = bus.task_snapshot().expect("multi-page snapshot settles");
    let retained = bus.fact_snapshot().expect("retained fact snapshot reads");
    assert_eq!(retained, cold);
    assert_eq!(before.facts().len(), RETAINED_FACTS);
    assert_eq!(before.tasks().len(), RETAINED_FACTS);
    assert!(
        before.materialization_steps() > 1,
        "retained history was drained inside one materialization step"
    );
    let mut tail = JournalWrite::new(
        Actor::Operator,
        "task:BUS-a1".parse().expect("origin"),
        instant("2030-04-12T18:44:11.271Z"),
        Producer::CommandDoor,
    );
    tail.fact(
        body::TaskNoted {
            task: stamp(),
            note: "one proportional tail".to_owned(),
        },
        FactCause::Existing(applied.last()),
    )
    .expect("tail fact builds");
    committed(bus.commit(tail).expect("tail commit answers"));

    let after = bus.task_snapshot().expect("advanced snapshot reads");
    let current_facts = bus.fact_snapshot().expect("advanced fact snapshot reads");
    assert_eq!(current_facts.horizon(), after.cursor());
    assert_eq!(
        current_facts.facts().iter().collect::<Vec<_>>(),
        after.facts().iter().collect::<Vec<_>>()
    );
    assert_eq!(retained.horizon(), before.cursor());
    assert_eq!(retained.facts().len(), RETAINED_FACTS);
    assert_eq!(after.facts().len(), RETAINED_FACTS + 1);
    assert_eq!(
        after.facts().horizon().map(Seq::get),
        Some((RETAINED_FACTS + 1) as u64)
    );
    assert_eq!(
        after.materialization_steps(),
        4,
        "one fact, two task-head pages, and one trust-head page complete the generation"
    );
    let committed_tail = after.facts().after(before.cursor()).collect::<Vec<_>>();
    assert_eq!(
        committed_tail.len(),
        1,
        "a caught-up lifecycle consumer touches only the one-fact suffix"
    );
    assert_eq!(committed_tail[0].seq.get(), (RETAINED_FACTS + 1) as u64);
    assert_shared_fact_prefix(&before, &after);
    assert!(
        std::ptr::eq(
            before
                .facts()
                .at(Seq::new(1).expect("first"))
                .expect("first fact is retained"),
            after
                .facts()
                .at(Seq::new(1).expect("first"))
                .expect("first fact stays retained"),
        ),
        "a position inside the shared prefix is answered from the shared chunk"
    );
    assert!(
        std::ptr::eq(
            after.facts().at(applied.last()).expect("last bulk fact"),
            before
                .facts()
                .iter()
                .last()
                .expect("bulk history is nonempty"),
        ),
        "the last position of a shared chunk is answered from that chunk"
    );
    assert_eq!(
        after
            .facts()
            .at(committed_tail[0].seq)
            .and_then(|fact| fact.fact.read::<body::TaskNoted>())
            .map(|note| note.note),
        Some("one proportional tail".to_owned()),
        "a position in the newest chunk is answered from that chunk"
    );
    assert!(
        after
            .facts()
            .at(Seq::new((RETAINED_FACTS + 2) as u64).expect("beyond"))
            .is_none(),
        "a position past the horizon is not retained"
    );
    bus.shutdown().expect("writer shuts down");
}

#[test]
fn refusals_and_faults_leave_every_durable_effect_and_signal_absent() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let bus = Bus::start(Journal::in_memory().expect("in-memory authority"))
        .expect("dedicated writer starts");
    let commits = bus.subscribe().expect("subscription registers");

    committed(
        bus.commit(initial_write(uuid(COMMAND_ONE), Arc::clone(&observed)))
            .expect("first commit answers"),
    );
    commits.recv().expect("first signal");

    let mut refused = note_write(uuid(COMMAND_TWO), 1, Arc::clone(&observed));
    refused.guard(JournalGuard::new(|read| {
        if read.cursor()?.is_none() {
            Ok(())
        } else {
            Err(GuardError::refused("expected an empty authority"))
        }
    }));
    match bus.commit(refused).expect("refusal answers") {
        CommitOutcome::Refused(refusal) => {
            assert_eq!(refusal.reason(), "expected an empty authority");
        }
        other => panic!("expected refusal, found {other:?}"),
    }
    assert!(matches!(
        commits.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        bus.fact_snapshot()
            .expect("snapshot after refusal")
            .facts()
            .len(),
        1
    );

    match bus
        .commit(cross_origin_write())
        .expect("causal refusal answers")
    {
        CommitOutcome::Refused(refusal) => {
            assert_eq!(
                refusal.reason(),
                "a fact cause cannot cross origin boundaries"
            );
        }
        other => panic!("expected causal refusal, found {other:?}"),
    }
    assert!(matches!(
        commits.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        bus.fact_snapshot()
            .expect("snapshot after causal refusal")
            .facts()
            .len(),
        1
    );

    let faulted = note_write(uuid(COMMAND_ONE), 1, Arc::clone(&observed));
    match bus.commit(faulted).expect("fault answers") {
        CommitOutcome::Fault(fault) => {
            assert!(fault.detail().contains("UNIQUE constraint failed"));
        }
        other => panic!("expected fault, found {other:?}"),
    }
    assert!(matches!(
        commits.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        bus.fact_snapshot()
            .expect("snapshot after fault")
            .facts()
            .len(),
        1
    );
    let verification = bus.verify().expect("rolled-back authority verifies");
    assert_eq!(verification.facts, 1);
    assert_eq!(verification.receipts, 1);
    assert_eq!(verification.heads, 2);
    bus.shutdown().expect("writer shuts down");
}

#[test]
fn command_receipts_refuse_body_and_answer_shapes_from_another_binding() {
    let task = task();
    let mut write = bare_write();
    let wrong_body = write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(COMMAND_ONE),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: CommandKind::TaskAdd,
                expected: vec![Witness::Absence { task: stamp() }],
                body: intent::TaskNote {
                    task: task.clone(),
                    note: "not an add body".to_owned(),
                },
            },
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            result(&task, "todo"),
        )
        .expect_err("task::add must refuse a task::note body");
    assert!(
        wrong_body
            .to_string()
            .contains("journal proposal is not current wire")
    );

    let wrong_answer = write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(COMMAND_ONE),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: CommandKind::TaskAdd,
                expected: vec![Witness::Absence { task: stamp() }],
                body: intent::TaskAdd {
                    title: "Implement bus writer".to_owned(),
                    project: Some("bus".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: EnvironmentRequirements::default(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                },
            },
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            BTreeMap::from([("unexpected".to_owned(), "answer".to_owned())]),
        )
        .expect_err("task::add must refuse another answer shape");
    assert!(
        wrong_answer
            .to_string()
            .contains("journal proposal is not current wire")
    );
}

fn append_trust_fleet(write: &mut JournalWrite) {
    for index in 1..300_u16 {
        let digest: Digest = format!("{index:064x}").parse().expect("fleet digest");
        let fact = write
            .fact(
                body::TrustPended {
                    surface: format!("fleet-{index}"),
                    digest: digest.clone(),
                    worktree: Some("/producer/worktree".to_owned()),
                    previous: None,
                },
                FactCause::Root,
            )
            .expect("fleet trust fact builds");
        write.trust_head(fact, move |_| TrustHead {
            digest,
            state: TrustState::Pended,
            worktree: Some("/producer/worktree".to_owned()),
        });
    }
}

fn trust_head<'a>(
    snapshot: &'a bus::TaskSnapshot,
    digest: &Digest,
) -> &'a journal::JournalHead<TrustHead> {
    snapshot
        .trust()
        .iter()
        .find(|head| &head.body().digest == digest)
        .expect("target trust head remains present")
}

fn assert_shared_fact_prefix(before: &bus::TaskSnapshot, after: &bus::TaskSnapshot) {
    assert!(
        std::ptr::eq(
            before.facts().iter().next().expect("history is nonempty"),
            after
                .facts()
                .iter()
                .next()
                .expect("history remains nonempty")
        ),
        "the one-fact commit rebuilt rather than shared retained facts"
    );
    assert_eq!(
        after
            .facts()
            .iter()
            .last()
            .and_then(|fact| fact.fact.read::<body::TaskNoted>())
            .map(|note| note.note),
        Some("one proportional tail".to_owned())
    );
}

fn append_retained_task(
    write: &mut JournalWrite,
    index: usize,
    cause: FactCause,
) -> journal::PendingFact {
    let task = journal::crossing::TaskStamp::for_micros(
        i64::try_from(index + 1_000).expect("fleet moment"),
    )
    .expect("fleet stamp");
    let task_phase = phase("todo");
    let pending = write
        .fact(
            retained_added(task.clone(), index, task_phase.clone()),
            cause,
        )
        .expect("fleet fact builds");
    let head_task = task.clone();
    write.task_head(pending, move |seq| {
        retained_task_head(head_task, index, seq)
    });
    write.workflow_head(task, pending, move |_| WorkflowHead {
        flow: vec![task_phase.clone()],
        phase: task_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    pending
}

fn retained_added(
    task: journal::crossing::TaskStamp,
    index: usize,
    phase: Phase,
) -> body::TaskAdded {
    body::TaskAdded {
        task,
        title: format!("Retained task {index}"),
        description: None,
        acceptance: Vec::new(),
        requirements: EnvironmentRequirements::default(),
        project: "bus".to_owned(),
        tags: Vec::new(),
        depends: Vec::new(),
        flow: vec![phase.clone()],
        phase,
        revision: u32::try_from(index + 1).expect("fleet revision"),
        creation: body::TaskCreationContext::unavailable(
            body::TaskCreationUnavailable::OperatorActor,
        ),
    }
}

fn retained_task_head(task: journal::crossing::TaskStamp, index: usize, seq: Seq) -> TaskHead {
    TaskHead {
        born: task,
        queue_origin: seq,
        title: format!("Retained task {index}"),
        description: None,
        acceptance: Vec::new(),
        requirements: EnvironmentRequirements::default(),
        project: "bus".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:BUS-a1".parse().expect("fleet origin"),
        creation: body::TaskCreationContext::unavailable(
            body::TaskCreationUnavailable::OperatorActor,
        ),
        revision: seq,
    }
}

fn initial_write(command_id: CanonicalUuid, observed: Arc<Mutex<Vec<ThreadId>>>) -> JournalWrite {
    let task = task();
    let at = instant("2030-04-12T18:44:09.271Z");
    let origin: Origin = "task:BUS-a1".parse().expect("origin");
    let phase = phase("todo");
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    write.guard(JournalGuard::new(move |read| {
        observed
            .lock()
            .expect("thread observations")
            .push(thread::current().id());
        if read.cursor()?.is_some() {
            return Err(GuardError::refused(
                "task creation requires an empty fixture",
            ));
        }
        Ok(())
    }));
    let fact = write
        .fact(
            body::TaskAdded {
                task: stamp(),
                title: "Implement bus writer".to_owned(),
                description: Some("Persist one ordered write".to_owned()),
                acceptance: vec!["The journal commits atomically".to_owned()],
                requirements: EnvironmentRequirements::default(),
                project: "bus".to_owned(),
                tags: vec!["journal".to_owned()],
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::OperatorActor,
                ),
            },
            FactCause::Root,
        )
        .expect("registered command-door fact");

    let task_for_head = stamp();
    let origin_for_head = origin.clone();
    write.task_head(fact, move |seq| TaskHead {
        born: task_for_head,
        queue_origin: seq,
        title: "Implement bus writer".to_owned(),
        description: Some("Persist one ordered write".to_owned()),
        acceptance: vec!["The journal commits atomically".to_owned()],
        requirements: EnvironmentRequirements::default(),
        project: "bus".to_owned(),
        tags: vec!["journal".to_owned()],
        dependencies: Vec::new(),
        provenance: origin_for_head,
        creation: body::TaskCreationContext::unavailable(
            body::TaskCreationUnavailable::OperatorActor,
        ),
        revision: seq,
    });
    let phase_for_head = phase.clone();
    write.workflow_head(stamp(), fact, move |_| WorkflowHead {
        flow: vec![phase_for_head.clone()],
        phase: phase_for_head,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });

    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: command_id,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: CommandKind::TaskAdd,
                expected: vec![Witness::Absence { task: stamp() }],
                body: intent::TaskAdd {
                    title: "Implement bus writer".to_owned(),
                    project: Some("bus".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: EnvironmentRequirements::default(),
                    origin: Some(origin),
                    flow: None,
                    depends: Vec::new(),
                },
            },
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            result(&task, "todo"),
        )
        .expect("canonical command result");
    write
}

fn note_write(
    command_id: CanonicalUuid,
    expected_cursor: u64,
    observed: Arc<Mutex<Vec<ThreadId>>>,
) -> JournalWrite {
    let task = task();
    let at = instant("2030-04-12T18:44:10.271Z");
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:BUS-a1".parse().expect("origin"),
        at,
        Producer::CommandDoor,
    );
    let guarded_task = stamp();
    write.guard(JournalGuard::new(move |read| {
        observed
            .lock()
            .expect("thread observations")
            .push(thread::current().id());
        let expected = Seq::new(expected_cursor).expect("expected cursor");
        let task_head = read.task_head(&guarded_task)?;
        let workflow_head = read.workflow_head(&guarded_task)?;
        if read.cursor()? != Some(expected)
            || task_head.as_ref().map(journal::JournalHead::position) != Some(expected)
            || workflow_head.as_ref().map(journal::JournalHead::position) != Some(expected)
        {
            return Err(GuardError::refused("task note witnesses changed"));
        }
        Ok(())
    }));
    write
        .fact(
            body::TaskNoted {
                task: stamp(),
                note: "writer observation".to_owned(),
            },
            FactCause::Existing(Seq::new(expected_cursor).expect("existing cause")),
        )
        .expect("registered command-door fact");
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: command_id,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: CommandKind::TaskNote,
                expected: vec![Witness::Existence { task: stamp() }],
                body: intent::TaskNote {
                    task: task.clone(),
                    note: "writer observation".to_owned(),
                },
            },
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            result(&task, "noted"),
        )
        .expect("canonical command result");
    write
}

fn cross_origin_write() -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:OTHER-a1".parse().expect("other origin"),
        instant("2030-04-12T18:44:10.271Z"),
        Producer::CommandDoor,
    );
    write
        .fact(
            body::TaskNoted {
                task: stamp(),
                note: "invalid cross-origin cause".to_owned(),
            },
            FactCause::Existing(Seq::new(1).expect("existing cause")),
        )
        .expect("registered command-door fact");
    write
}

fn bare_write() -> JournalWrite {
    JournalWrite::new(
        Actor::Operator,
        "task:BUS-a1".parse().expect("origin"),
        instant("2030-04-12T18:44:09.271Z"),
        Producer::CommandDoor,
    )
}

fn committed(outcome: CommitOutcome) -> bus::JournalCommit {
    match outcome {
        CommitOutcome::Committed(commit) => commit,
        other => panic!("expected commit, found {other:?}"),
    }
}

fn result(task: &TaskHandle, status: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("status".to_owned(), status.to_owned()),
        ("task".to_owned(), task.as_str().to_owned()),
    ])
}

fn task() -> TaskHandle {
    TaskHandle::for_project("bus", &stamp())
}

fn stamp() -> journal::crossing::TaskStamp {
    "1kKRRsDP".parse().expect("task stamp")
}

fn phase(value: &str) -> Phase {
    value.parse().expect("phase")
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical uuid")
}

fn instant(value: &str) -> journal::crossing::Instant {
    value.parse().expect("canonical instant")
}

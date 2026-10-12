use wire::rooms::{
    CandidateCoverage, CandidateCovered, CandidateCritique, CandidateCritiqued, CandidateRef,
    CandidateReference, CandidateRetained, CoverageAuthority, CoverageRef, CritiqueRef,
    CritiqueStanding, DecisionOutcome, DecisionRef, ExecutionAuthorization, ExecutionAuthorized,
    NegativeDecision, OutputControlled, PreparationDecided, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomAuthorityStamp, RoomPublicationContent, RoomPublished, RoomSource,
    WorkingGroup, WorkingGroupRemoved, WorkingGroupStored,
};
use wire::{Producer, SequenceCode, TaskPreparation};

use super::*;
use crate::guard::Applied;
use crate::{CommitError, FactCause, JournalHead, JournalWrite, ProposalAuthority};

type Commit = Result<Applied, CommitError>;

fn write() -> JournalWrite {
    JournalWrite::new(
        Actor::Operator,
        "task:rooms".parse().expect("room origin"),
        instant(),
        Producer::CommandDoor,
    )
}

fn commit<F: wire::Body>(journal: &mut Journal, body: F) -> Commit {
    let mut write = write();
    write.fact(body, FactCause::Root).expect("room fact builds");
    journal.apply(write)
}

fn cursor(journal: &Journal) -> Option<Seq> {
    journal.read(|read| read.cursor()).expect("cursor reads")
}

fn refused(journal: &Journal, result: Commit) {
    let before = cursor(journal);
    assert!(
        matches!(result, Err(CommitError::Refused(_))),
        "unsound authority must refuse: {result:?}"
    );
    assert_eq!(cursor(journal), before);
}

fn task(journal: &mut Journal, offset: i64, preparation: Option<TaskPreparation>) -> TaskStamp {
    let task = TaskStamp::for_micros(20_000 + offset).expect("task stamp");
    let flow = vec![phase("todo"), phase("review")];
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskAdded {
                preparation: preparation.clone(),
                intent: None,
                task: task.clone(),
                title: format!("Room task {offset}"),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "rooms".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: phase("todo"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact builds");
    let born = task.clone();
    write.task_head(fact, move |revision| TaskHead {
        preparation,
        intent: None,
        born,
        queue_origin: revision,
        title: format!("Room task {offset}"),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "rooms".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:rooms".parse().expect("provenance"),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let workflow = task.clone();
    write.workflow_head(workflow, fact, move |_| WorkflowHead {
        flow,
        phase: phase("todo"),
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
    journal.apply(write).expect("task commits");
    task
}

fn statement(journal: &Journal, task: &TaskStamp) -> TaskHead {
    journal
        .read(|read| read.task_head(task))
        .expect("task head reads")
        .expect("task exists")
        .into_body()
}

/// Adopt `output` with a task head carrying `head_preparation`, which a sound
/// write makes equal to the controlled preparation.
fn adopt(
    journal: &mut Journal,
    output: &TaskStamp,
    preparation: TaskPreparation,
    head_preparation: TaskPreparation,
) -> Commit {
    let head = statement(journal, output);
    let mut write = write();
    let fact = write
        .fact(
            OutputControlled {
                output: output.clone(),
                preparation,
            },
            FactCause::Root,
        )
        .expect("control fact builds");
    write.task_head(fact, move |revision| TaskHead {
        preparation: Some(head_preparation),
        revision,
        ..head
    });
    journal.apply(write)
}

fn edit_project(journal: &mut Journal, task: &TaskStamp, project: &str) {
    let head = statement(journal, task);
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: 2,
                dependencies: None,
                intent: None,
                notice: None,
            },
            FactCause::Root,
        )
        .expect("edit fact builds");
    let project = project.to_owned();
    write.task_head(fact, move |revision| TaskHead {
        project,
        revision,
        ..head
    });
    journal.apply(write).expect("re-homing commits");
}

fn authority(micros: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(1_760_000_000_000_000 + micros).expect("authority stamp")
}

fn name(text: &str) -> wire::Name {
    text.parse().expect("name")
}

fn journal_source(fact: u64) -> RoomSource {
    RoomSource::Journal {
        fact: SequenceCode::new(fact),
    }
}

fn ingest(
    journal: &mut Journal,
    head: TranscriptSourceHead,
    from: TranscriptPosition,
    supersedes: Option<CanonicalUuid>,
) {
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("lane:{}", head.lane)
            .parse()
            .expect("resident origin"),
        instant(),
        Producer::ResidentRecognition,
    );
    let body = wire::body::JournalIngested {
        lane: head.lane,
        vendor: head.vendor.clone(),
        stream: head.stream.clone(),
        source: head.source,
        supersedes,
        from,
        to: head.next,
        usage: Vec::new(),
        non_usage_records: 1,
        residue_records: 0,
    };
    let pending = write
        .fact(body, FactCause::Root)
        .expect("transcript ingestion builds");
    write.transcript_source_head(pending, move |_| head);
    journal.apply(write).expect("transcript ingestion commits");
}

fn prepared_by(controller: &TaskStamp) -> TaskPreparation {
    TaskPreparation {
        parent: controller.clone(),
        controller: controller.clone(),
        construction: controller.clone(),
    }
}

/// One controller with a born output, an adoptable task, and a governed input.
struct Preparation {
    controller: TaskStamp,
    output: TaskStamp,
    spare: TaskStamp,
    input: TaskStamp,
}

impl Preparation {
    fn new(journal: &mut Journal) -> Self {
        let controller = task(journal, 1, None);
        let output = task(journal, 2, Some(prepared_by(&controller)));
        let spare = task(journal, 3, None);
        let input = task(journal, 4, None);
        Self {
            controller,
            output,
            spare,
            input,
        }
    }

    fn candidate(&self, micros: i64) -> CandidateRef {
        CandidateRef {
            controller: self.controller.clone(),
            candidate: authority(micros),
        }
    }

    fn retained(&self, micros: i64, outputs: Vec<TaskStamp>) -> CandidateRetained {
        CandidateRetained {
            candidate: RetainedCandidate {
                candidate: self.candidate(micros),
                governed_inputs: vec![self.input.clone()],
                outputs,
                references: vec![CandidateReference {
                    relation: name("contract"),
                    source: journal_source(1),
                }],
            },
        }
    }

    fn coverage(&self, micros: i64, sequence: u64) -> CandidateCovered {
        CandidateCovered {
            coverage: CandidateCoverage {
                coverage: CoverageRef {
                    candidate: self.candidate(micros),
                    coverage: SequenceCode::new(sequence),
                },
                authority: CoverageAuthority::Operator {},
                scopes: vec![name("contracts")],
                source: journal_source(1),
            },
        }
    }

    fn decision(&self, sequence: u64, outcome: DecisionOutcome) -> PreparationDecided {
        PreparationDecided {
            decision: PreparationDecision {
                decision: self.decision_ref(sequence),
                source: journal_source(1),
                outcome,
            },
        }
    }

    fn decision_ref(&self, sequence: u64) -> DecisionRef {
        DecisionRef {
            controller: self.controller.clone(),
            decision: SequenceCode::new(sequence),
        }
    }

    fn authorization(
        &self,
        decision: u64,
        micros: i64,
        inputs: Vec<TaskStamp>,
        outputs: Vec<TaskStamp>,
    ) -> ExecutionAuthorized {
        ExecutionAuthorized {
            authorization: ExecutionAuthorization {
                decision: self.decision_ref(decision),
                candidate: self.candidate(micros),
                governed_inputs: inputs,
                selected_outputs: outputs,
                coverage: vec![CoverageRef {
                    candidate: self.candidate(micros),
                    coverage: SequenceCode::new(1),
                }],
            },
        }
    }
}

fn publication(room: &TaskStamp, micros: i64, content: RoomPublicationContent) -> RoomPublished {
    RoomPublished {
        room: room.clone(),
        publication: authority(micros),
        content,
    }
}

fn group(micros: i64, anchors: Vec<TaskStamp>) -> WorkingGroupStored {
    WorkingGroupStored {
        group: WorkingGroup {
            group: authority(micros),
            name: "Rooms".to_owned(),
            anchors,
        },
    }
}

fn projection(journal: &Journal) -> crate::rooms::Projection {
    crate::rooms::Projection::load(&journal.connection).expect("projection reads")
}

fn fact_bytes(journal: &Journal) -> Vec<Vec<u8>> {
    let mut statement = journal
        .connection
        .prepare("SELECT envelope FROM fact_envelopes ORDER BY seq")
        .expect("prepare fact bytes");
    statement
        .query_map([], |row| row.get(0))
        .expect("query fact bytes")
        .map(|row| row.expect("fact bytes"))
        .collect()
}

/// Accepted preparation through exact authorization, positioned for further
/// adversarial writes.
fn authorized(journal: &mut Journal) -> Preparation {
    let preparation = Preparation::new(journal);
    commit(
        journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(journal, preparation.coverage(1, 1)).expect("coverage recorded");
    commit(
        journal,
        preparation.decision(
            1,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("decision recorded");
    commit(
        journal,
        preparation.authorization(
            1,
            1,
            vec![preparation.input.clone()],
            vec![preparation.output.clone()],
        ),
    )
    .expect("exact selection authorized");
    preparation
}

#[test]
fn one_write_maintains_the_projection_that_rebuild_reproduces_without_touching_facts() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = authorized(&mut journal);
    let room = preparation.controller.clone();
    commit(
        &mut journal,
        publication(
            &room,
            1,
            RoomPublicationContent::Authored {
                text: "first".to_owned(),
            },
        ),
    )
    .expect("authored publication");
    commit(
        &mut journal,
        publication(
            &room,
            2,
            RoomPublicationContent::Linked {
                link: journal_source(1),
            },
        ),
    )
    .expect("linked publication");
    commit(
        &mut journal,
        CandidateCritiqued {
            critique: CandidateCritique {
                critique: CritiqueRef {
                    candidate: preparation.candidate(1),
                    critique: SequenceCode::new(1),
                },
                category: name("composition"),
                source: journal_source(2),
                standing: CritiqueStanding::Clean,
            },
        },
    )
    .expect("critique recorded");
    journal.verify().expect("committed projection verifies");

    let (decision, granted, page) = journal
        .read(|read| {
            let decision = read.proposal_head(&ProposalRef::Decision {
                decision: preparation.decision_ref(1),
            })?;
            let granted = read.authorization_head(&preparation.output)?;
            let page = read.room_publications(&room, None, fact_budget(1))?;
            Ok((decision, granted, page))
        })
        .expect("room authority reads");
    assert!(matches!(
        decision.map(JournalHead::into_body),
        Some(ProposalAuthority::Decision(PreparationDecision {
            outcome: DecisionOutcome::Accepted { .. },
            ..
        }))
    ));
    assert_eq!(
        granted
            .expect("output is authorized")
            .body()
            .selected_outputs,
        vec![preparation.output.clone()]
    );
    assert_eq!(page.rows.len(), 1, "the budget bounds the page");
    let rest = journal
        .read(|read| read.room_publications(&room, page.continuation, fact_budget(8)))
        .expect("continued publication page");
    assert_eq!(rest.rows.len(), 1);
    assert_eq!(rest.continuation, None);
    assert!(rest.rows[0].seq > page.rows[0].seq);

    let derived = projection(&journal);
    let facts = fact_bytes(&journal);
    journal
        .connection
        .execute_batch(
            "DELETE FROM authorization_heads;
             UPDATE proposal_heads SET head_seq = 1 WHERE head_seq = (SELECT max(head_seq) FROM proposal_heads);
             INSERT INTO room_publications(room, publication, seq) VALUES ('ghost', 'ghost', 2);",
        )
        .expect("corrupt the projection");
    assert!(refused_classes(&journal).contains(&ViolationClass::Head));

    journal.reindex().expect("projection rebuilds from facts");
    assert_eq!(projection(&journal), derived);
    assert_eq!(fact_bytes(&journal), facts, "rebuild never rewrites facts");
    journal.verify().expect("rebuilt projection verifies");
}

#[test]
fn duplicate_recordings_refuse_and_a_duplicated_history_fails_verification() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = authorized(&mut journal);
    let room = preparation.controller.clone();
    let post = publication(
        &room,
        1,
        RoomPublicationContent::Authored {
            text: "once".to_owned(),
        },
    );
    commit(&mut journal, post.clone()).expect("first publication");
    let result = commit(&mut journal, post.clone());
    refused(&journal, result);
    let result = commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    );
    refused(&journal, result);
    let result = commit(&mut journal, preparation.coverage(1, 1));
    refused(&journal, result);

    // Replaying one fact twice inside a single write refuses the whole write.
    let mut twice = write();
    let second = publication(
        &room,
        2,
        RoomPublicationContent::Authored {
            text: "twice".to_owned(),
        },
    );
    twice
        .fact(second.clone(), FactCause::Root)
        .expect("fact builds");
    twice.fact(second, FactCause::Root).expect("fact builds");
    let result = journal.apply(twice);
    refused(&journal, result);

    // History that bypassed the commit fold cannot verify.
    let seq = cursor(&journal).expect("nonempty").get() + 1;
    insert_fact(
        &journal,
        fact(
            seq,
            None,
            FactKind::RoomPublished,
            serde_json::to_value(&post).expect("publication body"),
        ),
    );
    let sql_seq = i64::try_from(seq).expect("seq");
    journal
        .connection
        .execute_batch(&format!(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES ({sql_seq}, 'task:1kKRRsDP', NULL, {sql_seq}, 0);
             UPDATE cursor_head SET seq = {sql_seq} WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = {sql_seq} WHERE singleton = 1;",
        ))
        .expect("index the duplicate");
    let violations = refused_violations(&journal);
    assert!(
        violations
            .iter()
            .any(|violation| violation.subject == format!("room_authority:{seq}")),
        "{violations:?}"
    );
    assert!(matches!(
        journal.reindex(),
        Err(crate::ReindexError::Violations(_))
    ));
}

#[test]
fn each_output_has_one_controller_and_rehoming_keeps_room_identity() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let other = task(&mut journal, 9, None);
    let control = prepared_by(&preparation.controller);

    // A head that disagrees with the controlled preparation refuses.
    let result = adopt(
        &mut journal,
        &preparation.spare,
        control.clone(),
        prepared_by(&other),
    );
    refused(&journal, result);
    // A task cannot control itself.
    let result = adopt(
        &mut journal,
        &preparation.controller,
        control.clone(),
        control.clone(),
    );
    refused(&journal, result);

    adopt(
        &mut journal,
        &preparation.spare,
        control.clone(),
        control.clone(),
    )
    .expect("adoption commits");
    // Neither an adopted nor a born output can move to another controller.
    for output in [&preparation.spare, &preparation.output] {
        let result = adopt(
            &mut journal,
            output,
            prepared_by(&other),
            prepared_by(&other),
        );
        refused(&journal, result);
    }
    commit(
        &mut journal,
        preparation.retained(
            1,
            vec![preparation.output.clone(), preparation.spare.clone()],
        ),
    )
    .expect("both controlled outputs join one candidate");
    let result = commit(
        &mut journal,
        CandidateRetained {
            candidate: RetainedCandidate {
                candidate: CandidateRef {
                    controller: other.clone(),
                    candidate: authority(2),
                },
                ..preparation
                    .retained(2, vec![preparation.output.clone()])
                    .candidate
            },
        },
    );
    refused(&journal, result);

    let room = preparation.output.clone();
    commit(
        &mut journal,
        publication(
            &room,
            1,
            RoomPublicationContent::Authored {
                text: "before".to_owned(),
            },
        ),
    )
    .expect("publication before re-homing");
    edit_project(&mut journal, &room, "elsewhere");
    commit(
        &mut journal,
        publication(
            &room,
            2,
            RoomPublicationContent::Authored {
                text: "after".to_owned(),
            },
        ),
    )
    .expect("publication after re-homing");
    let page = journal
        .read(|read| read.room_publications(&room, None, fact_budget(8)))
        .expect("room reads after re-homing");
    assert_eq!(page.rows.len(), 2, "the room is its anchor's birth stamp");
    journal
        .verify()
        .expect("controlled and re-homed authority verifies");
}

#[test]
fn authorization_selects_exactly_an_accepted_candidate() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    adopt(
        &mut journal,
        &preparation.spare,
        prepared_by(&preparation.controller),
        prepared_by(&preparation.controller),
    )
    .expect("spare output adopted");
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(&mut journal, preparation.coverage(1, 1)).expect("coverage");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Returned {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("returned decision");
    let input = vec![preparation.input.clone()];
    let output = vec![preparation.output.clone()];
    let result = commit(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);

    commit(
        &mut journal,
        preparation.decision(
            2,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("accepted decision");
    for inexact in [
        preparation.authorization(2, 1, input.clone(), vec![preparation.spare.clone()]),
        preparation.authorization(2, 1, Vec::new(), output.clone()),
        ExecutionAuthorized {
            authorization: ExecutionAuthorization {
                coverage: vec![CoverageRef {
                    candidate: preparation.candidate(1),
                    coverage: SequenceCode::new(9),
                }],
                ..preparation
                    .authorization(2, 1, input.clone(), output.clone())
                    .authorization
            },
        },
    ] {
        let result = commit(&mut journal, inexact);
        refused(&journal, result);
    }
    commit(
        &mut journal,
        preparation.authorization(2, 1, input.clone(), output.clone()),
    )
    .expect("exact selection authorized");

    // A sealed selection admits no second authorization of the same output.
    commit(
        &mut journal,
        preparation.decision(
            3,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("second acceptance");
    let result = commit(
        &mut journal,
        preparation.authorization(3, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);
    let result = commit(&mut journal, preparation.authorization(2, 1, input, output));
    refused(&journal, result);
    journal.verify().expect("exact authority verifies");
}

#[test]
fn negative_and_no_work_outcomes_authorize_nothing() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    for (sequence, conclusion) in [
        (1, NegativeDecision::Rejected),
        (2, NegativeDecision::NoWork),
    ] {
        commit(
            &mut journal,
            preparation.decision(sequence, DecisionOutcome::Negative { conclusion }),
        )
        .expect("negative decision needs no candidate");
        let result = commit(
            &mut journal,
            preparation.authorization(
                sequence,
                1,
                vec![preparation.input.clone()],
                vec![preparation.output.clone()],
            ),
        );
        refused(&journal, result);
    }
    let result = commit(
        &mut journal,
        preparation.decision(
            3,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    );
    refused(&journal, result);
    let granted = journal
        .read(|read| read.authorization_head(&preparation.output))
        .expect("authorization reads");
    assert_eq!(granted, None);
    journal.verify().expect("negative authority verifies");
}

#[test]
fn group_order_is_the_stored_order_and_removal_leaves_rooms_intact() {
    let mut journal = Journal::in_memory().expect("journal");
    let first = task(&mut journal, 1, None);
    let second = task(&mut journal, 2, None);
    let third = task(&mut journal, 3, None);
    let id = authority(7);
    let read_group = |journal: &Journal| {
        journal
            .read(|read| {
                Ok((
                    read.group_head(&id)?.map(|head| head.into_body().anchors),
                    read.anchor_group_page(&second, None, fact_budget(8))?.rows,
                    read.group_head_page(None, fact_budget(8))?.rows.len(),
                ))
            })
            .expect("group reads")
    };

    commit(
        &mut journal,
        group(7, vec![third.clone(), first.clone(), second.clone()]),
    )
    .expect("group stored");
    assert_eq!(
        read_group(&journal),
        (
            Some(vec![third.clone(), first.clone(), second.clone()]),
            vec![id.clone()],
            1
        )
    );
    commit(&mut journal, group(7, vec![first.clone(), third.clone()])).expect("group reordered");
    assert_eq!(
        read_group(&journal),
        (Some(vec![first.clone(), third.clone()]), Vec::new(), 1)
    );
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let result = commit(&mut journal, group(7, vec![first.clone(), absent]));
    refused(&journal, result);
    journal.verify().expect("ordered groups verify");

    commit(&mut journal, WorkingGroupRemoved { group: id.clone() }).expect("group removed");
    assert_eq!(read_group(&journal), (None, Vec::new(), 0));
    let result = commit(&mut journal, WorkingGroupRemoved { group: id.clone() });
    refused(&journal, result);
    assert!(
        journal
            .read(|read| read.task_head(&first))
            .expect("room reads")
            .is_some()
    );
    commit(&mut journal, group(7, vec![second.clone()])).expect("removed group recreates");
    journal.verify().expect("group history verifies");
}

#[test]
fn group_indexes_page_by_bytes_and_fanout_without_gaps() {
    let mut journal = Journal::in_memory().expect("journal");
    let anchor = task(&mut journal, 1, None);
    let groups = [1, 2, 3].map(authority);
    for micros in [1, 2, 3] {
        commit(&mut journal, group(micros, vec![anchor.clone()])).expect("group commits");
    }

    let row_limited = journal
        .read(|read| read.group_head_page(None, fact_budget(1)))
        .expect("row-limited group page");
    assert_eq!(row_limited.rows.len(), 1);
    assert_eq!(row_limited.continuation, Some(groups[0].clone()));
    let membership_limited = journal
        .read(|read| read.anchor_group_page(&anchor, None, fact_budget(1)))
        .expect("row-limited membership page");
    assert_eq!(membership_limited.rows, vec![groups[0].clone()]);
    assert_eq!(membership_limited.continuation, Some(groups[0].clone()));

    let group_bytes = serde_json::to_vec(&group(1, vec![anchor.clone()]).group)
        .expect("group encodes")
        .len();
    let group_budget = FactReadBudget::new(8, group_bytes).expect("one group body");
    let first = journal
        .read(|read| read.group_head_page(None, group_budget))
        .expect("first group page");
    assert_eq!(
        first
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[0].clone()]
    );
    assert_eq!(first.continuation, Some(groups[0].clone()));
    let second = journal
        .read(|read| read.group_head_page(first.continuation.as_ref(), group_budget))
        .expect("second group page");
    assert_eq!(
        second
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[1].clone()]
    );
    assert_eq!(second.continuation, Some(groups[1].clone()));
    let third = journal
        .read(|read| read.group_head_page(second.continuation.as_ref(), group_budget))
        .expect("third group page");
    assert_eq!(
        third
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[2].clone()]
    );
    assert_eq!(third.continuation, None);
    let too_small = FactReadBudget::new(8, group_bytes - 1).expect("smaller body budget");
    let indivisible = journal
        .read(|read| read.group_head_page(None, too_small))
        .expect("first indivisible group head advances the page");
    assert_eq!(indivisible.rows[0].body().group, groups[0]);
    assert_eq!(indivisible.continuation, Some(groups[0].clone()));

    let identity_bytes = groups[0].to_string().len();
    let anchor_budget = FactReadBudget::new(8, identity_bytes).expect("one identity");
    let first = journal
        .read(|read| read.anchor_group_page(&anchor, None, anchor_budget))
        .expect("first membership page");
    assert_eq!(first.rows, vec![groups[0].clone()]);
    assert_eq!(first.continuation, Some(groups[0].clone()));
    let second = journal
        .read(|read| read.anchor_group_page(&anchor, first.continuation.as_ref(), anchor_budget))
        .expect("second membership page");
    assert_eq!(second.rows, vec![groups[1].clone()]);
    assert_eq!(second.continuation, Some(groups[1].clone()));
    let third = journal
        .read(|read| read.anchor_group_page(&anchor, second.continuation.as_ref(), anchor_budget))
        .expect("third membership page");
    assert_eq!(third.rows, vec![groups[2].clone()]);
    assert_eq!(third.continuation, None);
    let too_small = FactReadBudget::new(8, identity_bytes - 1).expect("smaller identity budget");
    let indivisible = journal
        .read(|read| read.anchor_group_page(&anchor, None, too_small))
        .expect("first indivisible membership advances the page");
    assert_eq!(indivisible.rows, vec![groups[0].clone()]);
    assert_eq!(indivisible.continuation, Some(groups[0].clone()));
}

#[test]
fn links_must_name_existing_authority() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let room = preparation.controller.clone();
    let future = cursor(&journal).expect("nonempty").get() + 1;
    let transcript = RoomSource::Transcript {
        head: transcript_source_head(),
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
        },
    };
    for link in [journal_source(0), journal_source(future), transcript] {
        let result = commit(
            &mut journal,
            publication(&room, 1, RoomPublicationContent::Linked { link }),
        );
        refused(&journal, result);
    }
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let result = commit(
        &mut journal,
        publication(
            &absent,
            1,
            RoomPublicationContent::Authored {
                text: "nowhere".to_owned(),
            },
        ),
    );
    refused(&journal, result);
    let result = commit(&mut journal, preparation.coverage(1, 1));
    refused(&journal, result);
    let result = commit(
        &mut journal,
        preparation.retained(1, vec![preparation.spare.clone()]),
    );
    refused(&journal, result);
    commit(
        &mut journal,
        publication(
            &room,
            1,
            RoomPublicationContent::Linked {
                link: RoomSource::Artifact {
                    digest: digest("a"),
                },
            },
        ),
    )
    .expect("artifact bytes stay with their digest");
    journal.verify().expect("linked authority verifies");
}

#[test]
fn transcript_links_stay_within_the_accounted_generation_and_extent() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let head = transcript_source_head();
    ingest(
        &mut journal,
        head.clone(),
        TranscriptPosition {
            member: 1,
            offset: 0,
        },
        None,
    );
    let linked =
        |head: TranscriptSourceHead, locus: wire::MessageLocus| RoomPublicationContent::Linked {
            link: RoomSource::Transcript { head, locus },
        };
    let locus = |offset, length| wire::MessageLocus {
        member: 1,
        offset,
        length,
        event: 0,
    };

    commit(
        &mut journal,
        publication(&room, 20, linked(head.clone(), locus(4_095, 1))),
    )
    .expect("the accounted final byte is linkable");
    let mut earlier = head.clone();
    earlier.next.offset = 2_048;
    commit(
        &mut journal,
        publication(&room, 21, linked(earlier, locus(2_047, 1))),
    )
    .expect("an earlier boundary within the current generation is linkable");

    let mut future = head.clone();
    future.next.offset += 1;
    let mut wrong_generation = head.clone();
    wrong_generation.source = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    let mut wrong_stream = head.clone();
    wrong_stream.stream = "sidecar".to_owned();
    for (micros, source) in [
        (22, linked(future, locus(0, 1))),
        (23, linked(head.clone(), locus(4_095, 2))),
        (24, linked(head.clone(), locus(u64::MAX, 2))),
        (25, linked(wrong_generation, locus(0, 1))),
        (26, linked(wrong_stream, locus(0, 1))),
        (27, linked(head.clone(), locus(0, 0))),
    ] {
        let result = commit(&mut journal, publication(&room, micros, source));
        refused(&journal, result);
    }

    let mut advanced = head.clone();
    advanced.next.offset = 8_192;
    ingest(&mut journal, advanced, head.next, None);
    commit(
        &mut journal,
        publication(&room, 28, linked(head.clone(), locus(4_000, 96))),
    )
    .expect("a later head preserves links within the accounted generation");

    let mut replacement = head.clone();
    replacement.source = uuid("018f82d4-6f59-7d06-92e3-bce913116398");
    replacement.next.offset = 128;
    ingest(
        &mut journal,
        replacement,
        TranscriptPosition {
            member: 1,
            offset: 0,
        },
        Some(head.source),
    );
    let result = commit(
        &mut journal,
        publication(&room, 29, linked(head, locus(0, 1))),
    );
    refused(&journal, result);

    let expected = projection(&journal);
    journal.verify().expect("accounted links verify");
    journal.reindex().expect("accounted links rebuild");
    assert_eq!(projection(&journal), expected);
}

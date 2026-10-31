use wire::rooms::{
    CandidateCoverage, CandidateCovered, CandidateCritique, CandidateCritiqued, CandidateRef,
    CandidateReference, CoverageAuthority, CoverageRef, CritiqueRef, CritiqueStanding,
    DecisionOutcome, DecisionRef, DesignPassProposal, DesignProposed, ExecutionAuthorization,
    ExecutionAuthorized, OutputControlled, OutputDecision, OutputVerdict, PreparationDecided,
    PreparationDecision, ProposalRef, RoomAuthorityStamp, RoomPublicationContent, RoomPublished,
    RoomSource, TextRange, TranscriptHunk, WorkingGroup, WorkingGroupMoved, WorkingGroupPartition,
    WorkingGroupsArranged,
};
use wire::{Producer, SequenceCode, TaskPreparation};

use super::*;
use crate::guard::Applied;
use crate::{CommitError, FactCause, JournalHead, JournalWrite, ProposalAuthority};

type Commit = Result<Applied, CommitError>;

mod authorization;
mod groups;
mod hunks;
mod maturity;
mod withdrawal;

fn write() -> JournalWrite {
    write_as(Actor::Operator)
}

fn write_as(actor: Actor) -> JournalWrite {
    JournalWrite::new(
        actor,
        "task:rooms".parse().expect("room origin"),
        instant(),
        Producer::CommandDoor,
    )
}

fn commit<F: wire::Body>(journal: &mut Journal, body: F) -> Commit {
    commit_as(journal, Actor::Operator, body)
}

fn commit_as<F: wire::Body>(journal: &mut Journal, actor: Actor, body: F) -> Commit {
    let mut write = write_as(actor);
    write.fact(body, FactCause::Root).expect("room fact builds");
    journal.apply(write)
}

/// How an authorization write moves its controller's workflow.
#[derive(Clone, Copy)]
enum Conclusion {
    Completed,
    Closed,
    Untouched,
}

/// One write holding `body` that moves the controller per `conclusion` and,
/// when `outputs`, each existing selected output's statement to carry the
/// decision.
fn authorization_write(
    journal: &Journal,
    actor: Actor,
    body: ExecutionAuthorized,
    conclusion: Conclusion,
    outputs: bool,
) -> JournalWrite {
    let decision = body.authorization.decision.clone();
    let controller = body.authorization.candidate.controller.clone();
    let heads = body
        .authorization
        .selected_outputs
        .iter()
        .filter(|_| outputs)
        .filter_map(|output| journal.read(|read| read.task_head(output)).expect("reads"))
        .map(JournalHead::into_body)
        .collect::<Vec<_>>();
    let workflow = journal
        .read(|read| read.workflow_head(&controller))
        .expect("controller workflow reads")
        .expect("controller exists")
        .into_body();
    let mut write = write_as(actor);
    let fact = write
        .fact(body, FactCause::Root)
        .expect("authorization builds");
    if !matches!(conclusion, Conclusion::Untouched) {
        let status = match conclusion {
            Conclusion::Completed
                if matches!(workflow.status.review(), Some(wire::WorkflowReview::Clean)) =>
            {
                wire::WorkflowStatus::reviewed(instant())
            }
            Conclusion::Completed => wire::WorkflowStatus::completed(instant()),
            Conclusion::Closed => wire::WorkflowStatus::closed(instant()),
            Conclusion::Untouched => unreachable!("untouched workflow is not written"),
        };
        write.workflow_head(controller, fact, move |_| WorkflowHead {
            status,
            ..workflow
        });
    }
    for mut head in heads {
        let controller = decision.controller.clone();
        head.dependencies
            .retain(|dependency| dependency != &controller);
        write.task_head(fact, move |revision| TaskHead { revision, ..head });
    }
    write
}

/// Commit `body`, leaving its controller open and moving each existing
/// selected output's statement to carry the decision.
fn authorize(journal: &mut Journal, body: ExecutionAuthorized) -> Commit {
    let write = authorization_write(journal, Actor::Operator, body, Conclusion::Untouched, true);
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

fn design_task(journal: &mut Journal, offset: i64) -> TaskStamp {
    task_with_intent(journal, offset, None, Some(wire::TaskIntent::Design))
}

fn task(journal: &mut Journal, offset: i64, preparation: Option<TaskPreparation>) -> TaskStamp {
    task_with_intent(journal, offset, preparation, None)
}

fn task_with_intent(
    journal: &mut Journal,
    offset: i64,
    preparation: Option<TaskPreparation>,
    intent: Option<wire::TaskIntent>,
) -> TaskStamp {
    task_with_acceptance(journal, offset, preparation, intent, Vec::new())
}

/// A Design whose statement names `acceptance` as its question items.
fn design_with_acceptance(journal: &mut Journal, offset: i64, acceptance: &[&str]) -> TaskStamp {
    task_with_acceptance(
        journal,
        offset,
        None,
        Some(wire::TaskIntent::Design),
        acceptance.iter().map(|item| (*item).to_owned()).collect(),
    )
}

fn task_with_acceptance(
    journal: &mut Journal,
    offset: i64,
    preparation: Option<TaskPreparation>,
    intent: Option<wire::TaskIntent>,
    acceptance: Vec<String>,
) -> TaskStamp {
    let task = TaskStamp::for_micros(20_000 + offset).expect("task stamp");
    let current = if intent == Some(wire::TaskIntent::Design) {
        phase("design")
    } else {
        phase("todo")
    };
    let flow = if intent == Some(wire::TaskIntent::Design) {
        vec![current.clone()]
    } else {
        vec![current.clone(), phase("review")]
    };
    let dependencies = preparation
        .as_ref()
        .map(|preparation| vec![preparation.parent.clone()])
        .unwrap_or_default();
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskAdded {
                preparation: preparation.clone(),
                intent,
                task: task.clone(),
                title: format!("Room task {offset}"),
                pitch: "Rooms need work to talk about.".to_owned(),
                description: None,
                acceptance: acceptance.clone(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "rooms".to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                depends: dependencies.clone(),
                flow: flow.clone(),
                phase: current.clone(),
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
        intent,
        born,
        queue_origin: revision,
        title: format!("Room task {offset}"),
        pitch: "Rooms need work to talk about.".to_owned(),
        description: None,
        acceptance,
        requirements: wire::EnvironmentRequirements::default(),
        resources: wire::ResourceRequirements::default(),
        project: "rooms".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        dependencies,
        provenance: wire::TaskProvenance::Parentless,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let workflow = task.clone();
    write.workflow_head(workflow, fact, move |_| WorkflowHead {
        flow,
        phase: current,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
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

fn claim(journal: &mut Journal, task: &TaskStamp) -> Seq {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    let occupant = uuid("018f82d4-6f59-7d06-92e3-bce9131163d2");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163d3");
    let actor = Actor::Operator;
    let cell = wire::ClaimCell {
        project: "rooms".to_owned(),
        phase: phase("design"),
    };
    let mut write = write();
    let won = write
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: cell.clone(),
                until: instant(),
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("claim fact builds");
    write.claim_head(task.clone(), won, move |fact| wire::ClaimHead {
        actor,
        fact,
        claimed: fact,
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
        acknowledged: 0,
    });
    journal.apply(write).expect("claim commits").last()
}

/// Adopt `output` with a task head carrying `head_preparation`, which a sound
/// write makes equal to the controlled preparation.
fn adopt(
    journal: &mut Journal,
    output: &TaskStamp,
    preparation: TaskPreparation,
    head_preparation: TaskPreparation,
) -> Commit {
    let mut head = statement(journal, output);
    if !head.dependencies.contains(&head_preparation.parent) {
        head.dependencies.push(head_preparation.parent.clone());
    }
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

fn set_dependencies(journal: &mut Journal, task: &TaskStamp, dependencies: Vec<TaskStamp>) {
    let head = statement(journal, task);
    let revision = u32::try_from(head.revision.get() + 1).expect("fixture revision fits");
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision,
                dependencies: Some(dependencies.clone()),
                intent: None,
            },
            FactCause::Root,
        )
        .expect("dependency edit builds");
    write.task_head(fact, move |revision| TaskHead {
        dependencies,
        revision,
        ..head
    });
    journal.apply(write).expect("dependency edit commits");
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

/// Declare Plan intent on `task` through an ordinary statement edit.
fn declare_plan(journal: &mut Journal, task: &TaskStamp) -> Commit {
    let head = statement(journal, task);
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: 2,
                dependencies: None,
                intent: Some(wire::TaskIntentEdit::Set {
                    intent: wire::TaskIntent::Plan,
                }),
            },
            FactCause::Root,
        )
        .expect("intent edit fact builds");
    write.task_head(fact, move |revision| TaskHead {
        intent: Some(wire::TaskIntent::Plan),
        revision,
        ..head
    });
    journal.apply(write)
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

fn hunk(head: &TranscriptSourceHead) -> TranscriptHunk {
    TranscriptHunk {
        head: head.clone(),
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 64,
            event: 0,
        },
        text: TextRange { start: 0, end: 8 },
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
    statement: Seq,
    claim: Seq,
}

impl Preparation {
    fn new(journal: &mut Journal) -> Self {
        let controller = design_task(journal, 1);
        let statement = journal
            .read(|read| read.task_head(&controller))
            .expect("controller statement reads")
            .expect("controller exists")
            .position();
        let output = task(journal, 2, Some(prepared_by(&controller)));
        let spare = task(journal, 3, None);
        let input = task(journal, 4, None);
        let claim = claim(journal, &controller);
        Self {
            controller,
            output,
            spare,
            input,
            statement,
            claim,
        }
    }

    fn candidate(&self, micros: i64) -> CandidateRef {
        CandidateRef {
            controller: self.controller.clone(),
            candidate: authority(micros),
        }
    }

    fn retained(&self, micros: i64, outputs: Vec<TaskStamp>) -> DesignProposed {
        self.proposal(micros, wire::rooms::DesignPassRecommendation::Work, outputs)
    }

    fn proposal(
        &self,
        micros: i64,
        recommendation: wire::rooms::DesignPassRecommendation,
        outputs: Vec<TaskStamp>,
    ) -> DesignProposed {
        DesignProposed {
            candidate: DesignPassProposal {
                candidate: self.candidate(micros),
                statement: self.statement,
                claim: Some(self.claim),
                recommendation,
                governed_inputs: (recommendation == wire::rooms::DesignPassRecommendation::Work)
                    .then(|| self.input.clone())
                    .into_iter()
                    .collect(),
                outputs,
                justification: "the cited contract supports this pass".to_owned(),
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
                coverage: Vec::new(),
            },
        }
    }
}

fn selected(candidate: CandidateRef, outputs: Vec<TaskStamp>) -> DecisionOutcome {
    DecisionOutcome::Resolved {
        candidate,
        outputs: outputs
            .into_iter()
            .map(|output| OutputDecision {
                output,
                verdict: OutputVerdict::Accepted,
                comment: None,
            })
            .collect(),
        comment: None,
    }
}

fn publication(room: &TaskStamp, micros: i64, content: RoomPublicationContent) -> RoomPublished {
    RoomPublished {
        room: room.clone(),
        publication: authority(micros),
        content,
    }
}

/// The partition the journal currently holds.
fn partition(journal: &Journal) -> WorkingGroupPartition {
    journal
        .read(|read| read.group_partition())
        .expect("partition reads")
        .map(crate::JournalHead::into_body)
        .unwrap_or_default()
}

/// `design` moving from wherever it stands into group `to`, or out.
fn moved(journal: &Journal, design: &TaskStamp, to: Option<i64>) -> WorkingGroupMoved {
    let current = partition(journal);
    let from = current.holder(design).map(|group| group.group.clone());
    let to = to.map(authority);
    let mut groups = current.groups;
    for group in &mut groups {
        group.anchors.retain(|anchor| anchor != design);
    }
    groups.retain(|group| !group.anchors.is_empty());
    if let Some(target) = &to {
        match groups.iter_mut().find(|group| &group.group == target) {
            Some(group) => group.anchors.push(design.clone()),
            None => groups.push(WorkingGroup {
                group: target.clone(),
                anchors: vec![design.clone()],
            }),
        }
    }
    WorkingGroupMoved {
        design: design.clone(),
        from,
        to,
        partition: WorkingGroupPartition {
            revision: current.revision + 1,
            groups,
            fast: Vec::new(),
        },
    }
}

/// The current partition restated as `groups`, at the next revision.
fn arranged(journal: &Journal, groups: Vec<(i64, Vec<TaskStamp>)>) -> WorkingGroupsArranged {
    let from = partition(journal);
    WorkingGroupsArranged {
        partition: WorkingGroupPartition {
            revision: from.revision + 1,
            groups: groups
                .into_iter()
                .map(|(group, anchors)| WorkingGroup {
                    group: authority(group),
                    anchors,
                })
                .collect(),
            fast: Vec::new(),
        },
        from,
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
            selected(preparation.candidate(1), vec![preparation.output.clone()]),
        ),
    )
    .expect("decision recorded");
    authorize(
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
            outcome: DecisionOutcome::Resolved { .. },
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
             UPDATE proposal_heads SET head_seq = (SELECT min(seq) FROM fact_envelopes)
             WHERE head_seq = (SELECT max(head_seq) FROM proposal_heads);
             INSERT INTO room_publications(room, publication, seq)
             VALUES ('ghost', 'ghost', (SELECT seq FROM fact_envelopes ORDER BY seq LIMIT 1 OFFSET 1));",
        )
        .expect("corrupt the projection");
    assert!(refused_classes(&journal).contains(&ViolationClass::Head));

    journal.reindex().expect("projection rebuilds from facts");
    assert_eq!(projection(&journal), derived);
    assert_eq!(fact_bytes(&journal), facts, "rebuild never rewrites facts");
    journal.verify().expect("rebuilt projection verifies");
}

#[test]
fn publication_reads_start_after_the_cursor_without_decoding_older_room_history() {
    const PRE_HORIZON_PUBLICATIONS: usize = 1_025;
    const POST_HORIZON_PUBLICATIONS: usize = 3;

    let mut journal = Journal::in_memory().expect("journal");
    let preparation = authorized(&mut journal);
    let room = preparation.controller;
    let mut retained = write();
    for index in 0..PRE_HORIZON_PUBLICATIONS {
        retained
            .fact(
                publication(
                    &room,
                    i64::try_from(index).expect("fixture index") + 1,
                    RoomPublicationContent::Authored {
                        text: format!("retained {index}"),
                    },
                ),
                FactCause::Root,
            )
            .expect("retained publication builds");
    }
    journal
        .apply(retained)
        .expect("retained publications commit");
    let retained_start = journal
        .read(|read| read.room_publications(&room, None, fact_budget(1)))
        .expect("retained start reads")
        .rows[0]
        .seq;
    let after = cursor(&journal).expect("retained horizon");

    let mut current = write();
    for index in 0..POST_HORIZON_PUBLICATIONS {
        current
            .fact(
                publication(
                    &room,
                    i64::try_from(PRE_HORIZON_PUBLICATIONS + index + 1).expect("fixture index"),
                    RoomPublicationContent::Authored {
                        text: format!("current {index}"),
                    },
                ),
                FactCause::Root,
            )
            .expect("current publication builds");
    }
    journal.apply(current).expect("current publications commit");

    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = x'00' WHERE seq = ?1",
            [i64::try_from(retained_start.get()).expect("fixture sequence")],
        )
        .expect("make an older publication undecodable");
    let (page, latest) = journal
        .read(|read| {
            Ok((
                read.room_publications(&room, Some(after), fact_budget(8))?,
                read.latest_room_publication(&room, &Actor::Operator)?,
            ))
        })
        .expect("post-horizon authority reads without touching older bytes");
    assert_eq!(page.rows.len(), POST_HORIZON_PUBLICATIONS);
    assert!(page.rows.iter().all(|publication| publication.seq > after));
    assert_eq!(
        latest.map(|publication| publication.seq),
        page.rows.last().map(|publication| publication.seq)
    );
}

#[test]
fn preparation_authority_pages_one_controller_in_journal_order() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = authorized(&mut journal);
    commit(
        &mut journal,
        CandidateCritiqued {
            critique: CandidateCritique {
                critique: CritiqueRef {
                    candidate: preparation.candidate(1),
                    critique: SequenceCode::new(1),
                },
                category: name("composition"),
                source: journal_source(1),
                standing: CritiqueStanding::Clean,
            },
        },
    )
    .expect("critique recorded");

    let controller = preparation.controller.clone();
    let (first, population) = journal
        .read(|read| {
            Ok((
                read.preparation_authority_page(&controller, None, fact_budget(2))?,
                read.preparation_authority_count(&controller, None)?,
            ))
        })
        .expect("first preparation page reads");
    assert_eq!(population, 5);
    assert_eq!(first.rows.len(), 2);
    let continuation = first.continuation.expect("first page continues");
    let rest = journal
        .read(|read| {
            read.preparation_authority_page(&controller, Some(continuation), fact_budget(8))
        })
        .expect("preparation continuation reads");
    assert_eq!(rest.rows.len(), 3);
    assert_eq!(rest.continuation, None);
    let kinds = first
        .rows
        .iter()
        .chain(&rest.rows)
        .map(|envelope| envelope.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            FactKind::DesignProposed,
            FactKind::CandidateCovered,
            FactKind::PreparationDecided,
            FactKind::ExecutionAuthorized,
            FactKind::CandidateCritiqued,
        ]
    );
    assert!(
        journal
            .read(|read| read.preparation_authority_contains(&controller, continuation))
            .expect("controller cursor reads")
    );
    assert!(
        !journal
            .read(|read| read.preparation_authority_contains(&preparation.input, continuation))
            .expect("foreign controller cursor reads")
    );
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
        violations.iter().any(|violation| violation.subject
            == format!("room_authority:{}", wire::SequenceCode::new(seq))),
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
        DesignProposed {
            candidate: DesignPassProposal {
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
fn links_must_name_existing_authority() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let room = preparation.controller.clone();
    let future = cursor(&journal).expect("nonempty").get() + 1;
    let transcript = RoomSource::Transcript(TranscriptHunk {
        head: transcript_source_head(),
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
        },
        text: TextRange { start: 0, end: 1 },
    });
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
            link: RoomSource::Transcript(TranscriptHunk {
                head,
                locus,
                text: TextRange { start: 0, end: 1 },
            }),
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

    let mut next_member = head.clone();
    next_member.next = TranscriptPosition {
        member: 2,
        offset: 0,
    };
    ingest(
        &mut journal,
        next_member.clone(),
        TranscriptPosition {
            member: 1,
            offset: 8_192,
        },
        None,
    );
    let result = commit(
        &mut journal,
        publication(&room, 280, linked(next_member, locus(u64::MAX, 2))),
    );
    refused(&journal, result);

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

#[test]
fn a_candidate_accepts_one_complete_decision_and_refuses_another() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Resolved {
                candidate: preparation.candidate(1),
                outputs: vec![OutputDecision {
                    output: preparation.output.clone(),
                    verdict: OutputVerdict::Rejected,
                    comment: Some("not this boundary".to_owned()),
                }],
                comment: Some("keep the Design open".to_owned()),
            },
        ),
    )
    .expect("first decision recorded");
    let before = fact_bytes(&journal);
    let duplicate = commit(
        &mut journal,
        preparation.decision(
            2,
            DecisionOutcome::Resolved {
                candidate: preparation.candidate(1),
                outputs: vec![OutputDecision {
                    output: preparation.output.clone(),
                    verdict: OutputVerdict::Accepted,
                    comment: None,
                }],
                comment: None,
            },
        ),
    );
    refused(&journal, duplicate);
    assert_eq!(fact_bytes(&journal), before);
}

#[test]
fn duplicate_legacy_returns_remain_replayable_but_are_not_the_current_write_path() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    for (sequence, reason) in [(1, "first return"), (2, "repeated return")] {
        commit(
            &mut journal,
            preparation.decision(
                sequence,
                DecisionOutcome::Returned {
                    candidate: preparation.candidate(1),
                    reason: reason.to_owned(),
                },
            ),
        )
        .expect("retained legacy return remains admissible");
    }

    let expected = projection(&journal);
    journal.verify().expect("legacy returns verify");
    journal.reindex().expect("legacy returns rebuild");
    assert_eq!(projection(&journal), expected);
}

#[test]
fn newest_decisions_page_backward_strictly_before_the_cursor() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    for sequence in 1..=5 {
        commit(
            &mut journal,
            preparation.retained(sequence, vec![preparation.output.clone()]),
        )
        .expect("candidate retained");
        let returned = DecisionOutcome::Returned {
            candidate: preparation.candidate(sequence),
            reason: format!("reason {sequence}"),
        };
        commit(
            &mut journal,
            preparation.decision(sequence as u64, returned),
        )
        .expect("return recorded");
    }
    let budget = crate::FactReadBudget::new(2, crate::FactReadBudget::RETAINED.decoded_bytes())
        .expect("budget");
    let page = |before| {
        let page = journal
            .read(|read| read.newest_preparation_decisions(&preparation.controller, before, budget))
            .expect("decisions read");
        let codes: Vec<u64> = page
            .rows
            .iter()
            .map(|row| {
                row.fact
                    .read::<PreparationDecided>()
                    .expect("only decision facts")
                    .decision
                    .decision
                    .decision
                    .get()
            })
            .collect();
        (codes, page.continuation)
    };

    let (newest, cursor) = page(None);
    assert_eq!(newest, [5, 4], "newest first, the page bounded");
    let (older, cursor) = page(cursor);
    assert_eq!(older, [3, 2], "the cursor is exclusive");
    let (oldest, cursor) = page(cursor);
    assert_eq!(
        (oldest, cursor),
        (vec![1], None),
        "the first decision ends the record"
    );
}

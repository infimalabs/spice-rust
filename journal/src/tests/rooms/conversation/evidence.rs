//! Required question evidence: explicit task and candidate criteria consume
//! answers without treating them as approval.

use super::*;
use wire::TaskEvidenceEdit;
use wire::body::Verdict;

/// A statement edit of `task` by `actor` carrying `edit` and writing `carried`
/// as the statement's evidence.
fn edit(
    journal: &mut Journal,
    actor: Actor,
    task: &TaskStamp,
    edit: Option<TaskEvidenceEdit>,
    carried: Vec<QuestionResultRef>,
) -> Commit {
    let head = statement(journal, task);
    let mut write = write_as(actor);
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: 2,
                dependencies: None,
                intent: None,
                evidence: edit,
            },
            FactCause::Root,
        )
        .expect("edit fact builds");
    write.task_head(fact, move |revision| TaskHead {
        evidence: carried,
        revision,
        ..head
    });
    journal.apply(write)
}

/// An explicit evidence change whose statement carries what it applies to.
fn change(
    journal: &mut Journal,
    actor: Actor,
    task: &TaskStamp,
    change: TaskEvidenceEdit,
) -> Commit {
    let current = statement(journal, task).evidence;
    let carried = change.apply(&current).unwrap_or(current);
    edit(journal, actor, task, Some(change), carried)
}

fn require(result: &QuestionResultRef) -> TaskEvidenceEdit {
    TaskEvidenceEdit::Require {
        result: result.clone(),
    }
}

fn complete(journal: &mut Journal, task: &TaskStamp) -> Commit {
    commit(
        journal,
        wire::body::TaskDone {
            task: task.clone(),
            validation: "checked".to_owned(),
            terminal: None,
            git: None,
            source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
}

fn review(journal: &mut Journal, task: &TaskStamp, verdict: Verdict) -> Commit {
    let finding = (verdict == Verdict::Finding).then(|| "needs work".to_owned());
    commit(
        journal,
        wire::body::TaskReviewed {
            task: task.clone(),
            reviewer: Actor::Operator,
            verdict,
            finding,
            note: None,
            followups: Vec::new(),
            git: None,
            source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
}

/// The refusal explains each unsatisfied result by identity and state.
fn unsatisfied(journal: &Journal, result: Commit, unmet: &QuestionResultRef, state: &str) {
    assert!(
        matches!(&result, Err(CommitError::Refused(refusal))
            if refusal.to_string().contains(&unmet.to_string())
                && refusal.to_string().contains(state)),
        "{unmet} stays {state}: {result:?}"
    );
    refused(journal, result);
}

#[test]
fn required_evidence_gates_positive_completion_until_answered() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let task = room.room.clone();
    let question = room.ask(10);
    review(&mut room.journal, &task, Verdict::Clean)
        .expect("a pending question nobody requires blocks nothing");

    change(
        &mut room.journal,
        Actor::Lane(room.other.lane),
        &task,
        require(&question),
    )
    .expect("an editor requires a result asked in the task's room");
    assert_eq!(
        statement(&room.journal, &task).evidence,
        vec![question.clone()]
    );
    let result = complete(&mut room.journal, &task);
    unsatisfied(&room.journal, result, &question, "pending");
    let result = review(&mut room.journal, &task, Verdict::Clean);
    unsatisfied(&room.journal, result, &question, "pending");
    review(&mut room.journal, &task, Verdict::Finding)
        .expect("a finding is not positive completion");

    room.answer(20, &asked, &question, minute(3))
        .expect("the respondent answers");
    assert_eq!(
        statement(&room.journal, &task).evidence,
        vec![question],
        "an answer satisfies the requirement without retiring it"
    );
    complete(&mut room.journal, &task).expect("answered evidence is available for assessment");
    room.journal.verify().expect("required evidence rebuilds");
}

#[test]
fn every_terminal_outcome_but_an_answer_leaves_required_evidence_unsatisfied() {
    type Settle = fn(&mut Conversation, &QuestionResultRef) -> Commit;
    let outcomes: [(&str, Settle); 3] = [
        ("declined", |room, result| room.nak(result, minute(2))),
        ("cancelled", |room, result| {
            room.close(Actor::Operator, result, QuestionClosure::Cancelled)
        }),
        ("timed_out", |room, result| {
            room.time_out(result, minute(30))
        }),
    ];
    for (state, settle) in outcomes {
        let mut room = Conversation::new();
        let asked = room.asked.clone();
        let task = room.room.clone();
        let question = room.ask(10);
        change(
            &mut room.journal,
            Actor::Operator,
            &task,
            require(&question),
        )
        .expect("the operator requires the result");
        settle(&mut room, &question).expect("the result settles");
        let late = room.answer(20, &asked, &question, minute(31));
        room.refused(late);
        let result = complete(&mut room.journal, &task);
        unsatisfied(&room.journal, result, &question, state);
        room.journal
            .verify()
            .expect("unsatisfied evidence rebuilds");
    }
}

#[test]
fn required_evidence_moves_only_through_explicit_guarded_changes() {
    let mut room = Conversation::new();
    let task = room.room.clone();
    let lane = Actor::Lane(room.other.lane);
    let first = room.ask(10);
    let second = room.ask(20);
    let other_room = task_in(&mut room, 2);
    let home = std::mem::replace(&mut room.room, other_room);
    let elsewhere = room.ask(30);
    room.room = home;

    for unbound in [elsewhere, QuestionResultRef::new(authority(99))] {
        let result = change(&mut room.journal, lane.clone(), &task, require(&unbound));
        room.refused(result);
    }
    change(&mut room.journal, lane.clone(), &task, require(&first)).expect("first required");
    let twice = change(&mut room.journal, lane.clone(), &task, require(&first));
    room.refused(twice);

    edit_project(&mut room.journal, &task, "elsewhere");
    assert_eq!(
        statement(&room.journal, &task).evidence,
        vec![first.clone()]
    );
    let dropped = edit(&mut room.journal, Actor::Operator, &task, None, Vec::new());
    room.refused(dropped);

    let replace = TaskEvidenceEdit::Replace {
        result: first.clone(),
        with: second.clone(),
    };
    let waive = |result: &QuestionResultRef| TaskEvidenceEdit::Waive {
        result: result.clone(),
    };
    for released in [replace.clone(), waive(&first)] {
        let result = change(&mut room.journal, lane.clone(), &task, released);
        room.refused(result);
    }
    change(&mut room.journal, Actor::Operator, &task, replace).expect("operator replaces");
    assert_eq!(
        statement(&room.journal, &task).evidence,
        vec![second.clone()]
    );
    let result = complete(&mut room.journal, &task);
    unsatisfied(&room.journal, result, &second, "pending");

    change(&mut room.journal, Actor::Operator, &task, waive(&second)).expect("operator waives");
    complete(&mut room.journal, &task).expect("waived evidence no longer gates completion");
    room.journal.verify().expect("evidence changes rebuild");
}

/// A task `offset` in the same journal as the conversation's room.
fn task_in(room: &mut Conversation, offset: i64) -> TaskStamp {
    task(&mut room.journal, offset, None)
}

#[test]
fn reindex_preserves_required_evidence_and_its_completion_gate() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let task = room.room.clone();
    let question = room.ask(10);
    change(
        &mut room.journal,
        Actor::Operator,
        &task,
        require(&question),
    )
    .expect("the operator requires the pending result");

    room.journal
        .connection
        .execute("DELETE FROM task_evidence", ())
        .expect("remove the derived evidence row");
    assert!(
        refused_violations(&room.journal)
            .iter()
            .any(|violation| violation.class == ViolationClass::Head
                && violation.subject.starts_with("task_evidence:")),
        "verification must name the missing derived evidence row"
    );

    room.journal.reindex().expect("pending evidence rebuilds");
    let pending = complete(&mut room.journal, &task);
    unsatisfied(&room.journal, pending, &question, "pending");

    room.answer(20, &asked, &question, minute(3))
        .expect("the respondent answers");
    room.journal.reindex().expect("answered evidence rebuilds");
    complete(&mut room.journal, &task).expect("answered evidence permits positive completion");
    room.journal
        .verify()
        .expect("rebuilt evidence remains verifiable");
}

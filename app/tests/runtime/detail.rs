use super::*;
use app::TaskDetailState;
use wire::Actor;
use wire::intent::{TaskDetailAnswer, TaskDetailNote};

fn detail_answer(label: &str, text: &str) -> TaskDetailAnswer {
    TaskDetailAnswer {
        task: board_task(
            label,
            "unused",
            "Selected task",
            "canvas.detail",
            wire::EnvironmentRequirements::default(),
        ),
        notes: vec![TaskDetailNote {
            position: seq(41),
            at: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            author: Actor::Operator,
            text: text.to_owned(),
        }],
    }
}

fn detail_response(
    session: &Session,
    answer: TaskDetailAnswer,
) -> app::Admitted<Response<TaskDetailAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(44)),
            body: answer,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("detail response is admitted")
}

fn detail_refusal(session: &Session, reason: &str) -> app::Admitted<Response<TaskDetailAnswer>> {
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::<TaskDetailAnswer>::Refusal(wire::Refusal::bare(
            wire::RefusalDiagnostic::new(reason.to_owned(), None, None, None)
                .with_detail("request one current durable task identity"),
        )),
    );
    session
        .admit_frame(&bytes)
        .expect("detail refusal is admitted")
}

fn task_note_fact(task: &wire::TaskStamp, position: u64) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": format!("task:{task}"),
        "fact": {
            "kind": "task::noted",
            "task": task,
            "note": "new evidence",
        },
    })
}

fn task_phase_fact(task: &wire::TaskStamp, position: u64) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": format!("task:{task}"),
        "fact": {
            "kind": "task::phased",
            "task": task,
            "phase": "verify",
        },
    })
}

#[test]
fn repeated_detail_answers_replace_instead_of_duplicating_notes() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let task = fixture_stamp("detail-replay");
    let response = detail_response(&session, detail_answer("detail-replay", "one note"));

    runtime.begin_task_detail(task.clone());
    assert_eq!(runtime.task_detail(&task), Some(&TaskDetailState::Loading));
    runtime.apply_task_detail(&task, &response);
    runtime.apply_task_detail(&task, &response);

    let Some(TaskDetailState::Available(answer)) = runtime.task_detail(&task) else {
        panic!("replayed detail response was not retained")
    };
    assert_eq!(answer.notes.len(), 1);

    assert_eq!(answer.notes[0].text, "one note");
}
#[test]
fn detail_refusal_replaces_loading_with_explicit_unavailable_state() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let task = fixture_stamp("detail-missing");

    runtime.begin_task_detail(task.clone());
    runtime.apply_task_detail(&task, &detail_refusal(&session, "task::detail-unknown"));

    assert!(matches!(
        runtime.task_detail(&task),
        Some(TaskDetailState::Unavailable { reason, detail })
            if reason == "task::detail-unknown"
                && detail.as_deref() == Some("request one current durable task identity")
    ));
}

#[test]
fn task_fact_invalidates_only_the_affected_open_detail() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let affected = fixture_stamp("detail-affected");
    let untouched = fixture_stamp("detail-untouched");

    runtime.apply_task_detail(
        &affected,
        &detail_response(&session, detail_answer("detail-affected", "affected note")),
    );
    runtime.apply_task_detail(
        &untouched,
        &detail_response(
            &session,
            detail_answer("detail-untouched", "untouched note"),
        ),
    );

    let frame = admitted_stream(&session, &fact_frame(task_note_fact(&affected, 45)));
    let stale = stale_reads(&frame);
    let tasks = stale.task_details().cloned().collect::<Vec<_>>();
    assert_eq!(tasks, vec![affected.clone()]);
    for task in tasks {
        assert!(runtime.invalidate_task_detail(&task));
    }

    assert!(matches!(
        runtime.task_detail(&affected),
        Some(TaskDetailState::Refreshing(answer)) if answer.notes[0].text == "affected note"
    ));
    assert!(matches!(
        runtime.task_detail(&untouched),
        Some(TaskDetailState::Available(answer)) if answer.notes[0].text == "untouched note"
    ));

    runtime.apply_task_detail(
        &affected,
        &detail_response(&session, detail_answer("detail-affected", "refreshed note")),
    );
    assert!(matches!(
        runtime.task_detail(&affected),
        Some(TaskDetailState::Available(answer)) if answer.notes[0].text == "refreshed note"
    ));
    let head = admitted_stream(&session, &fact_frame(task_phase_fact(&affected, 46)));
    assert_eq!(
        stale_reads(&head)
            .task_details()
            .cloned()
            .collect::<Vec<_>>(),
        vec![affected]
    );
}

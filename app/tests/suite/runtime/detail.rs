use super::*;
use app::TaskDetailState;
use std::collections::{BTreeMap, BTreeSet};
use wire::intent::TaskDetailAnswer;

fn detail_answer(label: &str, title: &str) -> TaskDetailAnswer {
    TaskDetailAnswer {
        task: board_task(
            label,
            "unused",
            title,
            "canvas.detail",
            wire::TaskFiling::Filed,
            None,
            wire::EnvironmentRequirements::default(),
        ),
        media: Vec::new(),
        closure: None,
        disposition: None,
        origin_request: None,
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
fn detail_refusal_replaces_loading_with_explicit_unavailable_state() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let task = fixture_stamp("detail-missing");

    runtime.show_task_details([task.clone()]);
    runtime.apply_task_detail(&task, &detail_refusal(&session, "task::detail-unknown"));

    assert!(matches!(
        runtime.task_detail(&task),
        Some(TaskDetailState::Unavailable { reason, detail })
            if reason == "task::detail-unknown"
                && detail.as_deref() == Some("request one current durable task identity")
    ));
}

#[test]
fn task_fact_rereads_only_the_affected_shown_detail() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let affected = fixture_stamp("detail-affected");
    let untouched = fixture_stamp("detail-untouched");

    runtime.show_task_details([affected.clone(), untouched.clone()]);
    let sent = runtime.take_task_detail_reads(usize::MAX, |_| false);
    assert_eq!(sent.len(), 2);
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
        Some(TaskDetailState::Refreshing(answer)) if answer.task.presentation_title == "affected note"
    ));
    assert!(matches!(
        runtime.task_detail(&untouched),
        Some(TaskDetailState::Available(answer)) if answer.task.presentation_title == "untouched note"
    ));
    assert_eq!(
        runtime.take_task_detail_reads(usize::MAX, |_| false),
        vec![affected.clone()]
    );

    runtime.apply_task_detail(
        &affected,
        &detail_response(&session, detail_answer("detail-affected", "refreshed note")),
    );
    assert!(matches!(
        runtime.task_detail(&affected),
        Some(TaskDetailState::Available(answer)) if answer.task.presentation_title == "refreshed note"
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

#[test]
fn a_fact_forgets_a_detail_no_surface_shows_instead_of_reading_it() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let task = fixture_stamp("detail-hidden");
    runtime.show_task_details([task.clone()]);
    runtime.take_task_detail_reads(usize::MAX, |_| false);
    runtime.apply_task_detail(
        &task,
        &detail_response(&session, detail_answer("detail-hidden", "kept note")),
    );
    assert!(!runtime.show_task_details([]));

    assert!(!runtime.invalidate_task_detail(&task));
    assert_eq!(runtime.task_detail(&task), None);
    assert!(
        runtime
            .take_task_detail_reads(usize::MAX, |_| false)
            .is_empty()
    );

    assert!(runtime.show_task_details([task.clone()]));
    assert_eq!(runtime.task_detail(&task), Some(&TaskDetailState::Loading));
    assert_eq!(
        runtime.take_task_detail_reads(usize::MAX, |_| false),
        vec![task]
    );
}

#[test]
fn a_command_settling_after_deselection_creates_no_hidden_detail_demand() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let shown = fixture_stamp("detail-shown-command");
    let hidden = fixture_stamp("detail-hidden-command");
    runtime.show_task_details([shown.clone(), hidden.clone()]);
    runtime.take_task_detail_reads(usize::MAX, |_| false);
    for (task, label) in [
        (&shown, "detail-shown-command"),
        (&hidden, "detail-hidden-command"),
    ] {
        runtime.apply_task_detail(
            task,
            &detail_response(&session, detail_answer(label, "before the command")),
        );
    }
    runtime.show_task_details([shown.clone()]);

    runtime.reread_task_detail(&hidden);
    assert_eq!(runtime.task_detail(&hidden), None);
    assert!(
        runtime
            .take_task_detail_reads(usize::MAX, |_| false)
            .is_empty()
    );
    runtime.apply_task_detail(
        &hidden,
        &detail_response(
            &session,
            detail_answer("detail-hidden-command", "late stale answer"),
        ),
    );
    assert_eq!(runtime.task_detail(&hidden), None);

    runtime.reread_task_detail(&shown);
    assert!(matches!(
        runtime.task_detail(&shown),
        Some(TaskDetailState::Refreshing(answer))
            if answer.task.presentation_title == "before the command"
    ));
    assert_eq!(
        runtime.take_task_detail_reads(usize::MAX, |_| false),
        vec![shown]
    );
}

#[test]
fn a_refresh_settling_after_deselection_is_read_again_when_shown_again() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let task = fixture_stamp("detail-refresh-hidden");
    let answer = |text| detail_response(&session, detail_answer("detail-refresh-hidden", text));
    runtime.show_task_details([task.clone()]);
    runtime.take_task_detail_reads(usize::MAX, |_| false);
    runtime.apply_task_detail(&task, &answer("old"));
    assert!(runtime.invalidate_task_detail(&task));
    assert_eq!(
        runtime.take_task_detail_reads(usize::MAX, |_| false),
        vec![task.clone()]
    );

    runtime.show_task_details([]);
    runtime.apply_task_detail(&task, &answer("in flight"));
    assert_eq!(
        runtime.task_detail(&task),
        None,
        "a refresh answered after deselection forgets the hidden detail"
    );

    assert!(runtime.show_task_details([task.clone()]));
    assert_eq!(runtime.task_detail(&task), Some(&TaskDetailState::Loading));
    assert_eq!(
        runtime.take_task_detail_reads(usize::MAX, |_| false),
        vec![task.clone()],
        "showing it again owes exactly one read"
    );
    runtime.apply_task_detail(&task, &answer("current"));
    assert!(matches!(
        runtime.task_detail(&task),
        Some(TaskDetailState::Available(answer)) if answer.task.presentation_title == "current"
    ));
}

/// Drives the connection's pump against a mailbox that holds `limit` reads
/// and refuses the next `refusals` sends, answering the oldest in-flight read
/// between passes. A refusal leaves the rest of that pass owed, as the pump
/// does. Returns how many reads each task drew.
fn hydrate(
    runtime: &mut AppRuntime,
    session: &Session,
    labels: &[String],
    limit: usize,
    mut refusals: usize,
) -> BTreeMap<wire::TaskStamp, usize> {
    let mut in_flight = Vec::<wire::TaskStamp>::new();
    let mut reads = BTreeMap::new();
    for _ in 0..labels.len() * 4 {
        let mut taken = runtime
            .take_task_detail_reads(limit - in_flight.len(), |task| in_flight.contains(task))
            .into_iter();
        while let Some(task) = taken.next() {
            if refusals > 0 {
                refusals -= 1;
                std::iter::once(task)
                    .chain(taken.by_ref())
                    .for_each(|task| runtime.restore_task_detail_read(task));
                break;
            }
            *reads.entry(task.clone()).or_insert(0) += 1;
            in_flight.push(task);
        }
        if in_flight.is_empty() {
            continue;
        }
        let task = in_flight.remove(0);
        let label = labels
            .iter()
            .find(|label| fixture_stamp(label) == task)
            .expect("answered task was shown");
        runtime.apply_task_detail(
            &task,
            &detail_response(session, detail_answer(label, "note")),
        );
    }
    reads
}

#[test]
fn a_selection_larger_than_the_read_budget_hydrates_every_task_once_per_connection() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let labels = (0..80).map(|n| format!("bulk-{n}")).collect::<Vec<_>>();
    let tasks = labels
        .iter()
        .map(|label| fixture_stamp(label))
        .collect::<Vec<_>>();
    let assert_hydrated = |runtime: &AppRuntime, reads: &BTreeMap<wire::TaskStamp, usize>| {
        for task in &tasks {
            assert!(
                matches!(
                    runtime.task_detail(task),
                    Some(TaskDetailState::Available(_))
                ),
                "{task} stayed unanswered"
            );
            assert_eq!(
                reads.get(task),
                Some(&1),
                "{task} was not read exactly once"
            );
        }
    };

    assert!(runtime.show_task_details(tasks.iter().cloned()));
    let first_pass = runtime.take_task_detail_reads(16, |_| false);
    assert_eq!(
        first_pass.len(),
        16,
        "one pass sends no more than its budget"
    );
    first_pass
        .into_iter()
        .for_each(|task| runtime.restore_task_detail_read(task));
    let reads = hydrate(&mut runtime, &session, &labels, 16, 5);
    assert_hydrated(&runtime, &reads);

    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));
    let reads = hydrate(&mut runtime, &session, &labels, 16, 0);
    assert_hydrated(&runtime, &reads);
}

#[test]
fn changing_the_selection_stops_reads_for_tasks_no_longer_shown() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let first = (0..20)
        .map(|n| fixture_stamp(&format!("first-{n}")))
        .collect::<Vec<_>>();
    let second = (0..3)
        .map(|n| fixture_stamp(&format!("second-{n}")))
        .collect::<Vec<_>>();

    runtime.show_task_details(first.iter().cloned());
    let sent = runtime.take_task_detail_reads(4, |_| false);
    runtime.show_task_details(second.iter().cloned().chain([sent[0].clone()]));

    for task in first.iter().filter(|task| **task != sent[0]) {
        assert_eq!(runtime.task_detail(task), None, "{task} kept a placeholder");
    }
    assert_eq!(
        runtime.task_detail(&sent[0]),
        Some(&TaskDetailState::Loading)
    );
    let owed = runtime.take_task_detail_reads(usize::MAX, |task| sent.contains(task));
    assert_eq!(owed, {
        let mut owed = second.clone();
        owed.sort();
        owed
    });

    runtime.apply_task_detail(&sent[1], &detail_refusal(&session, "task::detail-unknown"));
    assert!(
        runtime
            .take_task_detail_reads(usize::MAX, |_| false)
            .is_empty()
    );
}

#[test]
fn reconnect_rereads_every_shown_detail_and_forgets_the_rest() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let shown = fixture_stamp("detail-shown");
    let pending = fixture_stamp("detail-pending");
    let hidden = fixture_stamp("detail-hidden");
    runtime.show_task_details([shown.clone(), pending.clone(), hidden.clone()]);
    runtime.take_task_detail_reads(usize::MAX, |_| false);
    for (task, label) in [(&shown, "detail-shown"), (&hidden, "detail-hidden")] {
        runtime.apply_task_detail(
            task,
            &detail_response(&session, detail_answer(label, "before the drop")),
        );
    }
    runtime.show_task_details([shown.clone(), pending.clone()]);

    assert!(!runtime.reconnect(&acceptance(&session, "operator", EPOCH)));

    assert!(matches!(
        runtime.task_detail(&shown),
        Some(TaskDetailState::Refreshing(answer)) if answer.task.presentation_title == "before the drop"
    ));
    assert_eq!(
        runtime.task_detail(&pending),
        Some(&TaskDetailState::Loading)
    );
    assert_eq!(runtime.task_detail(&hidden), None);
    assert_eq!(
        runtime
            .take_task_detail_reads(usize::MAX, |_| false)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([shown, pending]),
    );
}

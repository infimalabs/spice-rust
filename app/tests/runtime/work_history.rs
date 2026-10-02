use super::*;

pub(super) fn terminal_history_task(
    stamp: &str,
    title: &str,
    workflow: u64,
    completion: &str,
) -> wire::intent::BoardTask {
    let mut task = board_task(
        stamp,
        THIRD_LANE,
        title,
        "app.history",
        None,
        wire::EnvironmentRequirements::default(),
    );
    task.workflow.position = seq(workflow);
    task.workflow.body.status = "completed".to_owned();
    task.workflow.body.completion = Some(completion.parse().expect("history completion instant"));
    task
}

pub(super) fn history_page_answer(
    session: &Session,
    open: &app::Admitted<Response<wire::intent::BoardAnswer>>,
    before: Option<wire::intent::TaskHistoryCursor>,
    tasks: Vec<wire::intent::BoardTask>,
    total: u64,
    next: Option<wire::intent::TaskHistoryCursor>,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let mut body = match &open.get().outcome {
        Outcome::Reply(reply) => reply.body.clone(),
        Outcome::Refusal(_) | Outcome::Fault(_) => panic!("open fixture is a reply"),
    };
    body.tasks = tasks;
    body.history = Some(wire::intent::TaskHistoryPage {
        before,
        limit: 50,
        total,
        next,
    });
    admitted_board_answer(session, body)
}

#[test]
fn work_history_replaces_pages_and_navigates_in_both_directions() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let open = board_response(&session, "Hidden open work");
    runtime.apply_board(&open);

    let first = runtime
        .begin_work_history()
        .expect("the current open board admits its newest history page");
    assert_eq!(first.before, None);
    assert!(runtime.begin_work_history().is_none());
    let newest = terminal_history_task(
        "HISTORY-one",
        "Newest terminal work",
        10,
        "2030-04-12T18:44:02.000Z",
    );
    let next = wire::intent::TaskHistoryCursor {
        workflow: newest.workflow.position,
        task: newest.statement.body.born.clone(),
    };
    let newest_page =
        history_page_answer(&session, &open, None, vec![newest], 2, Some(next.clone()));
    runtime.apply_work_history(&first, &newest_page);
    assert_eq!(runtime.work_history().first_result(), 1);
    assert_eq!(runtime.work_history().last_result(), 1);
    assert!(!runtime.work_history().has_newer());
    assert!(runtime.work_history().has_older());
    assert_eq!(
        runtime.board().expect("board remains present").tasks.len(),
        3
    );

    let older = runtime
        .begin_older_work_history()
        .expect("the advertised older cursor is requestable");
    assert_eq!(older.before.as_ref(), Some(&next));
    let oldest = terminal_history_task(
        "HISTORY-two",
        "Oldest terminal work",
        9,
        "2030-04-11T18:44:02.000Z",
    );
    let oldest_stamp = oldest.statement.body.born.clone();
    let oldest_page = history_page_answer(&session, &open, Some(next), vec![oldest], 2, None);
    runtime.apply_work_history(&older, &oldest_page);
    let board = runtime.board().expect("older page remains with open board");
    assert_eq!(
        board.tasks.len(),
        3,
        "the previous terminal page was replaced"
    );
    assert!(
        board
            .tasks
            .iter()
            .any(|task| task.statement.body.born == oldest_stamp)
    );
    assert_eq!(runtime.work_history().first_result(), 2);
    assert!(runtime.work_history().has_newer());
    assert!(!runtime.work_history().has_older());

    let newer = runtime
        .begin_newer_work_history()
        .expect("the prior page remains reachable");
    assert_eq!(newer.before, None);
    runtime.apply_work_history(&newer, &newest_page);
    assert_eq!(runtime.work_history().first_result(), 1);
    assert!(!runtime.work_history().has_newer());
    assert!(runtime.work_history().has_older());
}

#[test]
fn work_history_filters_and_reconnects_invalidate_older_requests() {
    let cutoff = app::runtime::work_history_completion_cutoff("2030-04-12")
        .expect("date cutoff is valid")
        .expect("date cutoff is present");
    assert_eq!(cutoff.to_string(), "2030-04-12T23:59:59.999Z");
    assert_eq!(
        app::runtime::work_history_completion_cutoff("  ").expect("blank cutoff is valid"),
        None
    );
    assert!(app::runtime::work_history_completion_cutoff("2030-99-99").is_err());

    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let open = board_response(&session, "Hidden open work");
    runtime.apply_board(&open);
    let stale = runtime
        .apply_work_history_filters(" needle ".to_owned(), Some(cutoff))
        .expect("filters request their newest page");
    assert_eq!(stale.query.as_deref(), Some("needle"));
    assert_eq!(stale.completed_before, Some(cutoff));
    let stale_answer = history_page_answer(&session, &open, None, Vec::new(), 0, None);

    let current = runtime
        .apply_work_history_filters("replacement".to_owned(), None)
        .expect("changing filters supersedes the in-flight request");
    runtime.apply_work_history(&stale, &stale_answer);
    assert!(runtime.work_history().loading());
    assert_eq!(runtime.work_history().total(), None);
    let terminal = terminal_history_task(
        "HISTORY-current",
        "Current filtered terminal work",
        11,
        "2030-04-12T18:44:02.000Z",
    );
    let current_answer = history_page_answer(&session, &open, None, vec![terminal], 1, None);
    runtime.apply_work_history(&current, &current_answer);
    assert_eq!(runtime.board().expect("filtered page").tasks.len(), 3);

    assert!(
        !runtime.reconnect(&accepted),
        "the capability scope is unchanged"
    );
    assert_eq!(
        runtime
            .board()
            .expect("open rows survive reconnect")
            .tasks
            .len(),
        2
    );
    runtime.apply_work_history(&current, &current_answer);
    assert_eq!(runtime.work_history().total(), None);
    assert_eq!(
        runtime
            .board()
            .expect("stale page stays evicted")
            .tasks
            .len(),
        2
    );

    runtime.apply_board(&open);
    let resumed = runtime
        .begin_work_history()
        .expect("the refreshed board resumes the applied finder");
    assert_eq!(resumed.query.as_deref(), Some("replacement"));
    let no_match = history_page_answer(&session, &open, None, Vec::new(), 0, None);
    runtime.apply_work_history(&resumed, &no_match);
    assert_eq!(runtime.work_history().first_result(), 0);
    assert_eq!(runtime.work_history().last_result(), 0);
    assert!(runtime.work_history().filtered());
}

#[test]
fn work_history_scope_changes_restart_requests_without_mixing_pages() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let open = board_response(&session, "Open work");
    runtime.apply_board(&open);

    let all = runtime
        .begin_work_history()
        .expect("all Work requests its newest history page");
    assert_eq!(all.team, None);
    let team = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        .parse()
        .expect("fixture team");
    assert!(runtime.select_work_history_team(Some(team)));
    let scoped = runtime
        .begin_work_history()
        .expect("team Work restarts at its newest page");
    assert_eq!(scoped.team, Some(team));
    assert_ne!(scoped.revision, all.revision);

    let stale = history_page_answer(&session, &open, None, Vec::new(), 0, None);
    runtime.apply_work_history(&all, &stale);
    assert!(
        runtime.work_history().loading(),
        "the prior all-Work response cannot settle the team request"
    );

    assert!(runtime.select_work_history_team(None));
    let unscoped = runtime
        .begin_work_history()
        .expect("leaving team Work restarts all history");
    assert_eq!(unscoped.team, None);
    assert_ne!(unscoped.revision, scoped.revision);
    runtime.apply_work_history(&scoped, &stale);
    assert!(
        runtime.work_history().loading(),
        "the prior team response cannot settle all Work"
    );
}

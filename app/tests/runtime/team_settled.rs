use super::work_history::{history_page_answer, terminal_history_task};
use super::*;

fn settled_titles(runtime: &CanvasRuntime, team: wire::CanonicalUuid) -> Vec<String> {
    runtime
        .team_settled(team)
        .and_then(app::runtime::TeamSettledState::pages)
        .map(|pages| {
            pages
                .tasks
                .iter()
                .map(|task| task.presentation_title.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn team_settled_pages_are_reread_only_after_open_work_settles() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let open = board_response(&session, "Hidden open work");
    runtime.apply_board(&open);
    let team = uuid(THIRD_LANE);

    let first = runtime
        .begin_team_settled(team)
        .expect("a shown team reads its newest settled page");
    assert_eq!(first.team, team);
    assert_eq!(first.before, None);
    assert!(runtime.begin_team_settled(team).is_none());
    let newest = terminal_history_task("SETTLED-one", "Newest", 10, "2030-04-12T18:44:02.000Z");
    let next = wire::intent::TaskHistoryCursor {
        workflow: newest.workflow.position,
        task: newest.statement.body.born.clone(),
    };
    let newest_page =
        history_page_answer(&session, &open, None, vec![newest], 2, Some(next.clone()));
    runtime.apply_team_settled(&first, &newest_page);
    assert_eq!(settled_titles(&runtime, team), ["Newest"]);
    assert!(
        runtime
            .team_settled(team)
            .is_some_and(|state| state.has_older())
    );
    assert_eq!(
        runtime
            .board()
            .expect("open board is untouched")
            .tasks
            .len(),
        2
    );

    runtime.apply_board(&open);
    assert!(
        runtime.begin_team_settled(team).is_none(),
        "a refresh that settles nothing keeps the loaded page"
    );

    let older = runtime
        .begin_older_team_settled(team)
        .expect("the advertised older page is requestable");
    assert_eq!(older.before.as_ref(), Some(&next));
    let oldest = terminal_history_task("SETTLED-two", "Oldest", 9, "2030-04-11T18:44:02.000Z");
    let oldest_page = history_page_answer(&session, &open, Some(next), vec![oldest], 2, None);
    runtime.apply_team_settled(&older, &oldest_page);
    assert_eq!(settled_titles(&runtime, team), ["Oldest"]);
    let oldest_state = runtime.team_settled(team).expect("oldest page is visible");
    assert!(oldest_state.has_newer());
    assert!(!oldest_state.has_older());
    assert_eq!(
        (oldest_state.first_result(), oldest_state.last_result()),
        (2, 2)
    );

    let newer = runtime
        .begin_newer_team_settled(team)
        .expect("the newest page remains reachable");
    assert_eq!(newer.before, None);
    runtime.apply_team_settled(&newer, &newest_page);
    assert_eq!(settled_titles(&runtime, team), ["Newest"]);
    let newest_state = runtime.team_settled(team).expect("newest page is visible");
    assert!(!newest_state.has_newer());
    assert!(newest_state.has_older());
    assert_eq!(
        (newest_state.first_result(), newest_state.last_result()),
        (1, 1)
    );

    let mut settled_board = match &open.get().outcome {
        Outcome::Reply(reply) => reply.body.clone(),
        Outcome::Refusal(_) | Outcome::Fault(_) => panic!("open fixture is a reply"),
    };
    settled_board
        .tasks
        .retain(|task| task.presentation_title != "Visible allocator work");
    let revision = runtime.team_settled_revision();
    runtime.apply_board(&admitted_board_answer(&session, settled_board));
    assert!(runtime.team_settled_revision() > revision);
    assert_eq!(
        settled_titles(&runtime, team),
        ["Newest"],
        "the visible page stays until the fresh page lands"
    );
    let refreshed = runtime
        .begin_team_settled(team)
        .expect("work leaving the open board re-reads the newest page");
    assert_eq!(refreshed.before, None);
    assert_ne!(refreshed, first);

    runtime.apply_team_settled(&first, &newest_page);
    assert!(
        runtime
            .team_settled(team)
            .is_some_and(|state| state.loading()),
        "an answer reserved before the refresh is ignored"
    );
    let now_settled = terminal_history_task(
        "SETTLED-three",
        "Just settled",
        11,
        "2030-04-13T18:44:02.000Z",
    );
    let refreshed_page = history_page_answer(&session, &open, None, vec![now_settled], 3, None);
    runtime.apply_team_settled(&refreshed, &refreshed_page);
    assert_eq!(settled_titles(&runtime, team), ["Just settled"]);
}

#[test]
fn team_settled_filters_keep_an_independent_zero_based_page() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let open = board_response(&session, "Hidden open work");
    runtime.apply_board(&open);
    let team = uuid(SECOND_LANE);

    let empty_request = runtime
        .begin_team_settled(team)
        .expect("another team owns an independent newest page");
    let empty_page = history_page_answer(&session, &open, None, vec![], 0, None);
    runtime.apply_team_settled(&empty_request, &empty_page);
    let empty_state = runtime
        .team_settled(team)
        .expect("the empty team page is retained");
    assert_eq!(
        (empty_state.first_result(), empty_state.last_result()),
        (0, 0)
    );

    let cutoff = app::runtime::work_history_completion_cutoff("2030-04-13")
        .expect("a calendar day is a valid history cutoff");
    let filtered_request = runtime
        .filter_team_settled(team, "  match me  ".to_owned(), cutoff)
        .expect("finder criteria reserve the newest matching page");
    assert_eq!(filtered_request.team, team);
    assert_eq!(filtered_request.query.as_deref(), Some("match me"));
    assert_eq!(filtered_request.completed_before, cutoff);
    assert!(
        runtime.team_settled(team).is_some_and(|state| {
            state.filtered() && state.loading() && state.pages().is_none()
        })
    );
}

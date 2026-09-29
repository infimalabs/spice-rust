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
    assert_eq!(settled_titles(&runtime, team), ["Newest", "Oldest"]);
    assert!(
        runtime
            .team_settled(team)
            .is_some_and(|state| !state.has_older())
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
        ["Newest", "Oldest"],
        "loaded cards stay until the fresh page lands"
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

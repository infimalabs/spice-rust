//! The team Work pane's stable task-card lanes.

use super::*;
use crate::runtime::TeamSettledState;

/// What the Settled lane reads: the team's loaded pages and their controls.
pub(super) struct Settled<'s, 'p, 'a> {
    pub state: Option<&'s TeamSettledState>,
    pub work: Option<&'p crate::work::WorkProjection<'a>>,
    pub controls: &'s super::super::team_settled::Controls,
}
pub(super) fn render_tasks(
    render: &FleetRender<'_, '_>,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    settled: Option<&super::team_settled::Controls>,
) -> Html {
    let runtime = render.runtime;
    let selected = (**render.selected_task).as_ref();
    let select = render.select_task;
    let submissions = render.submissions;
    let team_controls = render.team_controls;
    let work = render
        .work
        .map(|projection| crate::work::team_work(projection, team));
    let settled_state = runtime.team_settled(team.lane);
    let settled_work = settled_state
        .and_then(crate::runtime::TeamSettledState::pages)
        .map(crate::work::WorkProjection::from_board);
    let settled = settled.map(|controls| team_work::Settled {
        state: settled_state,
        work: settled_work.as_ref(),
        controls,
    });
    let policy = TeamLife::from(team.subscription).label();
    let work_summary = work.as_ref().map_or_else(
        || "Task state unavailable".to_owned(),
        |work| {
            format!(
                "{} open",
                work.up_next().len() + work.in_flight().len() + work.waiting().len()
            )
        },
    );
    let member_lanes = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let message_links =
        watchband::work_message_links(&member_lanes, runtime.messages().iter().map(AsRef::as_ref));
    let follow_message = {
        let team = team.lane;
        let team_controls = team_controls.clone();
        Callback::from(move |event: MouseEvent| {
            if super::card_target::remember_work_return(&event, team) {
                team_controls.dispatch(TeamControlAction::ShowChat { team });
            }
        })
    };
    let cards = work_card::Controls {
        selected,
        select,
        lanes: runtime.lanes(),
        density: Density::Compact,
        map: None,
        messages: Some(work_card::MessageControls {
            links: &message_links,
            team: team.lane,
            follow: &follow_message,
        }),
    };
    html! {
        <main class="team-task-workspace" aria-label="Team tasks">
            <header class="team-task-summary">
                <div>
                    <span class="team-task-eyebrow">{"Team work"}</span>
                    <strong>{work_summary}</strong>
                </div>
                <div
                    class="team-task-policy"
                    data-task-policy-extension=""
                    aria-label={format!("Task policy: {policy}")}
                >
                    <span>{"Task policy"}</span>
                    <strong>{policy}</strong>
                </div>
            </header>
            {render_team_work_lanes(work.as_ref(), team.lane, &cards, settled.as_ref())}
            {render_team_jobs(standings, team.lane)}
            {render_directive_ledger(
                runtime,
                team,
                standings,
                submissions,
                select,
                &message_links,
                &follow_message,
            )}
        </main>
    }
}

pub(super) fn render_team_work_lanes(
    work: Option<&crate::work::TeamWork<'_, '_>>,
    team: wire::CanonicalUuid,
    cards: &work_card::Controls<'_>,
    settled: Option<&Settled<'_, '_, '_>>,
) -> Html {
    html! {
        <div class="team-work-flow" aria-label="Team work lanes">
            <section
                class="team-task-section team-work-lane"
                data-team-work-lane="up-next"
                aria-labelledby={format!("up-next-{team}")}
            >
                <header>
                    <h3 id={format!("up-next-{team}")}>{"Up next"}</h3>
                    if work.is_some_and(|work| !work.up_next().is_empty()) {
                        <span>{work.map_or(0, |work| work.up_next().len())}</span>
                    }
                </header>
                if let Some(work) = work {
                    if work.up_next().is_empty() {
                        <p class="team-task-empty">{"No work is up next."}</p>
                    } else {
                        <div class="team-task-card-list">
                            {for work.up_next().iter().map(|task| work_card::render(task, cards))}
                        </div>
                    }
                } else {
                    <p class="team-task-empty" role="status">{"Task snapshot unavailable."}</p>
                }
            </section>
            <section
                class="team-task-section team-work-lane"
                data-team-work-lane="in-flight"
                aria-labelledby={format!("in-flight-{team}")}
            >
                <header>
                    <h3 id={format!("in-flight-{team}")}>{"In flight"}</h3>
                    if work.is_some_and(|work| !work.in_flight().is_empty()) {
                        <span>{work.map_or(0, |work| work.in_flight().len())}</span>
                    }
                </header>
                if let Some(work) = work {
                    if work.in_flight().is_empty() {
                        <p class="team-task-empty">{"No work is in flight."}</p>
                    } else {
                        <div class="team-task-card-list">
                            {for work.in_flight().iter().map(|task| work_card::render(task, cards))}
                        </div>
                    }
                } else {
                    <p class="team-task-empty" role="status">{"Task snapshot unavailable."}</p>
                }
            </section>
            <section
                class="team-task-section team-work-lane"
                data-team-work-lane="waiting"
                aria-labelledby={format!("waiting-{team}")}
            >
                <header>
                    <h3 id={format!("waiting-{team}")}>{"Waiting"}</h3>
                    if work.is_some_and(|work| !work.waiting().is_empty()) {
                        <span>{work.map_or(0, |work| work.waiting().len())}</span>
                    }
                </header>
                if let Some(work) = work {
                    if work.waiting().is_empty() {
                        <p class="team-task-empty">{"No work is waiting."}</p>
                    } else {
                        <div class="team-task-card-list">
                            {for work.waiting().iter().map(|(_, task)| work_card::render(task, cards))}
                        </div>
                    }
                } else {
                    <p class="team-task-empty" role="status">{"Task snapshot unavailable."}</p>
                }
            </section>
            if let Some(settled) = settled {
                {render_settled_lane(settled, team, cards)}
            }
        </div>
    }
}

pub(super) fn render_directive_ledger(
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    submissions: &SubmissionHandle,
    select: &Callback<wire::TaskStamp>,
    message_links: &watchband::WorkMessageLinks<'_>,
    follow_message: &Callback<MouseEvent>,
) -> Html {
    let members = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let member_lanes = members.iter().copied().collect::<BTreeSet<_>>();
    let local = submissions
        .queue
        .borrow()
        .ledger_submissions(&member_lanes)
        .into_iter()
        .map(
            |submission| watchband::directive_ledger::DirectiveLedgerLocal {
                fanout: submission.fanout,
                lane: submission.lane,
                authored_millis: submission.authored_millis,
                preview: submission
                    .message
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("Preview unavailable")
                    .trim()
                    .to_owned(),
                key: submission.key,
                delivered: submission.delivered,
            },
        )
        .collect::<Vec<_>>();
    let rows = watchband::directive_ledger::directive_ledger(&members, &local, runtime.inbox());
    let board = runtime.board();
    let heading = format!("direction-ledger-{}", team.lane);
    html! {
        <section class="team-task-section" aria-labelledby={heading.clone()}>
            <header>
                <h3 id={heading}>{"Directions"}</h3>
                if !rows.is_empty() {
                    <span>{rows.len()}</span>
                }
            </header>
            if rows.is_empty() {
                <p
                    class="team-task-empty"
                    role={runtime.inbox().is_none().then_some("status")}
                >
                    {if runtime.inbox().is_some() {
                        "No directions yet."
                    } else {
                        "Direction snapshot unavailable."
                    }}
                </p>
            } else {
                <ol class="team-direction-ledger">
                    {for rows.iter().map(|row| {
                        render_directive_row(
                            row,
                            standings,
                            board,
                            select,
                            message_links,
                            follow_message,
                        )
                    })}
                </ol>
            }
        </section>
    }
}

fn render_settled_finder(settled: &Settled<'_, '_, '_>, team: wire::CanonicalUuid) -> Html {
    let query_id = super::super::team_settled::query_id(team);
    let cutoff_id = super::super::team_settled::cutoff_id(team);
    let validation = settled.controls.validation.get(&team);
    let query_value = settled
        .controls
        .query
        .get(&team)
        .cloned()
        .unwrap_or_default();
    let cutoff_value = settled
        .controls
        .cutoff
        .get(&team)
        .cloned()
        .unwrap_or_default();
    let on_query = {
        let values = settled.controls.query.clone();
        Callback::from(move |event: InputEvent| {
            let input: web_sys::HtmlInputElement = event.target_unchecked_into();
            super::super::team_settled::update_input(&values, team, input.value());
        })
    };
    let on_cutoff = {
        let values = settled.controls.cutoff.clone();
        Callback::from(move |event: InputEvent| {
            let input: web_sys::HtmlInputElement = event.target_unchecked_into();
            super::super::team_settled::update_input(&values, team, input.value());
        })
    };
    let apply = {
        let apply = settled.controls.apply.clone();
        Callback::from(move |_: MouseEvent| apply.emit(team))
    };
    let clear = {
        let clear = settled.controls.clear.clone();
        Callback::from(move |_: MouseEvent| clear.emit(team))
    };
    html! {
        <>
            <div class="work-history-finder" role="search" aria-label="Find settled tasks">
                <label>
                    <span>{"Search"}</span>
                    <input
                        id={query_id}
                        type="search"
                        maxlength={wire::intent::TASK_HISTORY_QUERY_BYTES_MAX.to_string()}
                        placeholder="Handle, title, project, or description"
                        value={query_value}
                        oninput={on_query}
                    />
                </label>
                <label>
                    <span>{"Completed by"}</span>
                    <input
                        id={cutoff_id}
                        type="date"
                        value={cutoff_value}
                        oninput={on_cutoff}
                    />
                </label>
                <div class="work-history-filter-actions canvas-control-rail team-mode-rail">
                    <button type="button" class="team-mode" onclick={apply}>{"Apply"}</button>
                    <button type="button" class="team-mode" onclick={clear}>{"Clear"}</button>
                </div>
            </div>
            if let Some(issue) = validation {
                <p class="form-refusal work-history-filter-issue" role="alert">{issue}</p>
            }
        </>
    }
}

/// The work the team settled most recently, newest first, one page at a time.
///
/// Loaded cards stay mounted while a stale page is read again, so a refresh
/// replaces them in place instead of blanking the lane.
fn render_settled_lane(
    settled: &Settled<'_, '_, '_>,
    team: wire::CanonicalUuid,
    cards: &work_card::Controls<'_>,
) -> Html {
    let state = settled.state;
    let total = state.and_then(TeamSettledState::total);
    let loading = state.is_some_and(TeamSettledState::loading);
    let issue = state.and_then(TeamSettledState::issue);
    let tasks = settled
        .work
        .map_or(&[][..], crate::work::WorkProjection::tasks);
    let newer = {
        let newer = settled.controls.newer.clone();
        Callback::from(move |_: MouseEvent| newer.emit(team))
    };
    let older = {
        let older = settled.controls.older.clone();
        Callback::from(move |_: MouseEvent| older.emit(team))
    };
    let retry = {
        let retry = settled.controls.retry.clone();
        Callback::from(move |_: MouseEvent| retry.emit(team))
    };
    html! {
        <section
            class="team-task-section team-work-lane team-work-history"
            data-team-work-lane="settled"
            aria-labelledby={format!("settled-{team}")}
            aria-busy={loading.to_string()}
        >
            <header>
                <h3 id={format!("settled-{team}")}>{"Settled"}</h3>
                if let Some(total) = total.filter(|total| *total > 0) {
                    <span>{total}</span>
                }
            </header>
            {render_settled_finder(settled, team)}
            if loading {
                <p class="work-board-state" role="status">{"Loading settled page…"}</p>
            }
            if !tasks.is_empty() {
                <div class="team-task-card-list work-history-list">
                    {for tasks.iter().map(|task| work_card::render(task, cards))}
                </div>
            } else if total == Some(0) {
                <p class="team-task-empty">{
                    if state.is_some_and(TeamSettledState::filtered) {
                        "No settled work matches these filters."
                    } else {
                        "No work has settled."
                    }
                }</p>
            } else if state.is_none() {
                <p class="team-task-empty" role="status">{"Loading settled work…"}</p>
            }
            if let Some(issue) = issue {
                <div class="work-history-actions">
                    <p class="form-refusal" role="alert">{issue}</p>
                    <span class="canvas-control-rail team-mode-rail">
                        <button type="button" class="team-mode" onclick={retry}>{"Retry"}</button>
                    </span>
                </div>
            }
            if let Some(state) = state.filter(|state| state.total().is_some()) {
                <div class="work-history-pagination">
                    <div class="work-history-range" role="status">
                        <strong>{format!(
                            "{}–{} of {}",
                            state.first_result(),
                            state.last_result(),
                            state.total().unwrap_or(0),
                        )}</strong>
                        <span>{match (state.has_newer(), state.has_older()) {
                            (false, false) => "Newest and oldest page",
                            (false, true) => "Newest page",
                            (true, false) => "Oldest page",
                            (true, true) => "Between newest and oldest",
                        }}</span>
                    </div>
                    <div
                        class="work-history-page-actions canvas-control-rail team-mode-rail"
                        role="group"
                        aria-label="Settled history pages"
                        aria-busy={loading.to_string()}
                    >
                        <button
                            type="button"
                            class="team-mode"
                            disabled={loading || !state.has_newer()}
                            onclick={newer}
                        >
                            {"Newer"}
                        </button>
                        <button
                            type="button"
                            class="team-mode"
                            disabled={loading || !state.has_older()}
                            onclick={older}
                        >
                            {"Older"}
                        </button>
                    </div>
                </div>
            }
        </section>
    }
}

fn render_directive_row(
    row: &watchband::directive_ledger::DirectiveLedgerRow,
    standings: &[Standing],
    board: Option<&crate::runtime::BoardView>,
    select: &Callback<wire::TaskStamp>,
    message_links: &watchband::WorkMessageLinks<'_>,
    follow_message: &Callback<MouseEvent>,
) -> Html {
    let task_handles = board.map_or_else(BTreeSet::new, |board| {
        row.keys
            .iter()
            .filter_map(|key| board.quote_tasks.get(key))
            .flatten()
            .collect::<BTreeSet<_>>()
    });
    let tasks = board.into_iter().flat_map(|board| {
        board.tasks.iter().filter_map(|task| {
            task_handles
                .contains(&task.statement.body.handle)
                .then_some((&task.statement.body.handle, &task.statement.body.born))
        })
    });
    let message_target = message_links
        .directive(&row.keys)
        .map(agent_message_source_id);
    let return_id = message_target
        .as_ref()
        .map(|_| format!("team-work-directive-{}", row.identity));
    html! {
        <li
            key={row.identity.to_string()}
            id={return_id}
            class="team-direction-ledger-row"
            tabindex={message_target.as_ref().map(|_| "-1")}
            data-card-return-origin={message_target.as_ref().map(|_| "work")}
            data-directive={row.identity.to_string()}
        >
            <div class="team-direction-ledger-heading">
                <span class="team-direction-preview">{row.preview.clone()}</span>
                <span class="team-direction-heading-actions">
                    if let Some(target) = &message_target {
                        <a
                            class="work-message-link"
                            href={crate::view::card_href(target)}
                            aria-label="Open acknowledging or declining message in Chat"
                            onclick={follow_message.clone()}
                        >{"Chat"}</a>
                    }
                    <Age authored={row.authored_millis} class="team-direction-age" />
                </span>
            </div>
            <ul class="team-direction-recipients" aria-label="Recipients">
                {for standings.iter().filter_map(|standing| {
                    let state = row.recipients.get(&standing.lane)?;
                    Some(html! {
                        <li
                            key={standing.lane.to_string()}
                            data-agent-lane={standing.lane.to_string()}
                            data-accent={standing.accent.to_string()}
                            data-direction-state={state.key()}
                        >
                            <span class="team-task-member">
                                <span class="agent-presence" aria-hidden="true"></span>
                                <strong class="team-task-member-name">{standing.label.clone()}</strong>
                            </span>
                            <span class="team-direction-state">{state.label()}</span>
                        </li>
                    })
                })}
            </ul>
            <div class="team-direction-tasks">
                {for tasks.map(|(handle, stamp)| {
                    let open = {
                        let select = select.clone();
                        let stamp = stamp.clone();
                        Callback::from(move |_| select.emit(stamp.clone()))
                    };
                    html! {
                        <button
                            key={handle.to_string()}
                            type="button"
                            class="team-direction-task-chip"
                            data-task-handle={handle.to_string()}
                            aria-label={format!("Open {handle} details")}
                            onclick={open}
                        >
                            <span class="team-direction-task-chip-label">{handle.to_string()}</span>
                        </button>
                    }
                })}
            </div>
        </li>
    }
}

/// Every command the team's members keep, oldest first, one keyed row each.
pub(super) fn render_team_jobs(standings: &[Standing], team: wire::CanonicalUuid) -> Html {
    let jobs = watchband::team_jobs(standings);
    html! {
        <section
            class="team-task-section team-jobs"
            aria-labelledby={format!("kept-jobs-{team}")}
        >
            <header>
                <h3 id={format!("kept-jobs-{team}")}>{"Kept jobs"}</h3>
                if !jobs.is_empty() {
                    <span>{jobs.len()}</span>
                }
            </header>
            if jobs.is_empty() {
                <p class="team-task-empty">{"No kept jobs."}</p>
            } else {
                <table class="team-job-table">
                    <thead>
                        <tr>
                            <th scope="col">{"Member"}</th>
                            <th scope="col">{"Started"}</th>
                            <th scope="col">{"Command"}</th>
                            <th scope="col">{"Standing"}</th>
                        </tr>
                    </thead>
                    <tbody>
                        {for jobs.iter().map(render_team_job)}
                    </tbody>
                </table>
            }
        </section>
    }
}

fn render_team_job(row: &watchband::TeamJob<'_>) -> Html {
    let standing = row.job.standing.as_str();
    html! {
        <tr
            key={row.key()}
            class="team-job-row"
            data-agent-lane={row.member.lane.to_string()}
            data-accent={row.member.accent.to_string()}
            data-job={row.job.id.clone()}
            data-job-standing={standing}
        >
            <td>
                <span class="team-task-member">
                    <span class="agent-presence" aria-hidden="true"></span>
                    <strong class="team-task-member-name">{row.member.label.clone()}</strong>
                </span>
            </td>
            <td>{lease_age(row.started_millis(), "team-job-age", "unknown")}</td>
            <td class="team-job-command" title={row.job.command.clone()}>
                <code>{row.job.command.clone()}</code>
            </td>
            <td class="team-job-standing">{standing}</td>
        </tr>
    }
}

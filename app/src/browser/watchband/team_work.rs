//! The team Work pane's three stable task-card lanes.

use super::*;

pub(super) fn render_team_work_lanes(
    work: Option<&crate::work::TeamWork<'_, '_>>,
    team: wire::CanonicalUuid,
    cards: &work_card::Controls<'_>,
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
        </div>
    }
}

pub(super) fn render_directive_ledger(
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    submissions: &SubmissionHandle,
    select: &Callback<wire::TaskStamp>,
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
                        render_directive_row(row, standings, board, select)
                    })}
                </ol>
            }
        </section>
    }
}

fn render_directive_row(
    row: &watchband::directive_ledger::DirectiveLedgerRow,
    standings: &[Standing],
    board: Option<&crate::runtime::BoardView>,
    select: &Callback<wire::TaskStamp>,
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
    html! {
        <li
            key={row.identity.to_string()}
            class="team-direction-ledger-row"
            data-directive={row.identity.to_string()}
        >
            <div class="team-direction-ledger-heading">
                <span class="team-direction-preview">{row.preview.clone()}</span>
                <Age authored={row.authored_millis} class="team-direction-age" />
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
                            {handle.to_string()}
                        </button>
                    }
                })}
            </div>
        </li>
    }
}

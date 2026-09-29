//! The team Work pane's stable task-card lanes.

use super::*;
use crate::runtime::TeamSettledState;

/// What the Settled lane reads: the team's loaded pages and their controls.
pub(super) struct Settled<'s, 'p, 'a> {
    pub state: Option<&'s TeamSettledState>,
    pub work: Option<&'p crate::work::WorkProjection<'a>>,
    pub controls: &'s super::super::team_settled::Controls,
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
            class="team-task-section team-work-lane"
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
            if !tasks.is_empty() {
                <div class="team-task-card-list">
                    {for tasks.iter().map(|task| work_card::render(task, cards))}
                </div>
            } else if total == Some(0) {
                <p class="team-task-empty">{"No work has settled."}</p>
            } else if issue.is_none() {
                <p class="team-task-empty" role="status">{"Loading settled work…"}</p>
            }
            if let Some(issue) = issue {
                <div class="work-history-actions">
                    <p class="form-refusal" role="alert">{issue}</p>
                    <span class="canvas-control-rail team-mode-rail">
                        <button type="button" class="team-mode" onclick={retry}>{"Retry"}</button>
                    </span>
                </div>
            } else if state.is_some_and(TeamSettledState::has_older) {
                <span class="canvas-control-rail team-mode-rail team-work-older">
                    <button type="button" class="team-mode" disabled={loading} onclick={older}>
                        {if loading { "Loading older…" } else { "Show older" }}
                    </button>
                </span>
            }
        </section>
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

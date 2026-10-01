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
        creator_context: Some(&follow_message),
        creator_messages: runtime.messages(),
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
            {render_team_jobs(standings, team.lane)}
            {render_team_work_lanes(work.as_ref(), team.lane, &cards, settled.as_ref())}
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
        <div class="team-work-flow">
            <div class="work-lanes team-work-lanes" aria-label="Team work lanes">
                {team_lane(team, "up-next", "ready", "Up next", "No work is up next.", work.map(|work| {
                    work.up_next().iter().map(|task| work_card::render(task, cards)).collect()
                }))}
                {team_lane(team, "in-flight", "in-flight", "In flight", "No work is in flight.", work.map(|work| {
                    work.in_flight().iter().map(|task| work_card::render(task, cards)).collect()
                }))}
                {team_lane(team, "waiting", "blocked", "Waiting", "No work is waiting.", work.map(|work| {
                    work.waiting().iter().map(|(_, task)| work_card::render(task, cards)).collect()
                }))}
            </div>
            if let Some(settled) = settled {
                {render_settled_lane(settled, team, cards)}
            }
        </div>
    }
}

/// One open-work state as a row: a label and count in the shared rail beside a
/// horizontal card track, the same grammar as the global Work lanes.
fn team_lane(
    team: wire::CanonicalUuid,
    key: &'static str,
    state: &'static str,
    label: &'static str,
    empty: &'static str,
    tasks: Option<Vec<Html>>,
) -> Html {
    let heading = format!("{key}-{team}");
    // Without a snapshot the count is unknown, so the rail shows none.
    let count = tasks.as_ref().map(Vec::len);
    html! {
        <section
            class="work-lane team-work-lane"
            data-work-lane={state}
            data-team-work-lane={key}
            aria-labelledby={heading.clone()}
        >
            <header class="work-lane-heading">
                <h3 id={heading}>{label}</h3>
                if let Some(count) = count {
                    <span aria-label={format!("{count} tasks")}>{count}</span>
                }
            </header>
            <div
                class="work-lane-track"
                tabindex="0"
                role="region"
                aria-label={format!("{label} task row")}
            >
                {match tasks {
                    None => html! {
                        <p class="work-lane-empty" role="status">{"Task snapshot unavailable."}</p>
                    },
                    Some(tasks) if tasks.is_empty() => html! { <p class="work-lane-empty">{empty}</p> },
                    Some(tasks) => html! { <div class="work-card-list">{for tasks}</div> },
                }}
            </div>
        </section>
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
                body: submission.message,
                attachments: submission.attachments,
                key: submission.key,
                delivered: submission.delivered,
            },
        )
        .collect::<Vec<_>>();
    let rows = watchband::directive_ledger::directive_ledger(&members, &local, runtime.inbox());
    let board = runtime.board();
    let heading = format!("direction-ledger-{}", team.lane);
    let history = crate::view::CanvasRoute::Directions {
        focus: Some(crate::view::DirectionsFocus::Team(team.lane)),
    };
    html! {
        <section class="team-task-section" aria-labelledby={heading.clone()}>
            <header>
                <h3 id={heading.clone()}>{"Directions"}</h3>
                if !rows.is_empty() {
                    <span>{rows.len()}</span>
                }
                <a
                    class="team-direction-history-link"
                    href={format!("#{}", history.path())}
                    aria-label="Open this team's direction history in Work"
                >{"History"}</a>
            </header>
            <div class="team-entry-region">
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
                    <ol class="team-direction-ledger" role="feed" aria-labelledby={heading.clone()}>
                        {for rows.iter().map(|row| {
                            super::super::direction_card::render(
                                &direction_card(row, team.lane, standings, message_links),
                                &super::super::direction_card::Controls {
                                    board,
                                    select,
                                    follow: follow_message,
                                },
                            )
                        })}
                    </ol>
                }
            </div>
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
            {crate::browser::archive_status::render(&crate::browser::archive_status::ArchiveStatus {
                range: state.and_then(|state| {
                    Some((state.first_result(), state.last_result(), state.total()?))
                }),
                has_newer: state.is_some_and(TeamSettledState::has_newer),
                has_older: state.is_some_and(TeamSettledState::has_older),
                loading: state.is_none() || loading,
                issue,
                pages_label: "Settled history pages",
                newer,
                older,
                retry,
            })}
            <div class="team-entry-region">
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
                }
            </div>
        </section>
    }
}

/// One team ledger row as the shared direction card.
fn direction_card<'a>(
    row: &'a watchband::directive_ledger::DirectiveLedgerRow,
    team: wire::CanonicalUuid,
    standings: &[Standing],
    message_links: &watchband::WorkMessageLinks<'_>,
) -> super::super::direction_card::Card<'a> {
    use super::super::direction_card::{Author, Card, Recipient, Recipients};
    let author = row.author.map_or(Author::Operator, |lane| {
        standings
            .iter()
            .find(|standing| standing.lane == lane)
            .map_or_else(
                || Author::Lane {
                    label: format!("lane {}", watchband::short(lane)),
                    accent: None,
                },
                |standing| Author::Lane {
                    label: standing.label.clone(),
                    accent: Some(standing.accent.to_string()),
                },
            )
    });
    let recipients = standings
        .iter()
        .filter_map(|standing| {
            let state = row.recipients.get(&standing.lane)?;
            Some(Recipient {
                key: standing.lane.to_string(),
                lane: standing.lane,
                label: standing.label.clone(),
                accent: Some(standing.accent.to_string()),
                state_key: state.key(),
                state_label: state.label(),
                target: row
                    .recipient_keys
                    .get(&standing.lane)
                    .and_then(|key| message_links.directive_key(key))
                    .map(agent_message_source_id),
                retirement: None,
            })
        })
        .collect();
    Card {
        identity: row.identity,
        id: crate::view::DirectionTarget::new(team, row.identity).element_id(),
        body: &row.body,
        complete: row.body_complete,
        attachments: &row.attachments,
        author,
        keys: row.keys.iter().map(String::as_str).collect(),
        recipients: Recipients::Team(recipients),
        authored_millis: row.authored_millis,
    }
}

/// Jobs shown before the table scrolls. `.team-job-scroll` in work.css caps
/// the table at this many rows plus half the next (`--team-job-visible`); keep
/// the two equal. Canvas's `style-src 'self'` refuses an inline value.
const KEPT_JOBS_VISIBLE: usize = 5;

/// Every command the team's members keep, oldest first, one keyed row each.
/// Up to [`KEPT_JOBS_VISIBLE`] rows show in full; beyond that the table
/// scrolls inside a keyboard-reachable region under pinned headers. The
/// region stands with no jobs too, reserving one job's height, so the first
/// job lands without moving what follows.
pub(super) fn render_team_jobs(standings: &[Standing], team: wire::CanonicalUuid) -> Html {
    let jobs = watchband::team_jobs(standings);
    let heading = format!("kept-jobs-{team}");
    let scrolls = jobs.len() > KEPT_JOBS_VISIBLE;
    html! {
        <section class="team-task-section team-jobs" aria-labelledby={heading.clone()}>
            <header>
                <h3 id={heading.clone()}>{"Kept jobs"}</h3>
                if !jobs.is_empty() {
                    <span>{jobs.len()}</span>
                }
            </header>
            <div
                class="team-job-scroll"
                tabindex={scrolls.then_some("0")}
                role={scrolls.then_some("region")}
                aria-labelledby={scrolls.then_some(heading)}
            >
                if jobs.is_empty() {
                    <p class="team-job-empty">{"No kept jobs."}</p>
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
            </div>
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

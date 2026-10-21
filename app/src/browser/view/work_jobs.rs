//! Running resident jobs represented inside the unified task overview.
//!
//! The panel is global: it lists every job the resident's lanes still run,
//! names the task each runs under, and leads with the jobs under the sheet's
//! selected tasks so the reader finds their own first.

use super::super::clock::Age;
use super::*;
use crate::runtime::{LaneMember, LaneRow};
use crate::work::{WorkProjection, WorkTask};
use wire::TaskStamp;

/// Rows shown before the existing Work table becomes scrollable.
const KEPT_JOBS_VISIBLE: usize = 5;

pub(super) struct WorkJobsRender<'a, 'work> {
    pub lanes: &'a [LaneRow],
    pub selected: &'a [TaskStamp],
    pub work: &'a WorkProjection<'work>,
    /// Opens a job's task: focused when selected, otherwise joined to the sheet.
    pub activate_task: &'a Callback<TaskStamp>,
}

pub(super) fn render(render: &WorkJobsRender<'_, '_>) -> Html {
    let selected = render.selected;
    let mut jobs = render
        .lanes
        .iter()
        .flat_map(|team| {
            team.members
                .iter()
                .enumerate()
                .flat_map(move |(position, member)| {
                    member.jobs.iter().map(move |job| ResidentJob {
                        member,
                        accent: team.accent(position),
                        job,
                        selected: crate::watchband::job_matches_tasks(job, selected),
                    })
                })
        })
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| {
        (!left.selected, &left.job.id, left.member.lane).cmp(&(
            !right.selected,
            &right.job.id,
            right.member.lane,
        ))
    });

    const HEADING: &str = "work-overview-running-jobs";
    let scrolls = jobs.len() > KEPT_JOBS_VISIBLE;
    html! {
        <section class="section team-jobs global-jobs work-overview-jobs" aria-labelledby={HEADING}>
            <div class="section-heading">
                <h3 id={HEADING}>{"Running jobs"}</h3>
                <span class="section-count" data-shown={(!jobs.is_empty()).to_string()}>
                    {jobs.len()}
                </span>
            </div>
            <div
                class="team-job-scroll"
                tabindex={scrolls.then_some("0")}
                role={scrolls.then_some("region")}
                aria-labelledby={scrolls.then_some(HEADING)}
            >
                if jobs.is_empty() {
                    <p class="team-job-empty">{"No running or awaiting-delivery jobs."}</p>
                } else {
                    <table class="team-job-table">
                        <thead>
                            <tr>
                                <th scope="col">{"Agent"}</th>
                                <th scope="col">{"Task"}</th>
                                <th scope="col">{"Started"}</th>
                                <th scope="col">{"Command"}</th>
                                <th scope="col">{"Standing"}</th>
                            </tr>
                        </thead>
                        <tbody>{for jobs.iter().map(|job| render_job(render, job))}</tbody>
                    </table>
                }
            </div>
        </section>
    }
}

#[derive(Clone, Copy)]
struct ResidentJob<'a> {
    member: &'a LaneMember,
    accent: usize,
    job: &'a wire::intent::LaneJob,
    /// Whether the job runs under one of the sheet's selected tasks.
    selected: bool,
}

fn render_job(render: &WorkJobsRender<'_, '_>, row: &ResidentJob<'_>) -> Html {
    let job = row.job;
    let lane = row.member.lane;
    let standing = job.standing.as_str();
    let started = job.started_micros().map(|micros| micros.div_euclid(1_000));
    let label =
        crate::watchband::agent_message_label(&row.member.worktree, row.member.name.as_deref());
    html! {
        <tr
            key={format!("{lane}/{}", job.id)}
            class="team-job-row"
            data-agent-lane={lane.to_string()}
            data-accent={row.accent.to_string()}
            data-job={job.id.clone()}
            data-job-selected={row.selected.to_string()}
            data-job-standing={standing}
        >
            <td>
                <span class="team-task-member">
                    <span class="app-presence-pip app-presence-pip--work" aria-hidden="true"></span>
                    <strong class="team-task-member-name">{label.to_owned()}</strong>
                </span>
            </td>
            <td class="team-job-task">{render_task(render, job.task.as_ref())}</td>
            <td>{started_age(started)}</td>
            <td class="team-job-command" title={job.command.clone()}>
                <code>{job.command.clone()}</code>
            </td>
            <td class="team-job-standing">{standing}</td>
        </tr>
    }
}

/// The task a job runs under as a control opening its sheet, or a dash.
fn render_task(render: &WorkJobsRender<'_, '_>, task: Option<&TaskStamp>) -> Html {
    let Some(task) = task else {
        return html! { <span class="muted" aria-label="No task">{"—"}</span> };
    };
    let handle = render
        .work
        .tasks()
        .iter()
        .map(WorkTask::row)
        .find(|row| row.statement.body.born == *task)
        .map_or_else(
            || task.to_string(),
            |row| row.statement.body.handle.to_string(),
        );
    let activate = {
        let open = render.activate_task.clone();
        let task = task.clone();
        Callback::from(move |_: MouseEvent| open.emit(task.clone()))
    };
    html! {
        <button type="button" class="team-job-task-link" data-task-stamp={task.to_string()} onclick={activate}>
            {handle}
        </button>
    }
}

fn started_age(started: Option<i64>) -> Html {
    match started {
        Some(authored) => html! { <Age {authored} class="team-job-age" /> },
        None => html! { "unknown" },
    }
}

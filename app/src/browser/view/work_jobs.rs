//! Running resident jobs represented inside the unified task overview.

use super::super::clock::Age;
use super::*;
use crate::runtime::{LaneMember, LaneRow};

/// Rows shown before the existing Work table becomes scrollable.
const KEPT_JOBS_VISIBLE: usize = 5;

pub(super) fn render(lanes: &[LaneRow]) -> Html {
    let mut jobs = lanes
        .iter()
        .flat_map(|team| {
            team.members
                .iter()
                .enumerate()
                .flat_map(move |(position, member)| {
                    member.jobs.iter().map(move |job| ResidentJob {
                        member,
                        accent: crate::watchband::accent(team.members.len(), position),
                        job,
                    })
                })
        })
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| {
        (&left.job.id, left.member.lane).cmp(&(&right.job.id, right.member.lane))
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
                                <th scope="col">{"Started"}</th>
                                <th scope="col">{"Command"}</th>
                                <th scope="col">{"Standing"}</th>
                            </tr>
                        </thead>
                        <tbody>{for jobs.iter().map(render_job)}</tbody>
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
}

fn render_job(row: &ResidentJob<'_>) -> Html {
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
            data-job-standing={standing}
        >
            <td>
                <span class="team-task-member">
                    <span class="app-presence-pip app-presence-pip--work" aria-hidden="true"></span>
                    <strong class="team-task-member-name">{label.to_owned()}</strong>
                </span>
            </td>
            <td>{started_age(started)}</td>
            <td class="team-job-command" title={job.command.clone()}>
                <code>{job.command.clone()}</code>
            </td>
            <td class="team-job-standing">{standing}</td>
        </tr>
    }
}

fn started_age(started: Option<i64>) -> Html {
    match started {
        Some(authored) => html! { <Age {authored} class="team-job-age" /> },
        None => html! { "unknown" },
    }
}

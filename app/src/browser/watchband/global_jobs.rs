//! Global resident job table shared by Work scopes.

use super::*;

/// Jobs shown before the table scrolls. `.team-job-scroll` in work.css caps
/// the table at this many rows plus half the next (`--team-job-visible`); keep
/// the two equal. The app's `style-src 'self'` refuses an inline value.
const KEPT_JOBS_VISIBLE: usize = 5;

/// Every command every resident lane keeps, independent of Work scope.
///
/// The topology rows are borrowed directly so global Work does not clone all
/// lane state merely to render the jobs it already owns.
pub(in crate::browser) fn render_global_jobs(lanes: &[crate::runtime::LaneRow]) -> Html {
    let mut jobs = lanes
        .iter()
        .flat_map(|team| {
            team.members
                .iter()
                .enumerate()
                .flat_map(move |(position, member)| {
                    member.jobs.iter().map(move |job| ResidentJob {
                        member,
                        accent: watchband::accent(team.members.len(), position),
                        job,
                    })
                })
        })
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| {
        (&left.job.id, left.member.lane).cmp(&(&right.job.id, right.member.lane))
    });
    const HEADING: &str = "global-running-jobs";
    html! {
        <section class="section team-jobs global-jobs" aria-labelledby={HEADING}>
            <div class="section-heading">
                <h2 id={HEADING}>{"Running jobs"}</h2>
                <span class="section-count" data-shown={(!jobs.is_empty()).to_string()}>
                    {jobs.len()}
                </span>
            </div>
            {render_job_scroll(
                jobs.len(),
                jobs.iter().map(render_resident_job),
                HEADING,
                "Agent",
                "No running or awaiting-delivery jobs.",
            )}
        </section>
    }
}

#[derive(Clone, Copy)]
struct ResidentJob<'a> {
    member: &'a crate::runtime::LaneMember,
    accent: usize,
    job: &'a wire::intent::LaneJob,
}

fn render_job_scroll(
    count: usize,
    rows: impl IntoIterator<Item = Html>,
    heading: &str,
    identity_heading: &'static str,
    empty: &'static str,
) -> Html {
    let scrolls = count > KEPT_JOBS_VISIBLE;
    html! {
        <div
            class="team-job-scroll"
            tabindex={scrolls.then_some("0")}
            role={scrolls.then_some("region")}
            aria-labelledby={scrolls.then(|| heading.to_owned())}
        >
            if count == 0 {
                <p class="team-job-empty">{empty}</p>
            } else {
                <table class="team-job-table">
                    <thead>
                        <tr>
                            <th scope="col">{identity_heading}</th>
                            <th scope="col">{"Started"}</th>
                            <th scope="col">{"Command"}</th>
                            <th scope="col">{"Standing"}</th>
                        </tr>
                    </thead>
                    <tbody>{for rows}</tbody>
                </table>
            }
        </div>
    }
}

fn render_resident_job(row: &ResidentJob<'_>) -> Html {
    render_job(
        row.member.lane,
        watchband::agent_message_label(&row.member.worktree, row.member.name.as_deref()),
        row.accent,
        row.job,
    )
}

fn render_job(
    lane: wire::CanonicalUuid,
    label: &str,
    accent: usize,
    job: &wire::intent::LaneJob,
) -> Html {
    let standing = job.standing.as_str();
    let started = job.started_micros().map(|micros| micros.div_euclid(1_000));
    html! {
        <tr
            key={format!("{lane}/{}", job.id)}
            class="team-job-row"
            data-agent-lane={lane.to_string()}
            data-accent={accent.to_string()}
            data-job={job.id.clone()}
            data-job-standing={standing}
        >
            <td>
                <span class="team-task-member">
                    <span class="agent-presence" aria-hidden="true"></span>
                    <strong class="team-task-member-name">{label.to_owned()}</strong>
                </span>
            </td>
            <td>{lease_age(started, "team-job-age", "unknown")}</td>
            <td class="team-job-command" title={job.command.clone()}>
                <code>{job.command.clone()}</code>
            </td>
            <td class="team-job-standing">{standing}</td>
        </tr>
    }
}

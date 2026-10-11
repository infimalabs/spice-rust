//! Every command every lane keeps, as one dense board at the tail of the
//! global Spice menu.
//!
//! Rows run oldest first by start key, so a starting job joins at the end
//! and long-lived services settle at the top while churn stays at the tail.
//! Each cell has a fixed width and each row a fixed height, so a standing
//! change or an age tick rewrites text without moving a box.

use super::super::clock::Age;
use super::*;
use crate::watchband::{Standing, TeamJob, team_jobs};
use wire::intent::{LaneJobKind, LaneJobStanding};

const HEADING: &str = "spice-jobs-heading";

/// The kinds in census order, each with the glyph its rows lead with while
/// running and once ended unheard.
const KINDS: [(LaneJobKind, &str, &str, &str); 3] = [
    (LaneJobKind::Command, "▶", "▷", "commands"),
    (LaneJobKind::Service, "◆", "◇", "services"),
    (LaneJobKind::Watch, "●", "○", "watches"),
];

fn glyph(kind: LaneJobKind, standing: LaneJobStanding) -> &'static str {
    let (_, running, unheard, _) = KINDS
        .iter()
        .find(|(listed, ..)| *listed == kind)
        .expect("every kind has a glyph");
    match standing {
        LaneJobStanding::Running => running,
        LaneJobStanding::Unheard => unheard,
    }
}

pub(super) fn render(standings: Option<&[Standing]>) -> Html {
    let Some(standings) = standings else {
        return html! {
            <section
                class="app-sheet-section app-spice-jobs"
                aria-labelledby={HEADING}
                data-state="loading"
            >
                <div class="section-heading">
                    <h2 id={HEADING}>{"Jobs"}</h2>
                    <span class="section-count">{"Waiting for agent state…"}</span>
                </div>
            </section>
        };
    };
    let jobs = team_jobs(standings);
    html! {
        <section class="app-sheet-section app-spice-jobs" aria-labelledby={HEADING} data-state="ready">
            <div class="section-heading">
                <h2 id={HEADING}>{"Jobs"}</h2>
                {census(&jobs)}
            </div>
            if jobs.is_empty() {
                <p class="app-empty-state">{"No jobs running or awaiting delivery."}</p>
            } else {
                <ol class="app-spice-job-board">{for jobs.iter().map(render_job)}</ol>
            }
        </section>
    }
}

/// Every kind's count, zero included, then the unheard count: a legend for
/// the row glyphs whose slots never come and go.
fn census(jobs: &[TeamJob<'_>]) -> Html {
    let unheard = jobs
        .iter()
        .filter(|row| row.job.standing == LaneJobStanding::Unheard)
        .count();
    html! {
        <span class="section-count app-spice-job-census">
            {for KINDS.iter().map(|(kind, running, _, plural)| {
                let count = jobs.iter().filter(|row| row.job.kind == *kind).count();
                html! {
                    <span data-job-kind={kind.as_str()} title={format!("{count} {plural}")}>
                        <span aria-hidden="true">{*running}</span>
                        <span class="visually-hidden">{format!("{plural}: ")}</span>
                        {count}
                    </span>
                }
            })}
            <span data-job-standing="unheard" title={format!("{unheard} ended unheard")}>
                {unheard}{" unheard"}
            </span>
        </span>
    }
}

fn render_job(row: &TeamJob<'_>) -> Html {
    let job = row.job;
    let kind = job.kind.as_str();
    let standing = job.standing.as_str();
    let label = row.member.label.clone();
    html! {
        <li
            key={row.key()}
            class="app-spice-job"
            data-agent-lane={row.member.lane.to_string()}
            data-accent={row.member.accent.to_string()}
            data-job={job.id.clone()}
            data-job-kind={kind}
            data-job-standing={standing}
            title={format!("{label} · {kind} · {standing}\n{}", job.command)}
        >
            <span class="app-spice-job-glyph" role="img" aria-label={format!("{kind}, {standing}")}>
                {glyph(job.kind, job.standing)}
            </span>
            <span class="app-spice-job-agent">{label}</span>
            {match row.started_millis() {
                Some(authored) => html! { <Age {authored} class="app-spice-job-age" /> },
                None => html! {
                    <span
                        class="app-spice-job-age"
                        data-age-slot={crate::watchband::AGE_SLOT_WIDTH.to_string()}
                    >{"?"}</span>
                },
            }}
            <code class="app-spice-job-command">{job.command.clone()}</code>
        </li>
    }
}

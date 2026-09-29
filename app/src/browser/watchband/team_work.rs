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

//! Global Work Directions: one responsive mosaic over the resident history read.

use std::collections::BTreeSet;
use std::rc::Rc;

use super::*;
use crate::browser::direction_card::{self, Author, Card, Recipient, Recipients};
use crate::browser::direction_history::Controls;
use crate::runtime::{DirectionHistoryState, LaneRow, LaneView};
use wire::Disposition;
use wire::intent::{DirectionGroup, DirectionRecipient};

const STATES: [(Disposition, &str); 6] = [
    (Disposition::Live, "Waiting"),
    (Disposition::Acked, "Acknowledged"),
    (Disposition::Declined, "Declined"),
    (Disposition::Lapsed, "Lapsed"),
    (Disposition::DeadLettered, "Dead-lettered"),
    (Disposition::Canceled, "Canceled"),
];

pub(super) fn render(
    state: &DirectionHistoryState,
    topology: Option<&LaneView>,
    board: Option<&crate::runtime::BoardView>,
    messages: &[Rc<wire::AgentMessage>],
    controls: &Controls,
    select_task: &Callback<wire::TaskStamp>,
) -> Html {
    let range = state
        .total()
        .map(|total| (state.first_result(), state.last_result(), total));
    let member_lanes = topology
        .into_iter()
        .flat_map(|topology| topology.lanes.iter())
        .flat_map(|team| team.members.iter())
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let message_links =
        crate::watchband::work_message_links(&member_lanes, messages.iter().map(AsRef::as_ref));
    let follow = Callback::from(|event: MouseEvent| {
        super::super::card_target::remember_global_work_return(&event);
    });
    let card_controls = direction_card::Controls {
        board,
        select: select_task,
        follow: &follow,
    };
    html! {
        <section
            id={crate::view::DIRECTIONS_SECTION_ID}
            class="work-directions"
            tabindex="-1"
            aria-labelledby="work-directions-heading"
        >
            <header class="work-directions-heading">
                <div>
                    <span class="team-task-eyebrow">{"Operator history"}</span>
                    <h2 id="work-directions-heading">{"Directions"}</h2>
                </div>
                <span class="work-history-count">{crate::browser::archive_status::count(range)}</span>
            </header>
            {render_finder(topology, controls)}
            if let Some(issue) = controls.validation.as_ref() {
                <p class="form-refusal work-history-filter-issue" role="alert">{issue}</p>
            }
            {crate::browser::archive_status::render(&crate::browser::archive_status::ArchiveStatus {
                range,
                has_newer: state.has_newer(),
                has_older: state.has_older(),
                loading: state.loading(),
                issue: state.issue(),
                pages_label: "Direction history pages",
                newer: controls.newer.clone(),
                older: controls.older.clone(),
                retry: controls.retry.clone(),
            })}
            if state.groups().is_empty() && state.total() == Some(0) {
                <div class="empty">
                    {if state.filtered() {
                        "No directions match these filters."
                    } else {
                        "No operator directions have been published."
                    }}
                </div>
            } else if !state.groups().is_empty() {
                if state.has_newer() {
                    <button
                        type="button"
                        class="team-mode work-direction-newer-edge"
                        disabled={state.loading()}
                        onclick={controls.newer.clone()}
                    >{"Newer directions"}</button>
                }
                <ol
                    class="team-direction-ledger work-direction-ledger"
                    role="feed"
                    aria-label="Direction history"
                >
                    {for state.groups().iter().map(|group| {
                        direction_card::render(&card(group, topology, &message_links), &card_controls)
                    })}
                </ol>
            }
        </section>
    }
}

fn render_finder(topology: Option<&LaneView>, controls: &Controls) -> Html {
    html! {
        <div class="work-direction-finder" role="search" aria-label="Find directions">
            <div class="work-direction-fields">
                <label>
                    <span>{"Search"}</span>
                    <input
                        id="direction-history-query"
                        type="search"
                        maxlength={wire::intent::DIRECTION_HISTORY_QUERY_BYTES_MAX.to_string()}
                        placeholder="Direction text"
                    />
                </label>
                <label>
                    <span>{"Authored by"}</span>
                    <input id="direction-history-cutoff" type="date" />
                </label>
            </div>
            if let Some(topology) = topology {
                <fieldset class="work-direction-chip-filter">
                    <legend>{"Teams"}</legend>
                    <div class="work-direction-chips">
                        {for topology.lanes.iter().filter(|team| !team.members.is_empty()).map(|team| {
                            let identity = team.lane;
                            let selected = controls.selected_teams.contains(&identity);
                            let toggle = controls.toggle_team.reform(move |_| identity);
                            html! {
                                <button
                                    key={identity.to_string()}
                                    type="button"
                                    class="team-mode"
                                    aria-pressed={selected.to_string()}
                                    onclick={toggle}
                                >{team_label(team)}</button>
                            }
                        })}
                    </div>
                </fieldset>
                <fieldset class="work-direction-chip-filter">
                    <legend>{"Agents"}</legend>
                    <div class="work-direction-chips">
                        {for topology.lanes.iter().flat_map(|team| team.members.iter()).map(|agent| {
                            let identity = agent.lane;
                            let selected = controls.selected_agents.contains(&identity);
                            let toggle = controls.toggle_agent.reform(move |_| identity);
                            let label = agent.name.clone().unwrap_or_else(|| {
                                agent.worktree.rsplit('/').next().unwrap_or("Unnamed agent").to_owned()
                            });
                            html! {
                                <button
                                    key={identity.to_string()}
                                    type="button"
                                    class="team-mode"
                                    aria-pressed={selected.to_string()}
                                    onclick={toggle}
                                >{label}</button>
                            }
                        })}
                    </div>
                </fieldset>
            }
            <fieldset class="work-direction-chip-filter">
                <legend>{"State"}</legend>
                <div class="work-direction-chips">
                    {for STATES.map(|(state, label)| {
                        let selected = controls.selected_states.contains(&state);
                        let toggle = controls.toggle_state.reform(move |_| state);
                        html! {
                            <button
                                type="button"
                                class="team-mode"
                                data-direction-state={state_key(state)}
                                aria-pressed={selected.to_string()}
                                onclick={toggle}
                            >{label}</button>
                        }
                    })}
                </div>
            </fieldset>
            <div class="work-history-filter-actions app-control-rail team-mode-rail">
                <button type="button" class="team-mode" onclick={controls.apply.clone()}>{"Apply"}</button>
                <button type="button" class="team-mode" onclick={controls.clear.clone()}>{"Clear"}</button>
            </div>
        </div>
    }
}

fn team_label(team: &LaneRow) -> String {
    let names = team
        .members
        .iter()
        .filter_map(|member| member.name.as_deref())
        .take(2)
        .collect::<Vec<_>>();
    if names.is_empty() {
        format!("Team {}", crate::watchband::short(team.lane))
    } else if team.members.len() > names.len() {
        format!("{} +{}", names.join(", "), team.members.len() - names.len())
    } else {
        names.join(", ")
    }
}

/// One history fanout as the shared direction card. The resident returns only
/// the first non-quoted line, so the card is always an explicit excerpt.
fn card<'a>(
    group: &'a DirectionGroup,
    topology: Option<&LaneView>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Card<'a> {
    let mut groups = topology
        .into_iter()
        .flat_map(|topology| topology.lanes.iter())
        .filter_map(|team| {
            let recipients = group
                .recipients
                .iter()
                .filter(|recipient| team.member(recipient.lane).is_some())
                .map(|recipient| badge(recipient, Some(team), message_links))
                .collect::<Vec<_>>();
            (!recipients.is_empty()).then(|| (team_label(team), recipients))
        })
        .collect::<Vec<_>>();
    let unknown = group
        .recipients
        .iter()
        .filter(|recipient| {
            topology.is_none_or(|topology| topology.member(recipient.lane).is_none())
        })
        .map(|recipient| badge(recipient, None, message_links))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        groups.push(("Unavailable team".to_owned(), unknown));
    }
    Card {
        identity: group.fanout,
        id: crate::view::global_direction_card_id(group.fanout),
        body: group.preview.as_deref().unwrap_or("Preview unavailable"),
        complete: false,
        attachments: &[],
        author: Author::Operator,
        keys: group
            .recipients
            .iter()
            .map(|recipient| recipient.key.as_str())
            .collect::<BTreeSet<_>>(),
        recipients: Recipients::Teams(groups),
        authored_millis: group.authored.get().as_millisecond(),
    }
}

fn badge(
    recipient: &DirectionRecipient,
    team: Option<&LaneRow>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Recipient {
    let label = team
        .and_then(|team| team.member(recipient.lane))
        .map(|member| {
            member.name.clone().unwrap_or_else(|| {
                member
                    .worktree
                    .rsplit('/')
                    .next()
                    .unwrap_or("Unnamed agent")
                    .to_owned()
            })
        })
        .unwrap_or_else(|| format!("Unknown agent {}", crate::watchband::short(recipient.lane)));
    Recipient {
        key: recipient.directive.to_string(),
        lane: recipient.lane,
        label,
        accent: None,
        state_key: state_key(recipient.disposition),
        state_label: state_label(recipient.disposition),
        target: recipient_message_target(recipient, team, message_links),
        retirement: recipient.retirement.map(|retirement| retirement.position),
    }
}

fn recipient_message_target(
    recipient: &DirectionRecipient,
    team: Option<&LaneRow>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Option<String> {
    team?;
    if !matches!(
        recipient.disposition,
        Disposition::Acked | Disposition::Declined
    ) {
        return None;
    }
    recipient.retirement?;
    let message = message_links
        .directive_key(&recipient.key)
        .filter(|message| message.lane == recipient.lane)?;
    Some(super::super::watchband::agent_message_source_id(message))
}

const fn state_key(state: Disposition) -> &'static str {
    match state {
        Disposition::Live => "waiting",
        Disposition::Acked => "acknowledged",
        Disposition::Declined => "declined",
        Disposition::Lapsed => "lapsed",
        Disposition::DeadLettered => "dead-lettered",
        Disposition::Canceled => "canceled",
    }
}

const fn state_label(state: Disposition) -> &'static str {
    match state {
        Disposition::Live => "Waiting",
        Disposition::Acked => "Acknowledged",
        Disposition::Declined => "Declined",
        Disposition::Lapsed => "Lapsed",
        Disposition::DeadLettered => "Dead-lettered",
        Disposition::Canceled => "Canceled",
    }
}

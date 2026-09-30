//! Global Work Directions: one responsive mosaic over the resident history read.

use std::collections::BTreeSet;
use std::rc::Rc;

use super::*;
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
    html! {
        <section class="work-directions" aria-labelledby="work-directions-heading">
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
                <ol
                    class="team-direction-ledger work-direction-ledger"
                    role="feed"
                    aria-label="Direction history"
                >
                    {for state.groups().iter().map(|group| {
                        render_group(group, topology, board, &message_links, select_task)
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
            <div class="work-history-filter-actions canvas-control-rail team-mode-rail">
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

fn render_group(
    group: &DirectionGroup,
    topology: Option<&LaneView>,
    board: Option<&crate::runtime::BoardView>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
    select_task: &Callback<wire::TaskStamp>,
) -> Html {
    let markdown = group.preview.as_deref().unwrap_or("Preview unavailable");
    let rendered = crate::composer_markdown::render_received_with_classifications(markdown, &[]);
    let diagrams = Rc::new(rendered.diagrams.into_iter().map(AttrValue::from).collect());
    html! {
        <li
            key={group.fanout.to_string()}
            class="team-direction-ledger-row"
            data-direction-fanout={group.fanout.to_string()}
            role="article"
            aria-label={format!("Direction to {} recipients", group.recipients.len())}
        >
            <super::super::mermaid::MarkdownBody
                html={AttrValue::from(rendered.html)}
                sources={diagrams}
                final_answer={false}
                quote_tasks={Rc::default()}
            />
            {render_tasks(group, board, select_task)}
            <footer class="team-direction-footer">
                <span class="team-direction-attribution">
                    <span class="operator-avatar" aria-hidden="true">{"O"}</span>
                    <strong>{"You"}</strong>
                    <span>{"Direction"}</span>
                </span>
                {render_recipients(group, topology, message_links)}
                <super::super::clock::Age
                    authored={group.authored.get().as_millisecond()}
                    class="team-direction-age"
                />
            </footer>
        </li>
    }
}

fn render_tasks(
    group: &DirectionGroup,
    board: Option<&crate::runtime::BoardView>,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let keys = group
        .recipients
        .iter()
        .map(|recipient| recipient.key.as_str())
        .collect::<BTreeSet<_>>();
    let handles = board.map_or_else(BTreeSet::new, |board| {
        keys.iter()
            .filter_map(|key| board.quote_tasks.get(*key))
            .flatten()
            .collect::<BTreeSet<_>>()
    });
    html! {
        <div class="team-direction-tasks">
            {for board.into_iter().flat_map(|board| board.tasks.iter()).filter_map(|task| {
                let handle = &task.statement.body.handle;
                if !handles.contains(handle) {
                    return None;
                }
                let stamp = task.statement.body.born.clone();
                let open = {
                    let select = select.clone();
                    Callback::from(move |_| select.emit(stamp.clone()))
                };
                Some(html! {
                    <button
                        key={handle.to_string()}
                        type="button"
                        class="team-direction-task-chip"
                        aria-label={format!("Open {handle} details")}
                        onclick={open}
                    >{handle.to_string()}</button>
                })
            })}
        </div>
    }
}

fn render_recipients(
    group: &DirectionGroup,
    topology: Option<&LaneView>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Html {
    let known = topology.into_iter().flat_map(|topology| {
        topology.lanes.iter().filter_map(|team| {
            let recipients = group
                .recipients
                .iter()
                .filter(|recipient| team.member(recipient.lane).is_some())
                .collect::<Vec<_>>();
            if recipients.is_empty() {
                return None;
            }
            Some(html! {
                <div class="team-direction-recipient-group">
                    <span class="team-direction-recipient-team">{team_label(team)}</span>
                    <ul class="team-direction-recipients">
                        {for recipients.into_iter().map(|recipient| {
                            render_recipient(recipient, Some(team), message_links)
                        })}
                    </ul>
                </div>
            })
        })
    });
    let unknown = group
        .recipients
        .iter()
        .filter(|recipient| {
            topology.is_none_or(|topology| topology.member(recipient.lane).is_none())
        })
        .collect::<Vec<_>>();
    html! {
        <div class="team-direction-recipient-groups" aria-label="Recipients grouped by team">
            {for known}
            if !unknown.is_empty() {
                <div class="team-direction-recipient-group">
                    <span class="team-direction-recipient-team">{"Unavailable team"}</span>
                    <ul class="team-direction-recipients">
                        {for unknown.into_iter().map(|recipient| {
                            render_recipient(recipient, None, message_links)
                        })}
                    </ul>
                </div>
            }
        </div>
    }
}

fn render_recipient(
    recipient: &DirectionRecipient,
    team: Option<&LaneRow>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Html {
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
    let state = state_label(recipient.disposition);
    let message_target = recipient_message_target(recipient, team, message_links);
    let link_label = format!("Open {label} {state} message in Chat");
    let badge = html! {
        <>
            <span class="team-task-member">
                <span class="agent-presence" aria-hidden="true"></span>
                <strong class="team-task-member-name">{label}</strong>
            </span>
            <span class="team-direction-state">{state}</span>
        </>
    };
    html! {
        <li
            key={recipient.directive.to_string()}
            data-agent-lane={recipient.lane.to_string()}
            data-direction-state={state_key(recipient.disposition)}
            data-direction-retirement-position={recipient.retirement.map(|retirement| {
                retirement.position.get().to_string()
            })}
        >
            if let Some((target, _retirement)) = message_target {
                <a
                    class="team-direction-recipient-link"
                    href={crate::view::card_href(&target)}
                    aria-label={link_label}
                >{badge}</a>
            } else {
                <span class="team-direction-recipient-label">{badge}</span>
            }
        </li>
    }
}

fn recipient_message_target(
    recipient: &DirectionRecipient,
    team: Option<&LaneRow>,
    message_links: &crate::watchband::WorkMessageLinks<'_>,
) -> Option<(String, wire::Seq)> {
    team?;
    if !matches!(
        recipient.disposition,
        Disposition::Acked | Disposition::Declined
    ) {
        return None;
    }
    let retirement = recipient.retirement?;
    let message = message_links
        .directive_key(&recipient.key)
        .filter(|message| message.lane == recipient.lane)?;
    Some((
        super::super::watchband::agent_message_source_id(message),
        retirement.position,
    ))
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

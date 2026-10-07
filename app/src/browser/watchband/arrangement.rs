//! The arrangement panel a team's pips open: swap with or join another team.

use super::controls::TeamControl;
use super::*;

pub(super) struct Context<'a> {
    pub(super) controls: &'a UseReducerHandle<TeamControls>,
    pub(super) sender: &'a SenderHandle,
    pub(super) topology: &'a [(CanonicalUuid, Vec<CanonicalUuid>)],
    pub(super) topology_revision: u64,
    /// Current team rows, which name each team by its lead agent.
    pub(super) lanes: &'a [crate::runtime::LaneRow],
    pub(super) can_arrange: bool,
}

pub(super) fn render(team: CanonicalUuid, control: &TeamControl, context: &Context<'_>) -> Html {
    if control.arrangement_open {
        render_arrangement(team, context)
    } else {
        Html::default()
    }
}

fn render_arrangement(team: CanonicalUuid, context: &Context<'_>) -> Html {
    let choices = watchband::teams(context.topology)
        .filter(|target| *target != team)
        .flat_map(|target| {
            [
                ("swap", "Swap positions", watchband::TeamDrop::Swap),
                ("join", "Join into", watchband::TeamDrop::Join),
            ]
            .into_iter()
            .map(|(kind, verb, drop)| {
                let teams = watchband::arrangement(context.topology, team, target, drop)
                    .map(|rows| topology(&rows));
                (kind, verb, target, teams)
            })
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    html! {
        <div
            class="team-action-menu team-arrangement-menu app-overlay-panel app-overlay-panel--popover"
            role="menu"
            aria-label={format!("Arrange team {}", watchband::team_label_in(context.lanes, team))}
        >
            {for choices.into_iter().map(|(kind, verb, target, teams)| {
                let sender = context.sender.clone();
                let controls = context.controls.clone();
                let topology_revision = context.topology_revision;
                let enabled = context.can_arrange && teams.is_some();
                html! {
                    <button
                        type="button"
                        class="team-action team-arrangement-action"
                        role="menuitem"
                        disabled={!enabled}
                        data-team-arrangement={kind}
                        data-team-arrangement-target={target.to_string()}
                        onclick={Callback::from(move |_| {
                            let Some(teams) = teams.clone() else { return; };
                            if !admit_action(
                                &sender,
                                ClientAction::ArrangeTeams {
                                    topology_revision,
                                    teams,
                                },
                            ) {
                                return;
                            }
                            controls.dispatch(TeamControlAction::CloseArrangement);
                        })}
                    >
                        <span class="team-action-label">{format!("{verb} team {}", watchband::team_label_in(context.lanes, target))}</span>
                        <span class="team-action-detail">{
                            if kind == "join" && !enabled {
                                "limited to six agents"
                            } else if kind == "join" {
                                "combine agents"
                            } else {
                                "exchange team positions"
                            }
                        }</span>
                    </button>
                }
            })}
        </div>
    }
}

fn topology(rows: &[(CanonicalUuid, Vec<CanonicalUuid>)]) -> Vec<TeamArrangement> {
    rows.iter()
        .map(|(team, lanes)| TeamArrangement {
            team: *team,
            lanes: lanes.clone(),
        })
        .collect()
}

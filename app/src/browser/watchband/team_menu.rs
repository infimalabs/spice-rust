//! Team-local actions opened by one team toolbar's hamburger.

use super::controls::TeamControl;
use super::*;

pub(super) struct Context<'a> {
    pub(super) controls: &'a UseReducerHandle<TeamControls>,
    pub(super) sender: &'a SenderHandle,
    pub(super) runtime: &'a CanvasRuntime,
    pub(super) agents: &'a view::AgentControls,
    pub(super) topology: &'a [(CanonicalUuid, Vec<CanonicalUuid>)],
    pub(super) topology_revision: u64,
    pub(super) can_configure: bool,
    pub(super) can_arrange: bool,
}

pub(super) fn trigger(
    team: CanonicalUuid,
    control: &TeamControl,
    controls: &UseReducerHandle<TeamControls>,
) -> Html {
    let toggle = controls.clone();
    html! {
        <button
            class="team-menu"
            type="button"
            aria-label={format!("Team actions for {}", watchband::short(team))}
            aria-haspopup="menu"
            aria-expanded={(control.menu_open || control.import_open).to_string()}
            title="Team actions"
            onclick={Callback::from(move |_| {
                toggle.dispatch(TeamControlAction::ToggleMenu { team });
            })}
        ><span class="team-menu-icon" aria-hidden="true"></span></button>
    }
}

pub(super) fn render(
    team: &crate::runtime::LaneRow,
    control: &TeamControl,
    context: &Context<'_>,
) -> Html {
    if control.arrangement_open {
        return render_arrangement(team.lane, context);
    }
    if control.import_open {
        return render_import(team, context);
    }
    if !control.menu_open {
        return Html::default();
    }
    let identity = team.lane;
    let member_count = team.members.len();
    let close = context.controls.clone();
    let import = context.controls.clone();
    let topology_actions = render_topology_actions(identity, control, context);
    html! {
        <div
            class="team-action-menu"
            role="menu"
            aria-label={format!("Actions for team {}", watchband::short(identity))}
        >
            <button
                type="button"
                class="team-action"
                role="menuitem"
                onclick={Callback::from(move |_| {
                    close.dispatch(TeamControlAction::CloseTeam { team: identity });
                })}
            >
                <span class="team-action-label">{"Close team"}</span>
                <span class="team-action-detail">{members(member_count)}</span>
            </button>
            <button
                type="button"
                class="team-action"
                role="menuitem"
                disabled={!context.can_configure && !context.runtime.offers(CommandKind::LaneSeat)}
                onclick={Callback::from(move |_| {
                    import.dispatch(TeamControlAction::ToggleImport { team: identity });
                })}
            >
                <span class="team-action-label">{"Import agent"}</span>
                <span class="team-action-detail">{"import or create"}</span>
            </button>
            {topology_actions}
        </div>
    }
}

fn render_topology_actions(
    identity: CanonicalUuid,
    control: &TeamControl,
    context: &Context<'_>,
) -> Html {
    let split = watchband::split_team(context.topology, identity).map(|rows| topology(&rows));
    let split_enabled = context.can_arrange && split.is_some();
    let split_controls = context.controls.clone();
    let split_sender = context.sender.clone();
    let original = topology(context.topology);
    let restore = control.previous_topology.clone();
    let restore_count = restore
        .as_ref()
        .and_then(|rows| rows.iter().find(|row| row.team == identity))
        .map_or(0, |row| row.lanes.len());
    let restore_enabled = context.can_arrange && restore.is_some();
    let restore_controls = context.controls.clone();
    let restore_sender = context.sender.clone();
    let topology_revision = context.topology_revision;
    html! {
        <>
            <button
                type="button"
                class="team-action"
                role="menuitem"
                disabled={!split_enabled}
                onclick={Callback::from(move |_| {
                    let Some(teams) = split.clone() else { return; };
                    if !admit_action(
                        &split_sender,
                        ClientAction::ArrangeTeams {
                            topology_revision,
                            teams,
                        },
                    ) {
                        return;
                    }
                    split_controls.dispatch(TeamControlAction::SaveSplit {
                        team: identity,
                        topology: original.clone(),
                    });
                })}
            >
                <span class="team-action-label">{"Split into individuals"}</span>
                <span class="team-action-detail">{
                    if split_enabled { "one team per agent" } else { "already individual" }
                }</span>
            </button>
            <button
                type="button"
                class="team-action"
                role="menuitem"
                disabled={!restore_enabled}
                onclick={Callback::from(move |_| {
                    let Some(teams) = restore.clone() else { return; };
                    if !admit_action(
                        &restore_sender,
                        ClientAction::ArrangeTeams {
                            topology_revision,
                            teams,
                        },
                    ) {
                        return;
                    }
                    restore_controls.dispatch(TeamControlAction::ClearRestore {
                        team: identity,
                    });
                })}
            >
                <span class="team-action-label">{"Restore previous team"}</span>
                <span class="team-action-detail">{
                    if restore_enabled { members(restore_count) } else { "no saved subgroup".to_owned() }
                }</span>
            </button>
        </>
    }
}

fn render_arrangement(team: CanonicalUuid, context: &Context<'_>) -> Html {
    let choices = watchband::teams(context.topology)
        .filter(|target| *target != team)
        .flat_map(|target| {
            [
                ("before", "Move before", watchband::TeamDrop::Reorder),
                ("after", "Move after", watchband::TeamDrop::ReorderAfter),
                ("join", "Join into", watchband::TeamDrop::Join),
            ]
            .into_iter()
            .filter_map(|(kind, verb, drop)| {
                watchband::arrangement(context.topology, team, target, drop)
                    .map(|rows| (kind, verb, target, topology(&rows)))
            })
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    html! {
        <div
            class="team-action-menu team-arrangement-menu"
            role="menu"
            aria-label={format!("Arrange team {}", watchband::short(team))}
        >
            {for choices.into_iter().map(|(kind, verb, target, teams)| {
                let sender = context.sender.clone();
                let controls = context.controls.clone();
                let topology_revision = context.topology_revision;
                html! {
                    <button
                        type="button"
                        class="team-action team-arrangement-action"
                        role="menuitem"
                        data-team-arrangement={kind}
                        data-team-arrangement-target={target.to_string()}
                        onclick={Callback::from(move |_| {
                            if !admit_action(
                                &sender,
                                ClientAction::ArrangeTeams {
                                    topology_revision,
                                    teams: teams.clone(),
                                },
                            ) {
                                return;
                            }
                            controls.dispatch(TeamControlAction::CloseMenus);
                        })}
                    >
                        <span class="team-action-label">{format!("{verb} team {}", watchband::short(target))}</span>
                        <span class="team-action-detail">{
                            if kind == "join" { "combine agents" } else { "reorder teams" }
                        }</span>
                    </button>
                }
            })}
        </div>
    }
}

pub(super) fn render_import(team: &crate::runtime::LaneRow, context: &Context<'_>) -> Html {
    let identity = team.lane;
    let current = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let waiting = context
        .runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let candidates = context
        .runtime
        .lanes()
        .into_iter()
        .flat_map(|lanes| &lanes.lanes)
        .flat_map(|candidate_team| {
            candidate_team
                .members
                .iter()
                .enumerate()
                .map(move |(position, member)| (candidate_team, position, member))
        })
        .filter(|(_, _, member)| !current.contains(&member.lane))
        .collect::<Vec<_>>();
    let close = context.controls.clone();
    let topology_revision = context.topology_revision;
    html! {
        <div
            class="team-action-menu team-import-menu"
            role="dialog"
            aria-label="Add agent"
        >
            <header>
                <strong>{"Add agent"}</strong>
                <button
                    type="button"
                    onclick={Callback::from(move |_| {
                        close.dispatch(TeamControlAction::ToggleImport { team: identity });
                    })}
                >{"Cancel"}</button>
            </header>
            {view::render_seat_panel(Some(context.runtime), context.agents)}
            <h3 class="team-import-heading">{"Import existing agent"}</h3>
            if candidates.is_empty() {
                <p class="team-import-empty">{"No available agents"}</p>
            } else {
                {for candidates.into_iter().map(|(candidate_team, position, member)| {
                    let sender = context.sender.clone();
                    let lane = member.lane;
                    let pending = waiting
                        .iter()
                        .find(|row| row.lane == lane)
                        .map_or(0, |row| row.pending.len());
                    let status = member.status(pending);
                    let label = member
                        .name
                        .clone()
                        .unwrap_or_else(|| watchband::leaf(&member.worktree).to_owned());
                    let worktree = watchband::leaf(&member.worktree).to_owned();
                    let accessible = format!("Import {label} on {worktree}");
                    html! {
                        <button
                            type="button"
                            class="team-import-choice"
                            aria-label={accessible}
                            data-team-member={lane.to_string()}
                            data-accent={watchband::accent(
                                candidate_team.members.len(),
                                position,
                            ).to_string()}
                            disabled={!context.can_configure}
                            onclick={Callback::from(move |_| {
                                let _admitted = admit_action(
                                    &sender,
                                    ClientAction::ConfigureTeam {
                                        topology_revision,
                                        team: identity,
                                        member: Some(lane),
                                        life: None,
                                    },
                                );
                            })}
                        >
                            <span class={classes!("canvas-member-presence", status.key())}></span>
                            <span class="canvas-member-copy">
                                <strong>{format!("{label} on {worktree}")}</strong>
                                <small>{format!("{} · {}", member.vendor, status.label())}</small>
                            </span>
                            <span class="team-import-action" aria-hidden="true">{"Import"}</span>
                        </button>
                    }
                })}
            }
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

fn members(count: usize) -> String {
    format!("{count} {}", if count == 1 { "agent" } else { "agents" })
}

//! Team-fleet shell, topology drag, and per-team toolbar rendering.

use super::*;

/// The media type a dragged team carries, and the only one a team accepts.
///
/// Message cards are draggable too, and they carry their prose as `text/plain`
/// so it can be quoted into a composer. A team surface that accepted plain
/// text would rearrange the fleet because the operator dragged a message onto
/// it, so the gesture is told apart by what it carries rather than by where it
/// lands.
const TEAM_DRAG_MEDIA: &str = "application/x-spice-team";

/// The box a drop is measured against: one team's whole surface.
const TEAM_SURFACE: &str = ".team-surface";

/// The team a drop carries, if it carries one at all.
fn dragged_team(transfer: &web_sys::DataTransfer) -> Option<CanonicalUuid> {
    transfer.get_data(TEAM_DRAG_MEDIA).ok()?.parse().ok()
}

/// Whether a drag in flight carries a team.
///
/// Only the media types are readable while a drag is moving; the identity
/// itself stays sealed until the drop. That is enough to decide whether this
/// surface is a drop target, which is all `dragover` has to answer.
pub(super) fn carries_team(transfer: &web_sys::DataTransfer) -> bool {
    transfer
        .types()
        .iter()
        .any(|media| media.as_string().as_deref() == Some(TEAM_DRAG_MEDIA))
}

/// The three one-click views of one live team.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::browser) enum TeamMode {
    Dash,
    Work,
    #[default]
    Chat,
}

impl TeamMode {
    pub(super) const fn key(self) -> &'static str {
        match self {
            Self::Dash => "Dash",
            Self::Work => "Work",
            Self::Chat => "Chat",
        }
    }

    const fn from_destination(destination: TeamDestination) -> Self {
        match destination {
            TeamDestination::Dash => Self::Dash,
            TeamDestination::Work => Self::Work,
            TeamDestination::Chat => Self::Chat,
        }
    }
}

pub(super) struct FleetRender<'a, 'w> {
    pub(super) work: Option<&'a crate::work::WorkProjection<'w>>,
    pub(super) team_settled: &'a super::super::team_settled::Controls,
    pub(super) selected_task: &'a UseStateHandle<Option<wire::TaskStamp>>,
    pub(super) select_task: &'a Callback<wire::TaskStamp>,
    pub(super) task_inspector: &'a crate::browser::message_task_inspector::Context,
    pub(super) runtime: &'a CanvasRuntime,
    pub(super) epoch: CanonicalUuid,
    pub(super) topology_revision: u64,
    pub(super) authority: SubmissionAuthority,
    pub(super) can_arrange_teams: bool,
    pub(super) can_configure: bool,
    pub(super) topology: Vec<(CanonicalUuid, Vec<CanonicalUuid>)>,
    pub(super) quote_agents: Rc<crate::composer_markdown::QuoteAgents>,
    pub(super) surface: UseStateHandle<SurfaceData>,
    pub(super) timeline_request: Callback<TeamFlowPageRequest>,
    pub(super) sender: &'a SenderHandle,
    pub(super) drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub(super) submissions: &'a SubmissionHandle,
    pub(super) team_controls: &'a UseReducerHandle<TeamControls>,
    pub(super) controls: &'a view::AgentControls,
    pub(super) composer_drag: &'a PrimaryComposerDragState,
}

pub(super) fn render_empty_fleet(fleet: &NodeRef, render: &FleetRender<'_, '_>) -> Html {
    let empty_team = crate::runtime::LaneRow {
        lane: render.epoch,
        subscription: wire::intent::TeamLife::Steer,
        members: Vec::new(),
    };
    let mode = render.team_controls.empty_mode();
    let control = render
        .team_controls
        .current(empty_team.lane, empty_team.subscription);
    let actions = team_menu::Context {
        controls: render.team_controls,
        sender: render.sender,
        runtime: render.runtime,
        submissions: &render.submissions.status,
        agents: render.controls,
        topology: &render.topology,
        topology_revision: render.topology_revision,
        can_configure: false,
        can_arrange: false,
    };
    let dismiss = team_menu_dismissal(render.team_controls);
    html! {
        <div ref={fleet.clone()} class="team-fleet" data-pane="agent-console" data-view="mosaic"
            tabindex="0" aria-label="Agent teams" onclick={dismiss}>
            <section class="team-surface" data-team-weight="1">
                {render_empty_toolbar(
                    mode,
                    render.team_controls,
                    render.epoch,
                    control.import_open,
                )}
                if control.import_open {
                    {team_menu::render_import(&empty_team, &actions)}
                }
                <div class="team-pane" data-team-mode={mode.key()}>
                    {match mode {
                        TeamMode::Chat => {
                            empty("No teams are assigned. Open Import agent to create an agent.")
                        }
                        TeamMode::Work => render_tasks(
                            render.runtime,
                            render.work,
                            &empty_team,
                            &[],
                            WorkRender {
                                selected: (**render.selected_task).as_ref(),
                                select: render.select_task,
                                submissions: render.submissions,
                                settled: None,
                            },
                        ),
                        TeamMode::Dash => render_details(
                            &render.surface,
                            render.runtime,
                            &empty_team,
                            &[],
                            DashRender {
                                task_inspector: render.task_inspector,
                                select: render.select_task,
                                request: &render.timeline_request,
                                controls: render.team_controls,
                            },
                        ),

                    }}
                </div>
            </section>
        </div>
    }
}

pub(super) fn render_populated_fleet(
    teams: &[&crate::runtime::LaneRow],
    standings: &[watchband::Standing],
    fleet: &NodeRef,
    render: &FleetRender<'_, '_>,
) -> Html {
    let dismiss = team_menu_dismissal(render.team_controls);
    html! {
        <div ref={fleet.clone()} class="team-fleet" data-pane="agent-console" data-view="mosaic"
            tabindex="0" aria-label="Agent teams" onclick={dismiss}>
            // Every team keeps its own subtree, so a reorder, fusion, or split moves
            // composers with their drafts, focus, and open menus instead of
            // repainting a neighbour's nodes. Yew diffs a list positionally unless
            // every child is keyed.
            {for teams.iter().map(|team| {
                let team_standings = standings
                    .iter()
                    .filter(|standing| standing.team == team.lane)
                    .cloned()
                    .collect::<Vec<_>>();
                let mode = render.team_controls
                    .current(team.lane, team.subscription)
                    .mode;
                html! {
                    <section
                        key={team.lane.to_string()}
                        class="team-surface"
                        data-team={team.lane.to_string()}
                        data-team-weight={watchband::team_visual_weight(team.members.len()).to_string()}
                        ondragover={team_dragover(render.can_arrange_teams)}
                        ondrop={team_drop(
                            render.can_arrange_teams,
                            &render.topology,
                            render.topology_revision,
                            team.lane,
                            render.sender,
                        )}
                    >
                        {render_team_toolbar(
                            mode,
                            team,
                            &team_standings,
                            render,
                        )}
                        {view::chrome::render_team_ticker(
                            render.runtime,
                            team,
                            &team_standings,
                        )}
                        <div class="team-pane" data-team-mode={mode.key()}>
                            <div
                                class="team-mode-pane"
                                data-mode="Chat"
                                data-active={(mode == TeamMode::Chat).to_string()}
                                aria-hidden={(mode != TeamMode::Chat).to_string()}
                                inert={(mode != TeamMode::Chat).then_some("true")}
                            >
                                {render_compose(
                                    render.runtime,
                                    team,
                                    &render.quote_agents,
                                    &team_standings,
                                    render.epoch,
                                    render.topology_revision,
                                    &render.topology,
                                    render.authority,
                                    render.can_arrange_teams,
                                    render.sender,
                                    render.composer_drag,
                                    render.drafts,
                                    render.submissions,
                                    render.team_controls,
                                    render.can_configure,
                                    render.controls,
                                )}
                            </div>
                            if mode != TeamMode::Chat {
                                <div
                                    class="team-mode-pane"
                                    data-mode={mode.key()}
                                    data-active="true"
                                    aria-hidden="false"
                                >
                                    {match mode {
                                        TeamMode::Work => render_tasks(
                                            render.runtime,
                                            render.work,
                                            team,
                                            &team_standings,
                                            WorkRender {
                                                selected: (**render.selected_task).as_ref(),
                                                select: render.select_task,
                                                submissions: render.submissions,
                                                settled: Some(render.team_settled),
                                            },
                                        ),
                                        TeamMode::Dash => render_details(
                                            &render.surface,
                                            render.runtime,
                                            team,
                                            &team_standings,
                                            DashRender {
                                                task_inspector: render.task_inspector,
                                                select: render.select_task,
                                                request: &render.timeline_request,
                                                controls: render.team_controls,
                                            },
                                        ),

                                        TeamMode::Chat => Html::default(),
                                    }}
                                </div>
                            }
                        </div>
                    </section>
                }
            })}
        </div>
    }
}

fn team_menu_dismissal(controls: &UseReducerHandle<TeamControls>) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        let inside_control = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| {
                target
                    .closest(".team-action-menu, .team-menu, .team-lights[data-lane-drag-handle]")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if !inside_control {
            controls.dispatch(TeamControlAction::CloseMenus);
        }
    })
}

/// Open team menus own Escape only while Chat shows them.
#[hook]
pub(in crate::browser) fn use_team_menu_escape(
    controls: &UseReducerHandle<TeamControls>,
    visible: bool,
) {
    let dismiss = {
        let controls = controls.clone();
        Callback::from(move |()| {
            if let Some(trigger) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| {
                    document
                        .query_selector(
                            ".team-lights[data-lane-drag-handle][aria-expanded=\"true\"]",
                        )
                        .ok()
                        .flatten()
                })
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = trigger.focus();
            }
            controls.dispatch(TeamControlAction::CloseMenus);
        })
    };
    crate::browser::overlay::use_escape_owner(visible && controls.any_menu_open(), dismiss);
}

/// Pick up a team from its pip stack, but only where it can be arranged.
///
/// Keeping the gesture on the compact status pips leaves composer prose and
/// activity cards free for their own selection and drag behavior.
fn team_drag_start(team: CanonicalUuid, can_arrange: bool) -> Callback<DragEvent> {
    let carried = team.to_string();
    Callback::from(move |event: DragEvent| {
        if !can_arrange {
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        let _ = transfer.set_data(TEAM_DRAG_MEDIA, &carried);
        transfer.set_effect_allowed("move");
    })
}

/// Accept a team in flight, and nothing else.
fn team_dragover(can_arrange: bool) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if !can_arrange {
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if !carries_team(&transfer) {
            return;
        }
        transfer.set_drop_effect("move");
        // Only a prevented dragover makes a surface droppable; leaving it
        // alone is how every other drag keeps passing through untouched.
        event.prevent_default();
    })
}

/// Commit the arrangement a drop on this team states.
///
/// The topology is the one this render drew, captured rather than re-read: the
/// operator arranged the teams they were looking at, and an arrangement built
/// from a table that changed underneath them would move a team they never
/// dragged.
fn team_drop(
    can_arrange: bool,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    topology_revision: u64,
    target: CanonicalUuid,
    sender: &SenderHandle,
) -> Callback<DragEvent> {
    let topology = topology.to_vec();
    let sender = sender.clone();
    Callback::from(move |event: DragEvent| {
        if !can_arrange {
            return;
        }
        let Some(dragged) = event.data_transfer().as_ref().and_then(dragged_team) else {
            return;
        };
        // The listener is delegated, so the element the drop landed on is the
        // only handle on this team's box; `current_target` is the delegation
        // root and would measure the whole workspace as one team.
        let Some(surface) = event
            .target()
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            .and_then(|node| node.closest(TEAM_SURFACE).ok().flatten())
        else {
            return;
        };
        if surface.get_attribute("data-team") != Some(target.to_string()) {
            return;
        }
        let rect = surface.get_bounding_client_rect();
        let drop = watchband::TeamDrop::at(f64::from(event.client_x()) - rect.left(), rect.width());
        let Some(arranged) = watchband::arrangement(&topology, dragged, target, drop) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        if !admit_action(
            &sender,
            ClientAction::ArrangeTeams {
                topology_revision,
                teams: arranged
                    .into_iter()
                    .map(|(team, lanes)| TeamArrangement { team, lanes })
                    .collect(),
            },
        ) && let Some(transfer) = event.data_transfer()
        {
            transfer.set_drop_effect("none");
        }
    })
}

fn render_empty_toolbar(
    mode: TeamMode,
    team_controls: &UseReducerHandle<TeamControls>,
    team: CanonicalUuid,
    import_open: bool,
) -> Html {
    html! {
        <header class="team-toolbar">
            <span
                class="team-lights"
                role="group"
                aria-label="0 admitted agents"
            ></span>
            {render_mode_rail(mode, None, &[], team_controls)}
            <button
                class="team-menu"
                type="button"
                aria-label="Import agent"
                aria-haspopup="dialog"
                aria-expanded={import_open.to_string()}
                title="Import or create an agent"
                onclick={{
                    let choose = team_controls.clone();
                    Callback::from(move |_| {
                        choose.dispatch(TeamControlAction::ToggleImport { team });
                    })
                }}
            ><span class="team-menu-icon" aria-hidden="true"></span></button>
        </header>
    }
}

fn render_team_toolbar(
    mode: TeamMode,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    render: &FleetRender<'_, '_>,
) -> Html {
    let pip_stack = crate::pip_layout::PipStack::new(standings.len());
    let can_arrange = render.can_arrange_teams && watchband::teams(&render.topology).count() > 1;
    let id = team.lane;
    let arrangement_help_id = format!("team-arrangement-help-{id}");
    let lights_label = if can_arrange {
        format!(
            "{} admitted agents; arrange team {}",
            standings.len(),
            watchband::short(team.lane)
        )
    } else {
        format!("{} admitted agents", standings.len())
    };
    let control = render.team_controls.current(team.lane, team.subscription);
    let actions = team_menu::Context {
        controls: render.team_controls,
        sender: render.sender,
        runtime: render.runtime,
        submissions: &render.submissions.status,
        agents: render.controls,
        topology: &render.topology,
        topology_revision: render.topology_revision,
        can_configure: render.can_configure,
        can_arrange: render.can_arrange_teams,
    };
    let toggle_arrangement = team_arrangement_click(id, can_arrange, render.team_controls);
    let keyboard_arrangement = team_arrangement_key(id, can_arrange, render.team_controls);
    // PipStack places index 0 at the lower right, so pip index is canonical
    // member position: the main agent sits beside the team controls.
    let render_pip = |index: usize| {
        let standing = &standings[index];
        let member = team.member(standing.lane)?;
        let status = member.status(standing.pending);
        let position = pip_stack
            .position(index)
            .expect("standing index belongs to the lane pip stack");
        let label = format!("{}: {}", standing.label, status.label());
        Some(html! {
            <span
                class={classes!("agent-presence-indicator", "team-light", status.key())}
                role="img"
                data-lane-light-target-id={member.lane.to_string()}
                data-lane-status={status.key()}
                data-pip-column={position.column.to_string()}
                data-pip-row={position.row.to_string()}
                aria-label={label.clone()}
                title={label}
            ></span>
        })
    };
    html! {
        <>
        <header class="team-toolbar">
            <span
                class={classes!("team-lights", (pip_stack.rows() == 1).then_some("single"))}
                role={if can_arrange { "button" } else { "group" }}
                aria-label={lights_label}
                aria-haspopup={can_arrange.then_some("menu")}
                aria-expanded={can_arrange.then(|| control.arrangement_open.to_string())}
                aria-describedby={can_arrange.then(|| arrangement_help_id.clone())}
                tabindex={can_arrange.then_some("0")}
                title={can_arrange.then_some("Drag, or press Enter or Space to arrange this team")}
                draggable={can_arrange.to_string()}
                data-lane-drag-handle={can_arrange.then_some("")}
                data-team-drag-id={can_arrange.then(|| team.lane.to_string())}
                ondragstart={team_drag_start(team.lane, can_arrange)}
                onclick={toggle_arrangement}
                onkeydown={keyboard_arrangement}
            >
                if can_arrange {
                    <span id={arrangement_help_id} class="visually-hidden">
                        {"Press Enter or Space, or tap, to choose where to move or join this team."}
                    </span>
                }
                {for pip_stack.columns_from_left().map(|column| html! {
                    <span class="team-pip-column" data-pip-column={column.column.to_string()}>
                        {column.upper.and_then(&render_pip)}
                        {render_pip(column.lower)}
                    </span>
                })}
            </span>
            {render_mode_rail(
                mode,
                Some(team.lane),
                standings,
                render.team_controls,
            )}
            {team_menu::trigger(team.lane, &control, render.team_controls)}
        </header>
        {team_menu::render(team, &control, &actions)}
        </>
    }
}

fn render_mode_rail(
    mode: TeamMode,
    team: Option<CanonicalUuid>,
    standings: &[Standing],
    team_controls: &UseReducerHandle<TeamControls>,
) -> Html {
    html! {
        <nav class="canvas-control-rail team-mode-rail" role="tablist" aria-label="Live team views">
            {for TeamDestination::ALL.map(|destination| {
                let candidate = TeamMode::from_destination(destination);
                let selected = candidate == mode;
                let choose = team_controls.clone();
                let badge = mode_badge(candidate, standings);
                html! {
                    <button
                        type="button"
                        role="tab"
                        class={classes!("team-mode", selected.then_some("selected"))}
                        aria-selected={selected.to_string()}
                        onclick={Callback::from(move |_| {
                            choose.dispatch(TeamControlAction::SetMode {
                                team,
                                mode: candidate,
                            });
                        })}
                    >
                        <span class="team-mode-label" title={destination.label()}>
                            {destination.label()}
                        </span>
                        if let Some(badge) = badge {
                            <span class="team-mode-badge">{badge}</span>
                        }
                    </button>
                }
            })}
        </nav>
    }
}

fn team_arrangement_click(
    team: CanonicalUuid,
    can_arrange: bool,
    controls: &UseReducerHandle<TeamControls>,
) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        event.stop_propagation();
        if can_arrange {
            controls.dispatch(TeamControlAction::ToggleArrangement { team });
        }
    })
}

fn team_arrangement_key(
    team: CanonicalUuid,
    can_arrange: bool,
    controls: &UseReducerHandle<TeamControls>,
) -> Callback<KeyboardEvent> {
    let controls = controls.clone();
    Callback::from(move |event: KeyboardEvent| {
        if !can_arrange || !matches!(event.key().as_str(), "Enter" | " ") {
            return;
        }
        event.prevent_default();
        event.stop_propagation();
        controls.dispatch(TeamControlAction::ToggleArrangement { team });
    })
}

fn mode_badge(mode: TeamMode, standings: &[Standing]) -> Option<String> {
    let count = match mode {
        TeamMode::Chat => standings.iter().map(|standing| standing.pending).sum(),
        TeamMode::Work => standings
            .iter()
            .filter(|standing| standing.handle.is_some())
            .count(),
        TeamMode::Dash => standings.len(),
    };
    watchband::badge(count)
}

//! Team-fleet shell, topology drag, and per-team toolbar rendering.

use super::*;

pub(super) struct FleetRender<'a, 'w> {
    pub(super) work: Option<&'a crate::work::WorkProjection<'w>>,
    pub(super) select_task: &'a Callback<wire::TaskStamp>,
    pub(super) runtime: &'a AppRuntime,
    pub(super) epoch: CanonicalUuid,
    pub(super) topology_revision: u64,
    pub(super) authority: SubmissionAuthority,
    pub(super) can_arrange_teams: bool,
    pub(super) can_configure: bool,
    pub(super) topology: Rc<Vec<(CanonicalUuid, Vec<CanonicalUuid>)>>,
    pub(super) quote_agents: Rc<crate::composer_markdown::QuoteAgents>,
    pub(super) surface: UseStateHandle<SurfaceData>,
    pub(super) sender: &'a SenderHandle,
    pub(super) drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub(super) submissions: &'a SubmissionHandle,
    pub(super) team_controls: &'a UseReducerHandle<TeamControls>,
    pub(super) controls: &'a view::AgentControls,
    pub(super) composer_drag: &'a PrimaryComposerDragState,
    pub(super) team_drag: &'a TeamDragState,
}

pub(super) fn render_empty_fleet(fleet: &NodeRef, render: &FleetRender<'_, '_>) -> Html {
    let empty_team = crate::runtime::LaneRow {
        lane: render.epoch,
        subscription: wire::intent::TeamLife::Steer,
        members: Vec::new(),
    };
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
                    render.team_controls,
                    render.epoch,
                    control.import_open,
                )}
                if control.import_open {
                    {team_menu::render_import(&empty_team, &actions)}
                }
                <div class="team-pane">
                    {empty("No teams are assigned. Open Import agent to create an agent.")}
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
                let attention = attention::TeamAttention::new(
                    render.runtime,
                    &team_standings,
                    render.work,
                    render.sender,
                    render.select_task,
                );
                html! {
                    <section
                        key={team.lane.to_string()}
                        class="team-surface"
                        data-team={team.lane.to_string()}
                        data-team-weight={watchband::team_visual_weight(team.members.len()).to_string()}
                    >
                        {render_team_toolbar(
                            team,
                            &team_standings,
                            render,
                        )}
                        <div class="team-pane">
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
                                &render.surface,
                                render.select_task,
                                &attention,
                            )}
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

fn render_empty_toolbar(
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
            {view::chrome::render_empty_team_ticker()}
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
    let dragging = team_dragging(
        id,
        can_arrange,
        render.topology_revision,
        &render.topology,
        render.sender,
        render.team_drag,
    );
    let lost_capture = dragging.cancel.clone();
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
                data-lane-drag-handle={can_arrange.then_some("")}
                data-team-drag-id={can_arrange.then(|| team.lane.to_string())}
                onpointerdown={dragging.start}
                onpointermove={dragging.movement}
                onpointerup={dragging.finish}
                onpointercancel={dragging.cancel}
                onlostpointercapture={lost_capture}
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
            {view::chrome::render_team_ticker(render.runtime, team, standings)}
            {team_menu::trigger(team.lane, &control, render.team_controls)}
        </header>
        {team_menu::render(team, &control, &actions)}
        </>
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

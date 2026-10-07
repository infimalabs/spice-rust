//! Team-fleet shell, topology drag, and per-team toolbar rendering.

use super::*;

pub(super) struct FleetRender<'a, 'w> {
    pub(super) work: Option<&'a crate::work::WorkProjection<'w>>,
    pub(super) navigate: &'a Callback<navigation::Intent>,
    pub(super) select_task: &'a Callback<wire::TaskStamp>,
    pub(super) open_message_task: &'a Callback<(wire::TaskStamp, web_sys::Element)>,
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
    pub(super) composer_slots: &'a super::ComposerSlots,
    pub(super) team_drag: &'a TeamDragState,
}

pub(super) fn render_empty_fleet(fleet: &NodeRef) -> Html {
    html! {
        <div ref={fleet.clone()} class="team-fleet" data-pane="agent-console" data-view="mosaic"
            tabindex="0" aria-label="Agent teams">
            <section class="team-surface" data-team-weight="1">
                <header class="team-toolbar">
                    {view::chrome::render_empty_team_ticker()}
                    <span
                        class="team-lights"
                        role="group"
                        aria-label="0 admitted agents"
                    ></span>
                </header>
                <div class="team-pane">
                    {empty("No teams are assigned.")}
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
    let dismiss = arrangement_dismissal(render.team_controls);
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
                    team.lane,
                    &team_standings,
                    render.work,
                    render.sender,
                    render.select_task,
                    render.navigate,
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
                                render.open_message_task,
                                &attention,
                                render.composer_slots.slot(team.lane),
                            )}
                        </div>
                    </section>
                }
            })}
        </div>
    }
}

fn arrangement_dismissal(controls: &UseReducerHandle<TeamControls>) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        let inside_control = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| {
                target
                    .closest(".team-action-menu, .team-lights[data-lane-drag-handle]")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if !inside_control {
            controls.dispatch(TeamControlAction::CloseArrangement);
        }
    })
}

/// An open team arrangement owns Escape only while Chat shows it.
#[hook]
pub(in crate::browser) fn use_team_arrangement_escape(
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
            controls.dispatch(TeamControlAction::CloseArrangement);
        })
    };
    crate::browser::overlay::use_escape_owner(visible && controls.any_arrangement_open(), dismiss);
}

fn render_team_toolbar(
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    render: &FleetRender<'_, '_>,
) -> Html {
    let can_arrange = render.can_arrange_teams && watchband::teams(&render.topology).count() > 1;
    let id = team.lane;
    let arrangement_help_id = format!("team-arrangement-help-{id}");
    let lights_label = if can_arrange {
        format!(
            "{} admitted agents; arrange team {}",
            standings.len(),
            watchband::team_label(team)
        )
    } else {
        format!("{} admitted agents", standings.len())
    };
    let control = render.team_controls.current(team.lane, team.subscription);
    let arrangement_context = arrangement::Context {
        controls: render.team_controls,
        sender: render.sender,
        topology: &render.topology,
        topology_revision: render.topology_revision,
        lanes: render.runtime.lanes().map_or(&[][..], |view| &view.lanes),
        can_arrange: render.can_arrange_teams,
    };
    let toggle_arrangement = team_arrangement_click(id, can_arrange, render.team_controls);
    let keyboard_arrangement = team_arrangement_key(id, can_arrange, render.team_controls);
    let render_pip = |index: usize| {
        let standing = &standings[index];
        let member = team.member(standing.lane)?;
        let status = member.status(standing.waking);
        let label = format!("{}: {}", standing.label, status.label());
        Some(html! {
            <span
                class={classes!("agent-presence-indicator", "team-light", status.key())}
                role="img"
                data-lane-light-target-id={member.lane.to_string()}
                data-lane-status={status.key()}
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
            {render_team_rail(team.lane, render)}
            {view::chrome::render_team_ticker(render.runtime, team, standings)}
            <span
                class="team-lights"
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
                {for standings.iter().enumerate().filter_map(|(index, _)| render_pip(index))}
            </span>
        </header>
        {arrangement::render(team.lane, &control, &arrangement_context)}
        </>
    }
}

/// The task tabs in this team's header band: a badge-only team total, then
/// each visibility-qualified stem once, badged with its open-task count and
/// followed by all of its open task handles. The fleet never shows over Work,
/// so none selects.
fn render_team_rail(team: CanonicalUuid, render: &FleetRender<'_, '_>) -> Html {
    let Some(work) = render.work else {
        return Html::default();
    };
    let groups = work.team_task_groups(team);
    if groups.is_empty() {
        return Html::default();
    }
    html! {
        <nav class="team-tasks" aria-label="Open tasks this team created or claims">
            {render_team_total(team, &groups, render.navigate)}
            {for groups.iter().map(|group| render_team_task_group(team, group, render))}
        </nav>
    }
}

/// The leading tab: only a badge counting every task the rail shows, opening
/// the unified overview with that team's complete current selection.
fn render_team_total(
    team: CanonicalUuid,
    groups: &[crate::work::TeamTaskGroup<'_, '_>],
    navigate: &Callback<navigation::Intent>,
) -> Html {
    use crate::work::{WorkCounts, WorkScope};
    let counts = groups
        .iter()
        .flat_map(|group| group.tasks().iter().map(|task| task.availability()))
        .collect::<WorkCounts>();
    let open = counts.open();
    let title = format!(
        "Open all of this team's work: {open} {}",
        if open == 1 { "task" } else { "tasks" }
    );
    let navigate = navigate.clone();
    let open_team = Callback::from(move |_| {
        navigate.emit(navigation::Intent::Work(WorkScope::team_wide(team)));
    });
    html! {
        <span
            key="team-total"
            class="task-pill task-pill--project task-pill--total"
            data-work-scope="team"
        >
            <button
                class="task-pill-stem-target"
                type="button"
                title={title.clone()}
                aria-label={title}
                onclick={open_team}
            >
                {view::chrome::render_task_count(counts)}
            </button>
        </span>
    }
}

/// One stem tab: the stem target with its count badge, followed by every
/// team-local task handle grouped beneath it.
fn render_team_task_group(
    team: CanonicalUuid,
    group: &crate::work::TeamTaskGroup<'_, '_>,
    render: &FleetRender<'_, '_>,
) -> Html {
    let stem = group.stem().clone();
    let stem_label = stem.stem().to_owned();
    let stem_key = stem.to_string();
    let counts = group
        .tasks()
        .iter()
        .map(|task| task.availability())
        .collect::<crate::work::WorkCounts>();
    let open = counts.open();
    let stem_aria_label = format!(
        "Open this team's {stem} work: {open} {}",
        if open == 1 { "task" } else { "tasks" }
    );
    let visibility = if stem.hidden() { "hidden" } else { "public" };
    let open_stem = {
        let navigate = render.navigate.clone();
        let scope = crate::work::WorkScope::Team {
            team,
            stem: Some(stem),
        };
        Callback::from(move |_| navigate.emit(navigation::Intent::Work(scope.clone())))
    };
    html! {
        <span
            key={stem_key}
            class="task-pill task-pill--project"
            data-task-stem={stem_label.clone()}
            data-task-visibility={visibility}
        >
            <button
                class="task-pill-stem-target"
                type="button"
                title={stem_aria_label.clone()}
                aria-label={stem_aria_label}
                onclick={open_stem}
            >
                <span class="task-pill-stem">{stem_label}</span>
                {view::chrome::render_task_count(counts)}
            </button>
            {for group.tasks().iter().copied().map(|task| render_team_task_handle(task, render))}
        </span>
    }
}

/// One task-detail target inside its stem group.
fn render_team_task_handle(task: &crate::work::WorkTask<'_>, render: &FleetRender<'_, '_>) -> Html {
    use crate::work::WorkAvailability::{InFlight, Ready};
    let statement = &task.row().statement.body;
    let availability = task.availability();
    let owner_accent = task.working_claimant_accent(render.runtime.lanes());
    let handle_label = format!(
        "Open {}: {}, {}",
        statement.handle,
        task.row().presentation_title(),
        availability.key(),
    );
    let open_task = {
        let open = render.open_message_task.clone();
        let born = statement.born.clone();
        Callback::from(move |event: MouseEvent| {
            if let Some(target) = event
                .current_target()
                .and_then(|target| target.dyn_into().ok())
            {
                open.emit((born.clone(), target));
            }
        })
    };
    html! {
        <button
            key={statement.born.to_string()}
            class={classes!(
                "task-pill",
                "task-pill-handle",
                (!matches!(availability, InFlight | Ready)).then_some("dormant"),
            )}
            type="button"
            title={handle_label.clone()}
            aria-label={handle_label}
            data-availability={availability.key()}
            data-accent={owner_accent.map(|accent| accent.to_string())}
            onclick={open_task}
        >{statement.handle.to_string()}</button>
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

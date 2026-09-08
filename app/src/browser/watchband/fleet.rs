//! Org-fleet shell, topology drag, and per-organization toolbar rendering.

use super::*;

/// The media type a dragged organization carries, and the only one an organization accepts.
///
/// Message cards are draggable too, and they carry their prose as `text/plain`
/// so it can be quoted into a composer. An organization surface that accepted plain
/// text would rearrange the fleet because the operator dragged a message onto
/// it, so the gesture is told apart by what it carries rather than by where it
/// lands.
const LINEAGE_DRAG_MEDIA: &str = "application/x-spice-organization";

/// The box a drop is measured against: one organization's whole surface.
const LINEAGE_SURFACE: &str = ".organization-surface";

/// The organization a drop carries, if it carries one at all.
fn dragged_lineage(transfer: &web_sys::DataTransfer) -> Option<CanonicalUuid> {
    transfer.get_data(LINEAGE_DRAG_MEDIA).ok()?.parse().ok()
}

/// Whether a drag in flight carries an organization.
///
/// Only the media types are readable while a drag is moving; the identity
/// itself stays sealed until the drop. That is enough to decide whether this
/// surface is a drop target, which is all `dragover` has to answer.
pub(super) fn carries_lineage(transfer: &web_sys::DataTransfer) -> bool {
    transfer
        .types()
        .iter()
        .any(|media| media.as_string().as_deref() == Some(LINEAGE_DRAG_MEDIA))
}

/// The four one-click views of one live organization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::browser) enum LineageMode {
    Org,
    Tasks,
    #[default]
    Lanes,
}

impl LineageMode {
    pub(super) const fn key(self) -> &'static str {
        match self {
            Self::Org => "Org",
            Self::Tasks => "Tasks",
            Self::Lanes => "Lanes",
        }
    }

    const fn from_destination(destination: OrgDestination) -> Self {
        match destination {
            OrgDestination::Org => Self::Org,
            OrgDestination::Tasks => Self::Tasks,
            OrgDestination::Agents => Self::Lanes,
        }
    }
}

pub(super) struct FleetRender<'a> {
    pub(super) runtime: &'a CanvasRuntime,
    pub(super) epoch: CanonicalUuid,
    pub(super) topology_revision: u64,
    pub(super) can_send: bool,
    pub(super) can_wake: bool,
    pub(super) can_arrange_lanes: bool,
    pub(super) can_arrange_lineages: bool,
    pub(super) can_configure: bool,
    pub(super) topology: Vec<(CanonicalUuid, Vec<CanonicalUuid>)>,
    pub(super) sender: &'a SenderHandle,
    pub(super) drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub(super) submissions: &'a SubmissionHandle,
    pub(super) lineage_controls: &'a UseReducerHandle<LineageControls>,
    pub(super) controls: &'a view::AgentControls,
}

pub(super) fn render_empty_fleet(
    runtime: &CanvasRuntime,
    epoch: CanonicalUuid,
    fleet: &NodeRef,
    lineage_controls: &UseReducerHandle<LineageControls>,
    controls: &view::AgentControls,
) -> Html {
    let empty_lineage = crate::runtime::LaneRow {
        lane: epoch,
        subscription: wire::intent::LineageLife::Steer,
        members: Vec::new(),
    };
    let mode = lineage_controls.empty_mode();
    html! {
        <div ref={fleet.clone()} class="lineage-fleet" data-pane="agent-console" data-view="mosaic"
            tabindex="0" aria-label="Agent organizations">
            <section class="organization-surface" data-lineage-weight="1">
                {render_empty_toolbar(
                    mode,
                    lineage_controls,
                    &[],
                )}
                <div class="organization-pane" data-organization-mode={mode.key()}>
                    {match mode {
                        LineageMode::Lanes => {
                            empty("No agents are assigned. Open Team to add one.")
                        }
                        LineageMode::Tasks => render_tasks(runtime, &empty_lineage, &[]),
                        LineageMode::Org => {
                            render_details(runtime, &empty_lineage, &[], mode, controls)
                        }
                    }}
                </div>
            </section>
        </div>
    }
}

pub(super) fn render_populated_fleet(
    lanes: &[crate::runtime::LaneRow],
    standings: &[watchband::Standing],
    fleet: &NodeRef,
    render: &FleetRender<'_>,
) -> Html {
    let dismiss = lineage_menu_dismissal(render.lineage_controls);
    let escape = lineage_menu_escape(render.lineage_controls);
    html! {
        <div ref={fleet.clone()} class="lineage-fleet" data-pane="agent-console" data-view="mosaic"
            tabindex="0" aria-label="Agent organizations" onclick={dismiss} onkeydown={escape}>
            {for lanes.iter().map(|lineage| {
                if render.lineage_controls.is_closed(lineage.lane) {
                    return Html::default();
                }
                let lineage_standings = standings
                    .iter()
                    .filter(|standing| standing.lineage_id == lineage.lane)
                    .cloned()
                    .collect::<Vec<_>>();
                let mode = render.lineage_controls
                    .current(lineage.lane, lineage.subscription)
                    .mode;
                html! {
                    <section
                        class="organization-surface"
                        data-lineage-id={lineage.lane.to_string()}
                        data-lineage-weight={watchband::lineage_visual_weight(lineage.members.len()).to_string()}
                        ondragover={lineage_dragover(render.can_arrange_lineages)}
                        ondrop={lineage_drop(
                            render.can_arrange_lineages,
                            &render.topology,
                            render.topology_revision,
                            lineage.lane,
                            render.sender,
                        )}
                    >
                        {render_lineage_toolbar(
                            mode,
                            lineage,
                            &lineage_standings,
                            render,
                        )}
                        {view::chrome::render_lineage_ticker(
                            render.runtime,
                            lineage,
                            &lineage_standings,
                        )}
                        <div class="organization-pane" data-organization-mode={mode.key()}>
                            <div
                                class="organization-mode-pane"
                                data-mode="Lanes"
                                data-active={(mode == LineageMode::Lanes).to_string()}
                                aria-hidden={(mode != LineageMode::Lanes).to_string()}
                                inert={(mode != LineageMode::Lanes).then_some("true")}
                            >
                                {render_compose(
                                    render.runtime,
                                    lineage,
                                    &lineage_standings,
                                    render.epoch,
                                    render.topology_revision,
                                    render.can_send,
                                    render.can_wake,
                                    render.can_arrange_lanes,
                                    render.sender,
                                    render.drafts,
                                    render.submissions,
                                    render.lineage_controls,
                                    render.can_configure,
                                    render.controls,
                                )}
                            </div>
                            if mode != LineageMode::Lanes {
                                <div
                                    class="organization-mode-pane"
                                    data-mode={mode.key()}
                                    data-active="true"
                                    aria-hidden="false"
                                >
                                    {match mode {
                                        LineageMode::Tasks => render_tasks(
                                            render.runtime,
                                            lineage,
                                            &lineage_standings,
                                        ),
                        LineageMode::Org => render_details(
                                            render.runtime,
                                            lineage,
                                            &lineage_standings,
                                            mode,
                                            render.controls,
                                        ),
                                        LineageMode::Lanes => Html::default(),
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

fn lineage_menu_dismissal(controls: &UseReducerHandle<LineageControls>) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        let inside_control = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .is_some_and(|target| {
                target
                    .closest(".organization-action-menu, .organization-menu, .organization-lights[data-lane-drag-handle]")
                    .ok()
                    .flatten()
                    .is_some()
            });
        if !inside_control {
            controls.dispatch(LineageControlAction::CloseMenus);
        }
    })
}

fn lineage_menu_escape(controls: &UseReducerHandle<LineageControls>) -> Callback<KeyboardEvent> {
    let controls = controls.clone();
    Callback::from(move |event: KeyboardEvent| {
        if event.key() != "Escape" {
            return;
        }
        if let Some(trigger) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| {
                document
                    .query_selector(
                        ".organization-lights[data-lane-drag-handle][aria-expanded=\"true\"]",
                    )
                    .ok()
                    .flatten()
            })
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = trigger.focus();
        }
        controls.dispatch(LineageControlAction::CloseMenus);
    })
}

/// Pick up an organization from its pip stack, but only where it can be arranged.
///
/// Keeping the gesture on the compact status pips leaves composer prose and
/// activity cards free for their own selection and drag behavior.
fn lineage_drag_start(lineage: CanonicalUuid, can_arrange: bool) -> Callback<DragEvent> {
    let carried = lineage.to_string();
    Callback::from(move |event: DragEvent| {
        if !can_arrange {
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        let _ = transfer.set_data(LINEAGE_DRAG_MEDIA, &carried);
        transfer.set_effect_allowed("move");
    })
}

/// Accept an organization in flight, and nothing else.
fn lineage_dragover(can_arrange: bool) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if !can_arrange {
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if !carries_lineage(&transfer) {
            return;
        }
        transfer.set_drop_effect("move");
        // Only a prevented dragover makes a surface droppable; leaving it
        // alone is how every other drag keeps passing through untouched.
        event.prevent_default();
    })
}

/// Commit the arrangement a drop on this organization states.
///
/// The topology is the one this render drew, captured rather than re-read: the
/// operator arranged the organizations they were looking at, and an arrangement built
/// from a table that changed underneath them would move an organization they never
/// dragged.
fn lineage_drop(
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
        let Some(dragged) = event.data_transfer().as_ref().and_then(dragged_lineage) else {
            return;
        };
        // The listener is delegated, so the element the drop landed on is the
        // only handle on this organization's box; `current_target` is the delegation
        // root and would measure the whole workspace as one organization.
        let Some(surface) = event
            .target()
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            .and_then(|node| node.closest(LINEAGE_SURFACE).ok().flatten())
        else {
            return;
        };
        if surface.get_attribute("data-lineage-id") != Some(target.to_string()) {
            return;
        }
        let rect = surface.get_bounding_client_rect();
        let drop =
            watchband::LineageDrop::at(f64::from(event.client_x()) - rect.left(), rect.width());
        let Some(arranged) = watchband::arrangement(&topology, dragged, target, drop) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        send_action(
            &sender,
            ClientAction::ArrangeLineages {
                topology_revision,
                lineages: arranged
                    .into_iter()
                    .map(|(lineage_id, lanes)| LineageArrangement { lineage_id, lanes })
                    .collect(),
            },
        );
    })
}

fn render_empty_toolbar(
    mode: LineageMode,
    lineage_controls: &UseReducerHandle<LineageControls>,
    standings: &[Standing],
) -> Html {
    html! {
        <header class="organization-toolbar">
            <span
                class="organization-lights"
                role="group"
                aria-label={format!("{} admitted agents", standings.len())}
            ></span>
            {render_mode_rail(mode, None, standings, lineage_controls)}
            <button
                class="organization-menu"
                type="button"
                aria-label="Organization and agent details"
                title="Organization and agent details"
                onclick={{
                    let choose = lineage_controls.clone();
                    Callback::from(move |_| {
                        choose.dispatch(LineageControlAction::SetMode {
                            lineage: None,
                            mode: LineageMode::Org,
                        });
                    })
                }}
            ><span class="organization-menu-icon" aria-hidden="true"></span></button>
        </header>
    }
}

fn render_lineage_toolbar(
    mode: LineageMode,
    lineage: &crate::runtime::LaneRow,
    standings: &[Standing],
    render: &FleetRender<'_>,
) -> Html {
    let pip_stack = crate::pip_layout::PipStack::new(standings.len());
    let can_arrange = render.can_arrange_lineages && render.topology.len() > 1;
    let lineage_id = lineage.lane;
    let arrangement_help_id = format!("organization-arrangement-help-{lineage_id}");
    let lights_label = if can_arrange {
        format!(
            "{} admitted agents; arrange organization {}",
            standings.len(),
            watchband::short(lineage.lane)
        )
    } else {
        format!("{} admitted agents", standings.len())
    };
    let control = render
        .lineage_controls
        .current(lineage.lane, lineage.subscription);
    let actions = lineage_menu::Context {
        controls: render.lineage_controls,
        sender: render.sender,
        topology: &render.topology,
        topology_revision: render.topology_revision,
        can_configure: render.can_configure,
        can_arrange: render.can_arrange_lineages,
    };
    let toggle_arrangement =
        lineage_arrangement_click(lineage_id, can_arrange, render.lineage_controls);
    let keyboard_arrangement =
        lineage_arrangement_key(lineage_id, can_arrange, render.lineage_controls);
    let render_pip = |index: usize| {
        let member_index = watchband::right_edge_ordinal(standings.len(), index);
        let standing = &standings[member_index];
        let member = lineage.member(standing.lane)?;
        let status = member.status(standing.pending);
        let position = pip_stack
            .position(index)
            .expect("standing index belongs to the lane pip stack");
        let label = format!("{}: {}", standing.label, status.label());
        Some(html! {
            <span
                class={classes!("organization-light", status.key())}
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
        <header class="organization-toolbar">
            <span
                class={classes!("organization-lights", (pip_stack.rows() == 1).then_some("single"))}
                role={if can_arrange { "button" } else { "group" }}
                aria-label={lights_label}
                aria-haspopup={can_arrange.then_some("menu")}
                aria-expanded={can_arrange.then(|| control.arrangement_open.to_string())}
                aria-describedby={can_arrange.then(|| arrangement_help_id.clone())}
                tabindex={can_arrange.then_some("0")}
                title={can_arrange.then_some("Drag, or press Enter or Space to arrange this organization")}
                draggable={can_arrange.to_string()}
                data-lane-drag-handle={can_arrange.then_some("")}
                data-lineage-drag-id={can_arrange.then(|| lineage.lane.to_string())}
                ondragstart={lineage_drag_start(lineage.lane, can_arrange)}
                onclick={toggle_arrangement}
                onkeydown={keyboard_arrangement}
            >
                if can_arrange {
                    <span id={arrangement_help_id} class="visually-hidden">
                        {"Press Enter or Space, or tap, to choose where to move or join this organization."}
                    </span>
                }
                {for pip_stack.columns_from_left().map(|column| html! {
                    <span class="organization-pip-column" data-pip-column={column.column.to_string()}>
                        {column.upper.and_then(&render_pip)}
                        {render_pip(column.lower)}
                    </span>
                })}
            </span>
            {render_mode_rail(
                mode,
                Some(lineage.lane),
                standings,
                render.lineage_controls,
            )}
            {lineage_menu::trigger(lineage.lane, &control, render.lineage_controls)}
        </header>
        {lineage_menu::render(lineage, &control, &actions)}
        </>
    }
}

fn render_mode_rail(
    mode: LineageMode,
    lineage: Option<CanonicalUuid>,
    standings: &[Standing],
    lineage_controls: &UseReducerHandle<LineageControls>,
) -> Html {
    html! {
        <nav class="organization-mode-rail" role="tablist" aria-label="Live organization views">
            {for OrgDestination::ALL.map(|destination| {
                let candidate = LineageMode::from_destination(destination);
                let selected = candidate == mode;
                let choose = lineage_controls.clone();
                let badge = mode_badge(candidate, standings);
                html! {
                    <button
                        type="button"
                        role="tab"
                        class={classes!("organization-mode", selected.then_some("selected"))}
                        aria-selected={selected.to_string()}
                        onclick={Callback::from(move |_| {
                            choose.dispatch(LineageControlAction::SetMode {
                                lineage,
                                mode: candidate,
                            });
                        })}
                    >
                        <span class="organization-mode-label" title={destination.label()}>
                            {destination.label()}
                        </span>
                        if let Some(badge) = badge {
                            <span class="organization-mode-badge">{badge}</span>
                        }
                    </button>
                }
            })}
        </nav>
    }
}

fn lineage_arrangement_click(
    lineage: CanonicalUuid,
    can_arrange: bool,
    controls: &UseReducerHandle<LineageControls>,
) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        event.stop_propagation();
        if can_arrange {
            controls.dispatch(LineageControlAction::ToggleArrangement { lineage });
        }
    })
}

fn lineage_arrangement_key(
    lineage: CanonicalUuid,
    can_arrange: bool,
    controls: &UseReducerHandle<LineageControls>,
) -> Callback<KeyboardEvent> {
    let controls = controls.clone();
    Callback::from(move |event: KeyboardEvent| {
        if !can_arrange || !matches!(event.key().as_str(), "Enter" | " ") {
            return;
        }
        event.prevent_default();
        event.stop_propagation();
        controls.dispatch(LineageControlAction::ToggleArrangement { lineage });
    })
}

fn mode_badge(mode: LineageMode, standings: &[Standing]) -> Option<String> {
    let count = match mode {
        LineageMode::Lanes => standings.iter().map(|standing| standing.pending).sum(),
        LineageMode::Tasks => standings
            .iter()
            .filter(|standing| standing.handle.is_some())
            .count(),
        LineageMode::Org => standings.len(),
    };
    watchband::badge(count)
}

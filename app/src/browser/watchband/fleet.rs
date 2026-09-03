//! Team-fleet shell, topology drag, and per-team toolbar rendering.

use super::*;

/// The media type a dragged team carries, and the only one a team accepts.
///
/// Message cards are draggable too, and they carry their prose as `text/plain`
/// so it can be quoted into a composer. A team surface that accepted plain
/// text would rearrange the fleet because the operator dragged a message onto
/// it, so the gesture is told apart by what it carries rather than by where it
/// lands.
const LINEAGE_DRAG_MEDIA: &str = "application/x-spice-team";

/// The box a drop is measured against: one team's whole surface.
const LINEAGE_SURFACE: &str = ".team-surface";

/// The team a drop carries, if it carries one at all.
fn dragged_lineage(transfer: &web_sys::DataTransfer) -> Option<CanonicalUuid> {
    transfer.get_data(LINEAGE_DRAG_MEDIA).ok()?.parse().ok()
}

/// Whether a drag in flight carries a team.
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

/// The four one-click views of one live team.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::browser) enum LineageMode {
    #[default]
    Compose,
    Tasks,
    Metrics,
    Info,
}

impl LineageMode {
    const ALL: [Self; 4] = [Self::Compose, Self::Tasks, Self::Metrics, Self::Info];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Compose => "Compose",
            Self::Tasks => "Tasks",
            Self::Metrics => "Metrics",
            Self::Info => "Info",
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
        <div class="lineage-fleet" data-pane="agent-console" data-view="mosaic">
            <section class="team-surface" data-lineage-weight="1">
                {render_empty_toolbar(
                    mode,
                    lineage_controls,
                    &[],
                )}
                <div class="team-pane" data-team-mode={mode.label()}>
                    {match mode {
                        LineageMode::Compose => {
                            empty("No agents are assigned. Open Info to add one.")
                        }
                        LineageMode::Tasks => render_tasks(runtime, &empty_lineage, &[]),
                        LineageMode::Metrics | LineageMode::Info => {
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
    render: &FleetRender<'_>,
) -> Html {
    html! {
        <div class="lineage-fleet" data-pane="agent-console" data-view="mosaic">
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
                        class="team-surface"
                        data-lineage-id={lineage.lane.to_string()}
                        data-lineage-weight={watchband::lineage_visual_weight(lineage_standings.len()).to_string()}
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
                        <div class="team-pane" data-team-mode={mode.label()}>
                            <div
                                class="team-mode-pane"
                                data-mode="Compose"
                                data-active={(mode == LineageMode::Compose).to_string()}
                                aria-hidden={(mode != LineageMode::Compose).to_string()}
                                inert={(mode != LineageMode::Compose).then_some("true")}
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
                            if mode != LineageMode::Compose {
                                <div
                                    class="team-mode-pane"
                                    data-mode={mode.label()}
                                    data-active="true"
                                    aria-hidden="false"
                                >
                                    {match mode {
                                        LineageMode::Tasks => render_tasks(
                                            render.runtime,
                                            lineage,
                                            &lineage_standings,
                                        ),
                                        LineageMode::Metrics | LineageMode::Info => render_details(
                                            render.runtime,
                                            lineage,
                                            &lineage_standings,
                                            mode,
                                            render.controls,
                                        ),
                                        LineageMode::Compose => Html::default(),
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

/// Let a team be picked up, but only where an arrangement can be committed.
///
/// The handle is the predecessor's: a team is dragged by its own grip rather
/// than by its whole surface, so selecting prose in a composer and lifting a
/// message card out of the activity plane both keep working underneath it.
fn lineage_drag_handle(lineage: CanonicalUuid, can_arrange: bool) -> Html {
    if !can_arrange {
        return Html::default();
    }
    let carried = lineage.to_string();
    html! {
        <span
            class="team-drag-handle"
            data-lane-drag-handle=""
            data-lineage-drag-id={carried.clone()}
            draggable="true"
            role="img"
            aria-label={format!("Drag team {}", watchband::short(lineage))}
            title="Drag onto a team to take its place, or onto a team's edge to join it"
            ondragstart={Callback::from(move |event: DragEvent| {
                let Some(transfer) = event.data_transfer() else {
                    return;
                };
                let _ = transfer.set_data(LINEAGE_DRAG_MEDIA, &carried);
                transfer.set_effect_allowed("move");
            })}
        ><span aria-hidden="true">{"⠿"}</span></span>
    }
}

/// Accept a team in flight, and nothing else.
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

/// Commit the arrangement a drop on this team states.
///
/// The topology is the one this render drew, captured rather than re-read: the
/// operator arranged the teams they were looking at, and an arrangement built
/// from a table that changed underneath them would move a team they never
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
        // only handle on this team's box; `current_target` is the delegation
        // root and would measure the whole workspace as one team.
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
        <header class="team-toolbar">
            <span
                class="team-lights"
                role="group"
                aria-label={format!("{} admitted agents", standings.len())}
            ></span>
            <nav class="team-mode-rail" role="tablist" aria-label="Live team views">
                {for LineageMode::ALL.map(|candidate| {
                    let selected = candidate == mode;
                    let choose = lineage_controls.clone();
                    let badge = mode_badge(candidate, standings);
                    html! {
                        <button
                            type="button"
                            role="tab"
                            class={classes!("team-mode", selected.then_some("selected"))}
                            aria-selected={selected.to_string()}
                            onclick={Callback::from(move |_| {
                                choose.dispatch(LineageControlAction::SetMode {
                                    lineage: None,
                                    mode: candidate,
                                });
                            })}
                        >
                            <span class="team-mode-label" title={candidate.label()}>
                                {candidate.label()}
                            </span>
                            if let Some(badge) = badge {
                                <span class="team-mode-badge">{badge}</span>
                            }
                        </button>
                    }
                })}
            </nav>
            <button
                class="team-menu"
                type="button"
                aria-label="Team and agent details"
                title="Team and agent details"
                onclick={{
                    let choose = lineage_controls.clone();
                    Callback::from(move |_| {
                        choose.dispatch(LineageControlAction::SetMode {
                            lineage: None,
                            mode: LineageMode::Info,
                        });
                    })
                }}
            ><span class="team-menu-icon" aria-hidden="true"></span></button>
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
    html! {
        <>
        <header class="team-toolbar">
            {lineage_drag_handle(lineage.lane, render.can_arrange_lineages)}
            <span
                class="team-lights"
                role="group"
                aria-label={format!("{} admitted agents", standings.len())}
            >
                {for standings.iter().enumerate().filter_map(|(index, standing)| {
                    let member = lineage.member(standing.lane)?;
                    let status = member.status(standing.pending);
                    let position = pip_stack
                        .position(index)
                        .expect("standing index belongs to the lane pip stack");
                    let label = format!("{}: {}", standing.label, status.label());
                    Some(html! {
                        <span
                            class={classes!("team-light", status.key())}
                            role="img"
                            data-lane-light-target-id={member.lane.to_string()}
                            data-lane-status={status.key()}
                            data-pip-column={position.column.to_string()}
                            data-pip-row={position.row.to_string()}
                            aria-label={label.clone()}
                            title={label}
                        ></span>
                    })
                })}
            </span>
            <nav class="team-mode-rail" role="tablist" aria-label="Live team views">
                {for LineageMode::ALL.map(|candidate| {
                    let selected = candidate == mode;
                    let choose = render.lineage_controls.clone();
                    let lineage = Some(lineage.lane);
                    let badge = mode_badge(candidate, standings);
                    html! {
                        <button
                            type="button"
                            role="tab"
                            class={classes!("team-mode", selected.then_some("selected"))}
                            aria-selected={selected.to_string()}
                            onclick={Callback::from(move |_| {
                                choose.dispatch(LineageControlAction::SetMode {
                                    lineage,
                                    mode: candidate,
                                });
                            })}
                        >
                            <span class="team-mode-label" title={candidate.label()}>
                                {candidate.label()}
                            </span>
                            if let Some(badge) = badge {
                                <span class="team-mode-badge">{badge}</span>
                            }
                        </button>
                    }
                })}
            </nav>
            {lineage_menu::trigger(lineage.lane, &control, render.lineage_controls)}
        </header>
        {lineage_menu::render(lineage, &control, &actions)}
        </>
    }
}

fn mode_badge(mode: LineageMode, standings: &[Standing]) -> Option<String> {
    let count = match mode {
        LineageMode::Compose => standings.iter().map(|standing| standing.pending).sum(),
        LineageMode::Tasks => standings
            .iter()
            .filter(|standing| standing.handle.is_some())
            .count(),
        LineageMode::Metrics => standings
            .iter()
            .filter(|standing| standing.held_since.is_some())
            .count(),
        LineageMode::Info => standings.len(),
    };
    watchband::badge(count)
}

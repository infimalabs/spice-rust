//! Canvas surface rendering for the operator browser app.

use super::*;
use crate::work::{WorkProjection, WorkScope};

pub(super) mod chrome;
mod team_menu;
mod work_board;
mod work_detail;

#[derive(Clone)]
pub(super) struct AgentControls {
    pub(super) seat: SeatDraft,
    pub(super) submit_seat: Callback<SubmitEvent>,
    /// Admit manual speech to the one browser-global playback queue.
    pub(super) speak: Callback<SpeechRequest>,
    /// Advance durable narration cursors and admit their live suffix.
    pub(super) narrate: Callback<NarrationBatch>,
    /// Stop and discard automatic speech while preserving explicit playback.
    pub(super) suppress_automatic_speech: Callback<()>,
    /// Exact rendered message currently owned by the speech queue.
    pub(super) speech_message: UseStateHandle<Option<String>>,
}

/// Render the reason the last submission was refused, or nothing.
///
/// The node is absent rather than empty while a submission stands. `role`
/// `alert` is an assertive live region, so an element that were always present
/// would be re-announced empty on every keystroke; leaving it out until there
/// is a sentence makes the announcement coincide with the refusal.
fn render_notice(notice: &Notice) -> Html {
    if !notice.is_some() {
        return Html::default();
    }
    html! {
        <p class="form-refusal" role="alert">{notice.to_string()}</p>
    }
}

/// Pending direction count shared by compact and expanded presence indicators.
pub(super) fn member_pending(
    waiting: &[wire::intent::LaneInbox],
    submissions: &SubmissionStatus,
    lane: CanonicalUuid,
) -> usize {
    let backend = waiting
        .iter()
        .find(|row| row.lane == lane)
        .map(|row| row.pending.as_slice())
        .unwrap_or(&[]);
    submissions.pending(lane, backend)
}

/// One shared live-state marker for global and team action menus.
pub(super) fn render_member_presence(status: crate::runtime::LaneStatus) -> Html {
    let label = status.label();
    html! {
        <span
            class={classes!("agent-presence-indicator", "canvas-member-presence", status.key())}
            role="img"
            data-lane-status={status.key()}
            aria-label={label}
            title={label}
        ></span>
    }
}

pub(super) struct CanvasRender<'a> {
    pub connection: &'a UseStateHandle<Connection>,
    pub surface: &'a UseStateHandle<SurfaceData>,
    pub axis: &'a UseStateHandle<Axis>,
    pub mosaic: &'a UseStateHandle<bool>,
    pub canvas_menu_open: &'a UseStateHandle<bool>,
    pub work_scope: &'a UseStateHandle<WorkScope>,
    pub work_scope_notice: &'a UseStateHandle<Option<String>>,
    pub identity: &'a UseStateHandle<Option<CanvasIdentity>>,
    pub canvas_update: &'a UseStateHandle<Option<CanvasUpdate>>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub team_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub selected_task: &'a UseStateHandle<Option<wire::TaskStamp>>,
    pub detail_close: &'a NodeRef,
    pub note_text: &'a UseStateHandle<String>,
    pub note_notice: &'a UseStateHandle<Notice>,
    pub note_submission: &'a UseStateHandle<NoteSubmission>,
    pub action_issue: &'a UseStateHandle<Option<String>>,
    pub speech_status: &'a UseStateHandle<SpeechStatus>,
    pub submit_note: Callback<SubmitEvent>,
    pub select_task: Callback<wire::TaskStamp>,
    pub close_task: Callback<()>,
    pub refresh: Callback<MouseEvent>,
}

pub(super) fn render_canvas(render: CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let active_axis = **render.axis;
    let surface = render.surface.read();
    let cursor = surface
        .as_ref()
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let topology_revision = surface
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| lanes.topology_revision.to_string());
    html! {
        <div
            class={classes!("shell", (**render.mosaic).then_some("shell--mosaic"))}
            data-topology-revision={topology_revision}
            data-unsafe-intent={render.unsafe_intent.to_string()}
            data-submission-count={render.submissions.status.len().to_string()}
        >
            {chrome::render_command_bar(chrome::CommandBarRender {
                connection: render.connection,
                axis: render.axis,
                mosaic: render.mosaic,
                menu_open: render.canvas_menu_open,
                identity: render.identity,
                canvas_update: render.canvas_update,
                work_scope: render.work_scope,
                work_scope_notice: render.work_scope_notice,
                work,
                active_axis,
                surface: render.surface,
                cursor: &cursor,
                sender: render.sender,
                submissions: &render.submissions.status,
                team_controls: render.team_controls,
            })}
            if let Some(notice) = &**render.work_scope_notice {
                <p class="visually-hidden" role="status" aria-live="polite">{notice}</p>
            }
            if let Some(issue) = &**render.action_issue {
                <p class="form-refusal" role="alert">{issue}</p>
            }
            if let SpeechStatus::Failed(issue) = &**render.speech_status {
                <p class="form-refusal" role="alert">{issue}</p>
            }
            {render_workspace(&render, active_axis, work)}
        </div>
    }
}

pub(super) fn render_workspace(
    render: &CanvasRender<'_>,
    active_axis: Axis,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    let mosaic_active = **render.mosaic;
    html! {
        <>
            <div
                key="agents"
                class="workspace mosaic-workspace"
                hidden={!mosaic_active}
                aria-hidden={(!mosaic_active).to_string()}
                inert={(!mosaic_active).then_some("true")}
                data-active={mosaic_active.to_string()}
            >
                <main class={classes!("mosaic-content", mosaic_active.then_some("content"))}>
                    {watchband::render_team(render)}
                </main>
            </div>
            if !mosaic_active {
                <div key="analytics" class="workspace">
                <header class="topbar">
                    <div class="mobile-brand">{"Spice Canvas"}</div>
                    <div class="topbar-actions">
                        <button
                            class="refresh"
                            type="button"
                            onclick={render.refresh.clone()}
                        >{"Refresh"}</button>
                    </div>
                </header>
                <main class="content">
                    {render_analytics(render, active_axis, work)}
                </main>
                </div>
            }
        </>
    }
}

fn render_analytics(
    render: &CanvasRender<'_>,
    active_axis: Axis,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    html! {
        <>
            <div class="page-heading">
                <div>
                    <h1>{analytics_heading(active_axis, work, render.work_scope)}</h1>
                    <p>{analytics_summary(active_axis, work, render.work_scope)}</p>
                </div>
            </div>
            <div class="pane-strip" aria-label="Panes on this axis">
                {for active_axis.panes().map(|pane| html! {
                    <span class="pane-chip">{pane.label()}</span>
                })}
            </div>
            {render_axis(active_axis, render, work)}
        </>
    }
}

pub(super) const fn axis_label(axis: Axis) -> &'static str {
    CanvasDestination::for_axis(axis).label()
}
fn analytics_heading(axis: Axis, work: Option<&WorkProjection<'_>>, scope: &WorkScope) -> String {
    if axis != Axis::Flow {
        return axis_label(axis).to_owned();
    }
    scope
        .stem()
        .and_then(|identity| work.and_then(|projection| projection.stem(identity)))
        .map_or_else(
            || "All open work".to_owned(),
            |stem| format!("{} work", stem.identity()),
        )
}

fn analytics_summary(axis: Axis, work: Option<&WorkProjection<'_>>, scope: &WorkScope) -> String {
    let Some(work) = work.filter(|_| axis == Axis::Flow) else {
        return axis.question().to_owned();
    };
    let counts = work.scope_counts(scope);
    format!(
        "{} open: {} ready, {} active, {} blocked, {} deferred",
        counts.open(),
        counts.ready(),
        counts.in_flight(),
        counts.blocked(),
        counts.deferred(),
    )
}

/// The seat form's fields as the operator has written them so far.
#[derive(Clone)]
pub(super) struct SeatDraft {
    pub(super) name: UseStateHandle<String>,
    pub(super) worktree: UseStateHandle<String>,
    pub(super) vendor: UseStateHandle<String>,
    pub(super) life: UseStateHandle<String>,
    pub(super) model: UseStateHandle<String>,
    pub(super) effort: UseStateHandle<String>,
    pub(super) notice: UseStateHandle<Notice>,
}

pub(super) fn render_seat_panel(runtime: Option<&CanvasRuntime>, controls: &AgentControls) -> Html {
    let can_seat = runtime.is_some_and(|runtime| runtime.offers(CommandKind::LaneSeat));
    let draft = &controls.seat;
    let catalog = runtime.and_then(CanvasRuntime::vendor_catalog);
    let selected = catalog.and_then(|catalog| {
        catalog
            .vendors
            .iter()
            .find(|vendor| vendor.name.as_str() == draft.vendor.trim())
    });
    let ready = can_seat
        && !draft.worktree.trim().is_empty()
        && selected.is_some()
        && !draft.life.trim().is_empty();
    // A model spelling belongs to the vendor that resolves it, so choosing
    // another vendor drops one the operator selected for the previous.
    let choose_vendor = {
        let vendor = draft.vendor.clone();
        let model = draft.model.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            vendor.set(select.value());
            model.set(String::new());
        })
    };
    let default_model = selected.map_or("", |vendor| vendor.default_model.as_str());
    let default_effort = selected.map_or("", |vendor| vendor.default_effort.as_str());
    html! {
        <aside class="seat-panel">
            <form class="seat-form" onsubmit={controls.submit_seat.clone()}>
                <button type="submit" disabled={!ready}>{"Seat agent"}</button>
                <label>
                    <span title="Name">{"Name"}</span>
                    <input type="text" placeholder="Ava"
                        value={(*draft.name).clone()}
                        oninput={state_from_input(draft.name.clone())} />
                </label>
                <label>
                    <span title="Worktree">{"Worktree"}</span>
                    <input type="text" placeholder="/absolute/path/to/worktree"
                        value={(*draft.worktree).clone()}
                        oninput={state_from_input(draft.worktree.clone())} />
                </label>
                <label>
                    <span title="Vendor">{"Vendor"}</span>
                    <select value={(*draft.vendor).clone()} onchange={choose_vendor}>
                        {for catalog.into_iter().flat_map(|catalog| catalog.vendors.iter()).map(|vendor| {
                            let name = vendor.name.to_string();
                            html! {
                                <option
                                    value={name.clone()}
                                    selected={name == *draft.vendor}
                                >{vendor.label.clone()}</option>
                            }
                        })}
                    </select>
                </label>
                <label>
                    <span title="Life">{"Life"}</span>
                    <select value={(*draft.life).clone()}
                        onchange={state_from_select(draft.life.clone())}>
                        {for wire::intent::TeamLife::ALL.iter().map(|life| html! {
                            <option
                                value={life.as_str()}
                                selected={life.as_str() == *draft.life}
                            >{life.as_str()}</option>
                        })}
                    </select>
                </label>
                <label>
                    <span title="Model">{"Model"}</span>
                    <input type="text" list="lane-models"
                        placeholder={format!("vendor default ({default_model})")}
                        value={(*draft.model).clone()}
                        oninput={state_from_input(draft.model.clone())} />
                </label>
                <datalist id="lane-models">
                    <option value={default_model.to_owned()} />
                </datalist>
                <label>
                    <span title="Effort">{"Effort"}</span>
                    <select value={(*draft.effort).clone()}
                        onchange={state_from_select(draft.effort.clone())}>
                        <option value="" selected={draft.effort.is_empty()}>
                            {format!("vendor default ({default_effort})")}
                        </option>
                        {for catalog.into_iter().flat_map(|catalog| catalog.efforts.iter()).map(|effort| html! {
                            <option
                                value={effort.clone()}
                                selected={*effort == **draft.effort}
                            >{effort.clone()}</option>
                        })}
                    </select>
                </label>
                {render_notice(&draft.notice)}
            </form>
        </aside>
    }
}

pub(super) fn render_axis(
    axis: Axis,
    render: &CanvasRender<'_>,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    match axis {
        Axis::Flow => render_flow(render, work),
        Axis::Topology => render_topology(render.surface),
    }
}

pub(super) fn render_flow(render: &CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let Some(work) = work else {
        return match &**render.connection {
            Connection::Starting | Connection::Connecting => {
                empty("Loading the current board snapshot.")
            }
            Connection::Offline(_) => empty("Board snapshot unavailable while reconnecting."),
            Connection::Online { .. } => empty("No board snapshot is available."),
        };
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let can_note = runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskNote));
    let scoped_stem = render
        .work_scope
        .stem()
        .filter(|identity| work.stem(identity).is_some());
    let counts = work.scope_counts(render.work_scope);
    let scope_key = scoped_stem.map_or_else(|| "all".to_owned(), ToString::to_string);
    let stale = matches!(&**render.connection, Connection::Offline(_));
    let selected = (**render.selected_task).as_ref();
    let detail = selected.and_then(|task| runtime.and_then(|runtime| runtime.task_detail(task)));
    html! {
        <div class="work-layout" data-work-scope={scope_key}>
            <section tabindex="-1" class="section work-board-section">
                <div class="section-heading">
                    <h2>{"Open tasks"}</h2>
                    <span class="section-count">{format!("{} tasks", counts.open())}</span>
                </div>
                {work_board::render(
                    work,
                    render.work_scope,
                    stale,
                    selected,
                    render.select_task.clone(),
                )}
            </section>
            {work_detail::render(work_detail::WorkDetailRender {
                work,
                scope: render.work_scope,
                selected,
                detail,
                stale,
                can_note,
                close_ref: render.detail_close,
                note_text: render.note_text,
                note_notice: render.note_notice,
                note_submission: render.note_submission,
                submit_note: render.submit_note.clone(),
                select_task: render.select_task.clone(),
                close: render.close_task.clone(),
            })}
        </div>
    }
}

pub(super) fn render_topology(surface: &SurfaceData) -> Html {
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted topology is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No agent snapshot is available.");
    };
    html! {
        <>
            <section class="section">
                <div class="section-heading">
                    <h2>{"Resident topology"}</h2>
                    <span class="section-count">{format!("{} agents", lanes.lanes.len())}</span>
                </div>
                if lanes.lanes.is_empty() {
                    {empty("No agents are seated.")}
                } else {
                    <div class="table-wrap"><table>
                        <thead><tr>
                            <th>{"Agent"}</th><th>{"Standing"}</th><th>{"Occupant"}</th>
                            <th>{"Worktree"}</th><th>{"Vendor"}</th>
                        </tr></thead>
                        <tbody>{for lanes.lanes.iter().flat_map(|team| {
                            team.members.iter().map(move |member| (team, member))
                        }).map(|(team, member)| html! {
                            <tr>
                                <td class="mono">{short(&team.lane.to_string())}</td>
                                <td><span class="tag active">{member.disposition.clone()}</span></td>
                                <td class="mono">{
                                    member
                                        .occupant
                                        .map(|value| short(&value.to_string()))
                                        .unwrap_or_else(|| "vacant".to_owned())
                                }</td>
                                <td class="mono">{member.worktree.clone()}</td>
                                <td>{member.vendor.to_string()}</td>
                            </tr>
                        })}</tbody>
                    </table></div>
                }
            </section>
        </>
    }
}

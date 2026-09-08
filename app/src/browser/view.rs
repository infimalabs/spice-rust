//! Canvas surface rendering for the operator browser app.

use super::*;

pub(super) mod chrome;
mod lineage_menu;

#[derive(Clone)]
pub(super) struct AgentControls {
    pub(super) seat_name: UseStateHandle<String>,
    pub(super) seat_worktree: UseStateHandle<String>,
    pub(super) seat_vendor: UseStateHandle<String>,
    pub(super) seat_life: UseStateHandle<String>,
    pub(super) seat_notice: UseStateHandle<Notice>,
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

pub(super) struct CanvasRender<'a> {
    pub connection: &'a UseStateHandle<Connection>,
    pub surface: &'a UseStateHandle<SurfaceData>,
    pub axis: &'a UseStateHandle<Axis>,
    pub mosaic: &'a UseStateHandle<bool>,
    pub canvas_menu_open: &'a UseStateHandle<bool>,
    pub lineage_controls: &'a UseReducerHandle<LineageControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub lineage_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub note_task: &'a UseStateHandle<String>,
    pub note_text: &'a UseStateHandle<String>,
    pub note_notice: &'a UseStateHandle<Notice>,
    pub submit_note: Callback<SubmitEvent>,
    pub refresh: Callback<MouseEvent>,
}

pub(super) fn render_canvas(render: CanvasRender<'_>) -> Html {
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
                axis: render.axis,
                mosaic: render.mosaic,
                menu_open: render.canvas_menu_open,
                active_axis,
                surface: render.surface,
                cursor: &cursor,
                sender: render.sender,
                submissions: &render.submissions.status,
                lineage_controls: render.lineage_controls,
            })}
            {render_workspace(&render, active_axis)}
        </div>
    }
}

pub(super) fn render_workspace(render: &CanvasRender<'_>, active_axis: Axis) -> Html {
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
                    {watchband::render_lineage(
                        render.surface,
                        render.sender,
                        render.composer_drafts,
                        render.submissions,
                        render.lineage_fleet,
                        render.lineage_controls,
                        render.agents,
                    )}
                </main>
            </div>
            if !mosaic_active {
                <div key="analytics" class="workspace">
                <header class="topbar">
                    <div class="mobile-brand">{"Spice Canvas"}</div>
                    <div class={render.connection.class()}>
                        <span class="status-dot"></span>
                        <span class="status-copy">{render.connection.label()}</span>
                    </div>
                    <div class="topbar-actions">
                        <button
                            class="refresh"
                            type="button"
                            onclick={render.refresh.clone()}
                        >{"Refresh"}</button>
                    </div>
                </header>
                <main class="content">
                    {render_analytics(render, active_axis)}
                </main>
                </div>
            }
        </>
    }
}

fn render_analytics(render: &CanvasRender<'_>, active_axis: Axis) -> Html {
    html! {
        <>
            <div class="page-heading">
                <div>
                    <h1>{axis_label(active_axis)}</h1>
                    <p>{active_axis.question()}</p>
                </div>
            </div>
            <div class="pane-strip" aria-label="Panes on this axis">
                {for active_axis.panes().map(|pane| html! {
                    <span class="pane-chip">{pane.label()}</span>
                })}
            </div>
            {render_axis(active_axis, render)}
        </>
    }
}

pub(super) const fn axis_label(axis: Axis) -> &'static str {
    CanvasDestination::for_axis(axis).label()
}

pub(super) fn render_seat_panel(runtime: Option<&CanvasRuntime>, controls: &AgentControls) -> Html {
    let can_seat = runtime.is_some_and(|runtime| runtime.offers(CommandKind::LaneSeat));
    let ready = can_seat
        && !controls.seat_worktree.trim().is_empty()
        && !controls.seat_vendor.trim().is_empty()
        && !controls.seat_life.trim().is_empty();
    html! {
        <aside class="seat-panel">
            <div class="seat-panel-heading">
                <span class="eyebrow">{"Add agent"}</span>
                <h2>{"Assign a worktree"}</h2>
                <p>{"Add one agent member from an existing worktree in this repository."}</p>
            </div>
            <form class="seat-form" onsubmit={controls.submit_seat.clone()}>
                <label for="seat-name">{"Name"}</label>
                <input id="seat-name" type="text" placeholder="Ava"
                    value={(*controls.seat_name).clone()}
                    oninput={state_from_input(controls.seat_name.clone())} />
                <label for="seat-worktree">{"Worktree"}</label>
                <input id="seat-worktree" type="text" placeholder="/absolute/path/to/worktree"
                    value={(*controls.seat_worktree).clone()}
                    oninput={state_from_input(controls.seat_worktree.clone())} />
                <label for="seat-vendor">{"Vendor"}</label>
                <input id="seat-vendor" type="text" value={(*controls.seat_vendor).clone()}
                    oninput={state_from_input(controls.seat_vendor.clone())} />
                <label for="seat-life">{"Life"}</label>
                <input id="seat-life" type="text" list="lane-lives"
                    value={(*controls.seat_life).clone()}
                    oninput={state_from_input(controls.seat_life.clone())} />
                <datalist id="lane-lives">
                    <option value="steer" /><option value="drive" /><option value="drain" />
                </datalist>
                <button type="submit" disabled={!ready}>{"Seat agent"}</button>
                {render_notice(&controls.seat_notice)}
            </form>
        </aside>
    }
}

pub(super) fn render_axis(axis: Axis, render: &CanvasRender<'_>) -> Html {
    match axis {
        Axis::Flow => render_flow(
            render.surface,
            render.note_task,
            render.note_text,
            render.note_notice,
            render.submit_note.clone(),
        ),
        Axis::Topology => render_topology(render.surface),
    }
}

pub(super) fn render_flow(
    surface: &SurfaceData,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    note_notice: &UseStateHandle<Notice>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    let surface = surface.read();
    let Some(board) = surface.as_ref().and_then(CanvasRuntime::board) else {
        return empty("No board snapshot is available.");
    };
    let can_note = surface
        .as_ref()
        .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote));
    let selected = (**note_task).clone();
    let note = (**note_text).clone();
    html! {
        <div class="split">
            <section class="section">
                <div class="section-heading">
                    <h2>{"Current work"}</h2>
                    <span class="section-count">{format!("{} tasks", board.tasks.len())}</span>
                </div>
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Task"}</th><th>{"Phase"}</th><th>{"Owner"}</th><th>{"Project"}</th>
                    </tr></thead>
                    <tbody>{for board.tasks.iter().map(|task| {
                        let statement = &task.statement.body;
                        let workflow = &task.workflow.body;
                        let title = task.presentation_title().to_owned();
                        html! {
                            <tr>
                                <td class="title-cell">
                                    <div class="mono muted">{statement.handle.to_string()}</div>
                                    <div>{title}</div>
                                </td>
                                <td><span class={phase_class(&workflow.status)}>
                                    {workflow.phase.to_string()}
                                </span></td>
                                <td class="mono">{
                                    workflow
                                        .assignment
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| "-".to_owned())
                                }</td>
                                <td>{statement.project.clone()}</td>
                            </tr>
                        }
                    })}</tbody>
                </table></div>
            </section>
            if can_note {
                <section class="section">
                    <div class="section-heading"><h2>{"Append observation"}</h2></div>
                    <form class="form" onsubmit={submit_note}>
                    <label for="note-task">{"Task"}</label>
                    <input
                        id="note-task"
                        list="note-tasks"
                        value={selected}
                        placeholder="SPICE-1kKRRsDP"
                        oninput={state_from_input(note_task.clone())}
                    />
                    <datalist id="note-tasks">
                        {for board.tasks.iter().map(|task| {
                            let statement = &task.statement.body;
                            let title = task.presentation_title().to_owned();
                            html! {
                                <option value={statement.handle.to_string()}>
                                    {title}
                                </option>
                            }
                        })}
                    </datalist>
                    <label for="note-body">{"Observation"}</label>
                    <textarea
                        id="note-body"
                        value={note}
                        oninput={state_from_textarea(note_text.clone())}
                    ></textarea>
                    <button
                        type="submit"
                        disabled={note_task.is_empty() || note_text.trim().is_empty()}
                    >{"Add note"}</button>
                    {render_notice(note_notice)}
                    </form>
                </section>
            }
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
                        <tbody>{for lanes.lanes.iter().flat_map(|lineage| {
                            lineage.members.iter().map(move |member| (lineage, member))
                        }).map(|(lineage, member)| html! {
                            <tr>
                                <td class="mono">{short(&lineage.lane.to_string())}</td>
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

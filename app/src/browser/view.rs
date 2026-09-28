//! Canvas surface rendering for the operator browser app.

use super::*;
use crate::work::{WorkArrangement, WorkProjection, WorkScope};

pub(super) mod chrome;
mod quota;
mod team_menu;
mod work_board;
mod work_detail;
mod work_map;

pub(super) use work_map::WorkMapState;

#[derive(Clone)]
pub(super) struct AgentControls {
    pub(super) seat: SeatDraft,
    pub(super) submit_seat: Callback<SubmitEvent>,
    /// Admit manual speech to the one browser-global playback queue.
    pub(super) speak: Callback<SpeechRequest>,
    /// Advance durable narration cursors and admit their live prefix.
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
pub(super) fn render_notice(notice: &Notice) -> Html {
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
    pub destination: CanvasDestination,
    pub card_target_notice: Option<String>,
    pub navigate: &'a Callback<navigation::Intent>,
    pub canvas_menu_open: &'a UseStateHandle<bool>,
    pub work_scope: &'a UseStateHandle<WorkScope>,
    pub work_scope_notice: &'a UseStateHandle<Option<String>>,
    pub work_arrangement: &'a UseStateHandle<WorkArrangement>,
    pub identity: &'a UseStateHandle<Option<CanvasIdentity>>,
    pub canvas_update: &'a UseStateHandle<Option<CanvasUpdate>>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub composer_drag: &'a super::watchband::PrimaryComposerDragState,
    pub team_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub selected_task: &'a UseStateHandle<Option<wire::TaskStamp>>,
    pub detail_presented: bool,
    pub detail_closing: bool,
    pub work_map: &'a UseStateHandle<WorkMapState>,
    pub work_history: &'a work_history::Controls,
    pub detail_close: &'a NodeRef,
    pub detail_sheet: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub action_issue: &'a UseStateHandle<Option<String>>,
    pub speech_status: &'a UseStateHandle<SpeechStatus>,
    pub submit_note: Callback<(wire::TaskStamp, SubmitEvent)>,
    pub select_task: Callback<wire::TaskStamp>,
    pub close_task: Callback<()>,
    pub retry_relations: Callback<MouseEvent>,
}

pub(super) fn render_canvas(render: CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
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
            class={classes!(
                "shell",
                (render.destination == CanvasDestination::Chat).then_some("shell--mosaic"),
            )}
            data-topology-revision={topology_revision}
            data-unsafe-intent={render.unsafe_intent.to_string()}
            data-submission-count={render.submissions.status.len().to_string()}
        >
            {chrome::render_command_bar(chrome::CommandBarRender {
                connection: render.connection,
                destination: render.destination,
                menu_open: render.canvas_menu_open,
                navigate: render.navigate,
                identity: render.identity,
                canvas_update: render.canvas_update,
                work_scope: render.work_scope,
                work,
                surface: render.surface,
                cursor: &cursor,
                sender: render.sender,
                submissions: &render.submissions.status,
                team_controls: render.team_controls,
            })}
            {render_review_feedback(surface.as_ref(), &render.select_task)}
            if let Some(notice) = &**render.work_scope_notice {
                <p class="visually-hidden" role="status" aria-live="polite">{notice}</p>
            }
            if let Some(issue) = &**render.action_issue {
                <p class="form-refusal" role="alert">{issue}</p>
            }
            if let Some(notice) = &render.card_target_notice {
                <p
                    class="visually-hidden"
                    role="status"
                    aria-live="polite"
                    data-card-target-status=""
                >{notice.clone()}</p>
            }
            if let SpeechStatus::Failed(issue) = &**render.speech_status {
                <p class="form-refusal" role="alert">{issue}</p>
            }
            {render_workspace(&render, work)}
            {render_task_detail(&render, work)}
        </div>
    }
}

fn render_review_feedback(
    runtime: Option<&CanvasRuntime>,
    select_task: &Callback<wire::TaskStamp>,
) -> Html {
    let Some(runtime) = runtime else {
        return Html::default();
    };
    if runtime.review_feedback().is_empty() {
        return Html::default();
    }
    html! {
        <section class="review-feedback-list" aria-live="polite" aria-label="Peer review feedback">
            {for runtime.review_feedback().iter().rev().map(|feedback| {
                let task = feedback.task().clone();
                let task_label = runtime
                    .board()
                    .and_then(|board| {
                        board.tasks.iter().find(|candidate| {
                            candidate.statement.body.born == task
                        })
                    })
                    .map_or_else(|| task.to_string(), |task| task.statement.body.handle.to_string());
                let open = {
                    let select_task = select_task.clone();
                    Callback::from(move |_| select_task.emit(task.clone()))
                };
                html! {
                    <article
                        class="review-feedback-notice"
                        data-review-fact={feedback.seq().to_string()}
                        data-review-task={feedback.task().to_string()}
                    >
                        <header>
                            <strong class="review-feedback-source" aria-label="Spice">{"🌶️"}</strong>
                            <span class="review-feedback-verdict">{"Peer review finding"}</span>
                            <button
                                type="button"
                                class="review-feedback-task"
                                aria-label={format!("Open {task_label} details")}
                                onclick={open}
                            >{task_label}</button>
                        </header>
                        <blockquote><p>{feedback.text().to_owned()}</p></blockquote>
                    </article>
                }
            })}
        </section>
    }
}

pub(super) fn render_workspace(
    render: &CanvasRender<'_>,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    let mosaic_active = render.destination == CanvasDestination::Chat;
    let active_axis = render.destination.axis();
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
            if let Some(active_axis) = active_axis {
                <div key="analytics" class="workspace">
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
    pub(super) vendor: UseStateHandle<String>,
    pub(super) life: UseStateHandle<String>,
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
    let ready = can_seat && selected.is_some() && !draft.life.trim().is_empty();
    // A model spelling belongs to the vendor that resolves it, so choosing
    // another vendor drops one the operator selected for the previous.
    let choose_vendor = {
        let vendor = draft.vendor.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            vendor.set(select.value());
            if let Some(model) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id("seat-model"))
                .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                model.set_value("");
            }
        })
    };
    let default_model = selected.map_or("", |vendor| vendor.default_model.as_str());
    let default_effort = selected.map_or("", |vendor| vendor.default_effort.as_str());
    html! {
        <aside class="seat-panel">
            <form class="seat-form" onsubmit={controls.submit_seat.clone()}>
                <button type="submit" class="canvas-compact-control" disabled={!ready}>{"Seat agent"}</button>
                <label>
                    <span title="Name">{"Name"}</span>
                    <input id="seat-name" type="text" placeholder="Ava" />
                </label>
                <label>
                    <span title="Worktree">{"Worktree"}</span>
                    <input id="seat-worktree" type="text" placeholder="/absolute/path/to/worktree" />
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
                    <input id="seat-model" type="text" list="lane-models"
                        placeholder={format!("vendor default ({default_model})")} />
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
    let scoped_stem = render
        .work_scope
        .stem()
        .filter(|identity| work.stem(identity).is_some());
    let scope_key = scoped_stem.map_or_else(|| "all".to_owned(), ToString::to_string);
    let stale = matches!(&**render.connection, Connection::Offline(_));
    let selected = (**render.selected_task).as_ref();
    let toggle_map_root = work_map::toggle_root_callback(render.work_map.clone());
    html! {
        <div class="work-layout" data-work-scope={scope_key}>
            <section tabindex="-1" class="section work-board-section">
                {work_board::render(
                    work,
                    render.work_scope,
                    stale,
                    selected,
                    work_board::WorkBoardControls {
                        map: render.work_map,
                        select: render.select_task.clone(),
                        toggle_map_root,
                        arrangement: render.work_arrangement,
                        lanes: runtime.and_then(CanvasRuntime::lanes),
                        history_state: runtime.map(CanvasRuntime::work_history),
                        history: render.work_history,
                    },
                )}
            </section>
        </div>
    }
}

fn render_task_detail(render: &CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let Some(work) = work else {
        return Html::default();
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let selected = (**render.selected_task).as_ref();
    let detail = selected.and_then(|task| runtime.and_then(|runtime| runtime.task_detail(task)));
    let relation_key = selected.map(|selected| render.work_map.relations_key(selected));
    let relations = relation_key
        .as_ref()
        .and_then(|key| runtime.and_then(|runtime| runtime.task_relations(key)));
    work_detail::render(work_detail::WorkDetailRender {
        work,
        scope: render.work_scope,
        lanes: runtime.and_then(CanvasRuntime::lanes),
        selected,
        presented: render.detail_presented,
        closing: render.detail_closing,
        map: render.work_map,
        detail,
        relations,
        can_read_relations: runtime
            .is_some_and(|runtime| runtime.offers(CommandKind::TaskRelations)),
        stale: matches!(&**render.connection, Connection::Offline(_)),
        can_note: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
        close_ref: render.detail_close,
        sheet_ref: render.detail_sheet,
        notes: render.notes,
        submit_note: render.submit_note.clone(),
        select_task: render.select_task.clone(),
        close: render.close_task.clone(),
        retry_relations: render.retry_relations.clone(),
    })
}

pub(super) fn render_topology(surface: &SurfaceData) -> Html {
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted topology is available.");
    };
    let operator = render_operator(runtime.settings());
    let Some(lanes) = runtime.lanes() else {
        return html! {
            <>
                {operator}
                {empty("No agent snapshot is available.")}
            </>
        };
    };
    let group_count = lanes.lanes.len();
    let agent_count = lanes
        .lanes
        .iter()
        .map(|team| team.members.len())
        .sum::<usize>();
    let occupant_count = lanes
        .lanes
        .iter()
        .flat_map(|team| &team.members)
        .filter(|member| member.occupant.is_some())
        .count();
    let worktree_count = lanes
        .lanes
        .iter()
        .flat_map(|team| &team.members)
        .map(|member| member.worktree.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let vendor_count = lanes
        .lanes
        .iter()
        .flat_map(|team| &team.members)
        .map(|member| member.vendor.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let quota = quota::render_team(Some(runtime));
    html! {
        <>
            {operator}
            {quota}
            <section class="section team-overview-topology-section">
            <div class="section-heading">
                <div>
                    <h2>{"Resident topology"}</h2>
                    <p class="team-overview-section-copy">
                        {"Resident groups contain independently steerable agent lanes."}
                    </p>
                </div>
                <span class="section-count">{
                    format!(
                        "{} across {}",
                        topology_count(agent_count, "agent lane", "agent lanes"),
                        topology_count(group_count, "group", "groups"),
                    )
                }</span>
            </div>
            <div class="team-overview-summary" aria-label="Resident topology totals">
                {topology_stat(group_count, "Group", "Groups")}
                {topology_stat(agent_count, "Agent lane", "Agent lanes")}
                {topology_stat(occupant_count, "Occupant", "Occupants")}
                {topology_stat(worktree_count, "Worktree", "Worktrees")}
                {topology_stat(vendor_count, "Vendor", "Vendors")}
            </div>
            if lanes.lanes.is_empty() {
                {empty("No agent lanes are connected.")}
            } else {
                <div class="team-overview-topology-groups">
                    {for lanes.lanes.iter().map(render_topology_group)}
                </div>
            }
            </section>
        </>
    }
}

fn render_operator(settings: Option<&wire::intent::GlobalSettingsAnswer>) -> Html {
    let Some(settings) = settings else {
        return empty("Operator identity is loading.");
    };
    let operator = &settings.operator;
    let name_source = if settings.operator_name.is_some() {
        "Explicit setting"
    } else {
        "Git user.name"
    };
    let email_source = if settings.operator_email.is_some() {
        "Explicit setting"
    } else {
        "Git user.email"
    };
    html! {
        <section class="section team-overview-operator-section" data-operator-member="">
            <div class="section-heading">
                <div>
                    <h2>{"You"}</h2>
                    <p class="team-overview-section-copy">
                        {"The operator is the human member of this resident team."}
                    </p>
                </div>
                <span class="section-count">{"Operator"}</span>
            </div>
            <article class="team-overview-operator">
                {render_operator_avatar(operator)}
                <div class="team-overview-operator-identity">
                    <h3>{operator.name.as_deref().unwrap_or("Operator")}</h3>
                    <p>{operator.email.as_deref().unwrap_or("No email configured")}</p>
                    <dl class="team-overview-agent-facts">
                        <div>
                            <dt>{"Badge"}</dt>
                            <dd>{operator.initials.as_deref().unwrap_or("Not available")}</dd>
                        </div>
                        <div>
                            <dt>{"Name source"}</dt>
                            <dd>{name_source}</dd>
                        </div>
                        <div>
                            <dt>{"Email source"}</dt>
                            <dd>{email_source}</dd>
                        </div>
                    </dl>
                </div>
            </article>
        </section>
    }
}

pub(super) fn render_operator_avatar(operator: &wire::OperatorIdentity) -> Html {
    let label = operator.name.as_deref().unwrap_or("Operator");
    match operator.gravatar.as_ref() {
        Some(gravatar) => html! {
            <span class="operator-avatar" data-avatar-source="gravatar">
                <img
                    src={format!("https://gravatar.com/avatar/{}?s=160&d=retro", gravatar.as_str())}
                    alt={format!("{label} Gravatar")}
                />
            </span>
        },
        None => html! {
            <span
                class="operator-avatar"
                data-avatar-source="initials"
                role="img"
                aria-label={format!("{label} badge")}
            >{operator.initials.as_deref().unwrap_or("?")}</span>
        },
    }
}

fn topology_count(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn topology_stat(count: usize, singular: &str, plural: &str) -> Html {
    html! {
        <div class="team-overview-stat">
            <strong>{count}</strong>
            <span>{if count == 1 { singular } else { plural }}</span>
        </div>
    }
}

fn render_topology_group(team: &crate::runtime::LaneRow) -> Html {
    let identity = team.lane.to_string();
    let host = team.host();
    let host_label = host
        .map(|member| {
            crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
        })
        .unwrap_or("Empty");
    let teamed = team.teamed();
    let heading = if teamed {
        format!("{host_label} team")
    } else {
        host_label.to_owned()
    };
    html! {
        <article
            class="team-overview-topology-group"
            data-topology-group={identity.clone()}
            data-group-kind={if teamed { "team" } else { "independent" }}
        >
            <header class="team-overview-group-heading">
                <div>
                    <span class="team-overview-group-kind">
                        {if teamed { "Team" } else { "Independent lane" }}
                    </span>
                    <h3>{heading}</h3>
                    <code
                        class="team-overview-identity"
                        title={identity.clone()}
                        aria-label={format!("Group identity {identity}")}
                        tabindex="0"
                    >{format!("Group {}", short(&identity))}</code>
                </div>
                <span class="team-overview-group-count">
                    {topology_count(team.members.len(), "agent lane", "agent lanes")}
                </span>
            </header>
            <div class="team-overview-agent-list">
                {for team.members.iter().map(render_topology_agent)}
            </div>
        </article>
    }
}

fn render_topology_agent(member: &crate::runtime::LaneMember) -> Html {
    let label = crate::watchband::agent_message_label(&member.worktree, member.name.as_deref());
    let lane = member.lane.to_string();
    let status = member.status(0);
    let (occupant, occupant_title) = member.occupant.map_or_else(
        || {
            (
                "Vacant".to_owned(),
                "No occupant is currently seated".to_owned(),
            )
        },
        |occupant| {
            let identity = occupant.to_string();
            (short(&identity), identity)
        },
    );
    let vendor = member.model.as_ref().map_or_else(
        || member.vendor.to_string(),
        |model| format!("{} · {model}", member.vendor),
    );
    html! {
        <section
            class="team-overview-agent"
            data-agent-lane={lane.clone()}
            data-lane-status={status.key()}
        >
            <header class="team-overview-agent-heading">
                <div>
                    <span class="team-overview-agent-kind">{"Agent"}</span>
                    <h4>{label}</h4>
                </div>
                <span class={classes!("team-overview-agent-standing", status.key())}>
                    {status.label()}
                </span>
            </header>
            <p class="team-overview-agent-worktree" title={member.worktree.clone()}>
                {member.worktree.clone()}
            </p>
            <dl class="team-overview-agent-facts">
                <div>
                    <dt>{"Agent lane"}</dt>
                    <dd><code
                        class="team-overview-identity"
                        title={lane.clone()}
                        aria-label={format!("Agent lane identity {lane}")}
                        tabindex="0"
                    >{short(&lane)}</code></dd>
                </div>
                <div>
                    <dt>{"Occupant"}</dt>
                    <dd><code
                        class="team-overview-identity"
                        title={occupant_title.clone()}
                        aria-label={format!("Occupant {occupant_title}")}
                        tabindex="0"
                    >{occupant}</code></dd>
                </div>
                <div>
                    <dt>{"Standing"}</dt>
                    <dd title={member.disposition.clone()}>{member.disposition.clone()}</dd>
                </div>
                <div>
                    <dt>{"Vendor"}</dt>
                    <dd title={vendor.clone()}>{vendor}</dd>
                </div>
            </dl>
        </section>
    }
}

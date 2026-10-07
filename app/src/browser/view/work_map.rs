//! Immediate whole-set graph with optional typed relationship enrichment.

use std::rc::Rc;

use wire::TaskStamp;
use wire::intent::{
    TASK_RELATION_DEPTH_LIMIT, TASK_RELATION_NODE_LIMIT, TASK_RELATION_ROOT_LIMIT,
    TaskRelationKind, TaskRelationNodeRef, TaskRelationTruncation,
};
use yew::prelude::*;

use crate::browser::mermaid::MermaidImage;
use crate::work::{
    TaskOverviewSelection, WorkAvailability, WorkProjection, WorkRelationshipDirection, lens,
};
use crate::{AppRuntime, TaskRelationsKey, TaskRelationsState};

mod overlay;

const DEFAULT_DEPTH: u8 = 2;
const DEFAULT_NODES: u16 = 48;
const NODE_STEP: u16 = 32;
const ZOOM_MIN: u16 = 100;
const ZOOM_MAX: u16 = 200;
const ZOOM_STEP: u16 = 25;
const PAN_STEP: i16 = 48;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum WorkMapLens {
    #[default]
    Dependency,
    Origin,
    Review,
    Lifecycle,
}

impl WorkMapLens {
    const fn relation(self) -> TaskRelationKind {
        match self {
            Self::Dependency => TaskRelationKind::Dependency,
            Self::Origin => TaskRelationKind::Origin,
            Self::Review => TaskRelationKind::Review,
            Self::Lifecycle => TaskRelationKind::Lifecycle,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Dependency => "Dependencies",
            Self::Origin => "Lineage",
            Self::Review => "Reviews",
            Self::Lifecycle => "Lifecycle",
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Dependency => "dependency",
            Self::Origin => "origin",
            Self::Review => "review",
            Self::Lifecycle => "lifecycle",
        }
    }

    const fn context(self) -> &'static str {
        match self {
            Self::Dependency => {
                "Dependency edges point from each dependent task to the prerequisite it requires."
            }
            Self::Origin => {
                "Task ancestry includes shared parents; non-task ACK and origin identities remain exact."
            }
            Self::Review => "Completed handoffs preserve phase author → reviewer → verdict order.",
            Self::Lifecycle => {
                "Historical task transitions remain ordered by durable fact position."
            }
        }
    }
}

/// Browser-local selection roots and graph controls. The complete selection is
/// never reduced to fit a typed request; only optional enrichment is bounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::browser) struct WorkMapState {
    lens: WorkMapLens,
    direction: WorkRelationshipDirection,
    depth: u8,
    max_nodes: u16,
    zoom: u16,
    pan_x: i16,
    pan_y: i16,
}

impl Default for WorkMapState {
    fn default() -> Self {
        Self {
            lens: WorkMapLens::Dependency,
            direction: WorkRelationshipDirection::Both,
            depth: DEFAULT_DEPTH,
            max_nodes: DEFAULT_NODES,
            zoom: ZOOM_MIN,
            pan_x: 0,
            pan_y: 0,
        }
    }
}

impl WorkMapState {
    pub(in crate::browser) fn request_key(
        &self,
        selection: &TaskOverviewSelection,
    ) -> Option<TaskRelationsKey> {
        let roots = selection
            .tasks()
            .iter()
            .take(TASK_RELATION_ROOT_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        TaskRelationsKey::new(roots, self.lens.relation(), self.depth, self.max_nodes)
    }

    fn reset_controls(&mut self) {
        self.direction = WorkRelationshipDirection::Both;
        self.depth = DEFAULT_DEPTH;
        self.max_nodes = DEFAULT_NODES;
        self.zoom = ZOOM_MIN;
        self.pan_x = 0;
        self.pan_y = 0;
    }

    fn expand(&mut self) {
        self.depth = self.depth.saturating_add(1).min(TASK_RELATION_DEPTH_LIMIT);
        self.max_nodes = self
            .max_nodes
            .saturating_add(NODE_STEP)
            .min(TASK_RELATION_NODE_LIMIT);
    }

    fn zoom(&mut self, increase: bool) {
        self.zoom = if increase {
            self.zoom.saturating_add(ZOOM_STEP).min(ZOOM_MAX)
        } else {
            self.zoom.saturating_sub(ZOOM_STEP).max(ZOOM_MIN)
        };
    }

    fn pan(&mut self, x: i16, y: i16) {
        self.pan_x = self.pan_x.saturating_add(x);
        self.pan_y = self.pan_y.saturating_add(y);
    }
}

pub(super) struct WorkMapRender<'a, 'work> {
    pub selection: &'a TaskOverviewSelection,
    pub state: &'a UseStateHandle<WorkMapState>,
    pub focus_task: Callback<TaskStamp>,
    pub open_overview: Callback<TaskOverviewSelection>,
    pub work: &'a WorkProjection<'work>,
    pub runtime: Option<&'a AppRuntime>,
    pub retry: Callback<MouseEvent>,
}

pub(super) fn render(render: WorkMapRender<'_, '_>) -> Html {
    let local = render.work.overview_graph(render.selection);
    let key = render.state.request_key(render.selection);
    let relations = key.as_ref().and_then(|key| {
        render
            .runtime
            .and_then(|runtime| runtime.task_relations(key))
    });
    let answer = relations.and_then(TaskRelationsState::answer);
    let presentation = lens::project(
        &local,
        render.work,
        answer,
        render.state.lens.relation(),
        render.state.direction,
        render.selection,
    );
    let controls = render_controls(render.state);
    let can_read = render
        .runtime
        .is_some_and(|runtime| runtime.offers(wire::CommandKind::TaskRelations));
    let enrichment_roots = key.as_ref().map_or(0, |key| key.roots().len());
    let activate_task = {
        let selection = render.selection.clone();
        let focus = render.focus_task.clone();
        let open = render.open_overview.clone();
        Callback::from(move |task: TaskStamp| {
            if selection.tasks().contains(&task) {
                focus.emit(task);
            } else {
                open.emit(selection.with_related_task(task));
            }
        })
    };

    html! {
        <section
            class="work-map-content work-overview-map"
            aria-labelledby="work-map-diagram-title"
            data-work-map-roots={render.selection.tasks().len().to_string()}
            data-work-map-context={render.selection.context().label(|team| {
                let lanes = render.runtime.and_then(AppRuntime::lanes);
                crate::watchband::team_label_in(lanes.map_or(&[][..], |view| &view.lanes), team)
            })}
            data-work-map-lens={render.state.lens.key()}
        >
            <div class="work-map-heading">
                <div>
                    <p class="work-detail-kicker">{"Admitted board + typed enrichment"}</p>
                    <h3 id="work-map-diagram-title">{format!("{} graph", render.state.lens.label())}</h3>
                </div>
                <span>{presentation.as_ref().map_or_else(
                    || "Graph projection unavailable".to_owned(),
                    |presentation| format!(
                        "{} selected · {} visible nodes · {} relationships",
                        render.selection.tasks().len(),
                        presentation.nodes.len(),
                        presentation.edges.len(),
                    ),
                )}</span>
            </div>
            {controls}
            <p class="work-map-context">{render.state.lens.context()}</p>
            if render.selection.tasks().len() > enrichment_roots && enrichment_roots > 0 {
                <p class="work-map-bounds" role="status">
                    {format!(
                        "Typed enrichment covers {enrichment_roots} of {} selected roots per request; every selected root remains in this graph.",
                        render.selection.tasks().len(),
                    )}
                </p>
            }
            <crate::browser::toast::Notice text={enrichment_problem(can_read, relations)} />
            if render.selection.is_empty() {
                <div class="work-map-empty" role="status">
                    <strong>{"No tasks selected"}</strong>
                    <span>{"This valid context has no current tasks."}</span>
                </div>
            } else if let Some(presentation) = presentation {
                {render_graph(
                    &presentation,
                    render.state,
                    render.work,
                    &activate_task,
                    can_read && matches!(relations, None | Some(TaskRelationsState::Loading)),
                    matches!(relations, Some(TaskRelationsState::Unavailable { .. }))
                        .then(|| render.retry.clone()),
                )}
                {render_relationship_list(&presentation, render.work, render.selection, render.state.lens, &activate_task)}
                {render_truncation(answer.and_then(|answer| answer.truncation.as_ref()), render.state)}
            } else {
                <div class="work-map-problem" role="alert">
                    <h4>{"Relationship graph unavailable"}</h4>
                    <p>{"The admitted enrichment could not be merged without crossing typed identities."}</p>
                </div>
            }
        </section>
    }
}

/// The real problem behind absent enrichment, if any. Loading and refreshing
/// over retained evidence are routine and say nothing.
fn enrichment_problem(can_read: bool, relations: Option<&TaskRelationsState>) -> Option<AttrValue> {
    if !can_read {
        return Some(AttrValue::from(
            "Typed enrichment is unavailable to this capability; the admitted-board graph remains current.",
        ));
    }
    let Some(TaskRelationsState::Unavailable { reason, detail }) = relations else {
        return None;
    };
    Some(
        detail
            .as_ref()
            .map_or_else(
                || format!("Typed enrichment unavailable: {reason}"),
                |detail| format!("Typed enrichment unavailable: {reason}. {detail}"),
            )
            .into(),
    )
}

/// Draw the graph. A first enrichment read holds the frame's ceiling until it
/// answers. Unavailable enrichment is absent content, so its Retry rests inside
/// the graph's own reserved viewport rather than in flow.
fn render_graph(
    presentation: &lens::Presentation,
    state: &WorkMapState,
    work: &WorkProjection<'_>,
    activate: &Callback<TaskStamp>,
    pending: bool,
    retry: Option<Callback<MouseEvent>>,
) -> Html {
    let targets = map_targets(presentation, work);
    let node_ids = Rc::new(targets.iter().map(|target| target.id.clone()).collect());
    let viewport = overlay::ViewportTransform {
        zoom: state.zoom,
        pan_x: state.pan_x,
        pan_y: state.pan_y,
    };
    let render_geometry = Some(overlay::renderer(targets, activate.clone(), viewport));
    html! {
        <div
            class="work-map-viewport"
            aria-busy={pending.then_some("true")}
            data-work-map-zoom={state.zoom.to_string()}
            data-work-map-pan-x={state.pan_x.to_string()}
            data-work-map-pan-y={state.pan_y.to_string()}
        >
            <MermaidImage
                source={AttrValue::from(presentation.source.clone())}
                {node_ids}
                {render_geometry}
                final_answer={true}
            />
            if let Some(retry) = retry {
                <button type="button" class="app-compact-control work-map-retry" onclick={retry}>{"Retry enrichment"}</button>
            }
        </div>
    }
}

fn render_controls(state: &UseStateHandle<WorkMapState>) -> Html {
    let reset = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::reset_controls))
    };
    html! {
        <>
            <div class="work-map-primary-controls">
                <section class="work-map-lenses app-control-rail" aria-label="Relationship lens">
                    {lens_button(state, WorkMapLens::Dependency)}
                    {lens_button(state, WorkMapLens::Origin)}
                    {lens_button(state, WorkMapLens::Review)}
                    {lens_button(state, WorkMapLens::Lifecycle)}
                </section>
                if state.lens == WorkMapLens::Dependency {
                    <div class="work-map-control-group work-map-direction app-control-rail" aria-label="Dependency direction">
                        {direction_button(state, WorkRelationshipDirection::Prerequisites, "Prerequisites")}
                        {direction_button(state, WorkRelationshipDirection::Both, "Both")}
                        {direction_button(state, WorkRelationshipDirection::Dependents, "Dependents")}
                    </div>
                }
            </div>
            <section class="work-map-controls" aria-label="Map controls">
                <div class="work-map-control-group app-control-rail work-map-depth" aria-label="Relationship depth">
                    <button type="button" aria-label="Decrease map depth" disabled={state.depth == 0} onclick={depth_callback(state.clone(), false)}>{"−"}</button>
                    <span>{format!("Depth {}", state.depth)}</span>
                    <button type="button" aria-label="Increase map depth" disabled={state.depth >= TASK_RELATION_DEPTH_LIMIT} onclick={depth_callback(state.clone(), true)}>{"+"}</button>
                </div>
                <div class="work-map-control-group app-control-rail work-map-zoom" aria-label="Map zoom">
                    <button type="button" aria-label="Zoom map out" disabled={state.zoom <= ZOOM_MIN} onclick={viewport_callback(state.clone(), ViewportAction::Zoom(false))}>{"−"}</button>
                    <span>{format!("{}%", state.zoom)}</span>
                    <button type="button" aria-label="Zoom map in" disabled={state.zoom >= ZOOM_MAX} onclick={viewport_callback(state.clone(), ViewportAction::Zoom(true))}>{"+"}</button>
                </div>
                <div class="work-map-control-group app-control-rail work-map-pan" aria-label="Map pan">
                    <button type="button" aria-label="Pan map left" onclick={viewport_callback(state.clone(), ViewportAction::Pan(-PAN_STEP, 0))}>{"←"}</button>
                    <button type="button" aria-label="Pan map up" onclick={viewport_callback(state.clone(), ViewportAction::Pan(0, -PAN_STEP))}>{"↑"}</button>
                    <button type="button" aria-label="Pan map down" onclick={viewport_callback(state.clone(), ViewportAction::Pan(0, PAN_STEP))}>{"↓"}</button>
                    <button type="button" aria-label="Pan map right" onclick={viewport_callback(state.clone(), ViewportAction::Pan(PAN_STEP, 0))}>{"→"}</button>
                </div>
                <div class="work-map-control-group app-control-rail work-map-actions">
                    <button type="button" disabled={is_default_view(state)} onclick={reset}>{"Reset view"}</button>
                </div>
                <span class="work-map-bounds">{format!("Limit {} enrichment nodes", state.max_nodes)}</span>
            </section>
        </>
    }
}

fn lens_button(state: &UseStateHandle<WorkMapState>, lens: WorkMapLens) -> Html {
    let active = state.lens == lens;
    let change = {
        let state = state.clone();
        Callback::from(move |_| update(&state, |next| next.lens = lens))
    };
    html! {
        <button type="button" aria-pressed={active.to_string()} class={active.then_some("active")} onclick={change}>
            {lens.label()}
        </button>
    }
}

fn direction_button(
    state: &UseStateHandle<WorkMapState>,
    direction: WorkRelationshipDirection,
    label: &'static str,
) -> Html {
    let active = state.direction == direction;
    let change = {
        let state = state.clone();
        Callback::from(move |_| update(&state, |next| next.direction = direction))
    };
    html! {
        <button type="button" aria-pressed={active.to_string()} class={active.then_some("active")} onclick={change}>{label}</button>
    }
}

fn depth_callback(state: UseStateHandle<WorkMapState>, increase: bool) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        update(&state, |next| {
            next.depth = if increase {
                next.depth.saturating_add(1).min(TASK_RELATION_DEPTH_LIMIT)
            } else {
                next.depth.saturating_sub(1)
            };
        });
    })
}

#[derive(Clone, Copy)]
enum ViewportAction {
    Zoom(bool),
    Pan(i16, i16),
}

fn viewport_callback(
    state: UseStateHandle<WorkMapState>,
    action: ViewportAction,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        update(&state, |next| match action {
            ViewportAction::Zoom(increase) => next.zoom(increase),
            ViewportAction::Pan(x, y) => next.pan(x, y),
        });
    })
}

fn is_default_view(state: &WorkMapState) -> bool {
    state.direction == WorkRelationshipDirection::Both
        && state.depth == DEFAULT_DEPTH
        && state.max_nodes == DEFAULT_NODES
        && state.zoom == ZOOM_MIN
        && state.pan_x == 0
        && state.pan_y == 0
}

fn map_targets(
    presentation: &lens::Presentation,
    work: &WorkProjection<'_>,
) -> Rc<Vec<overlay::MapNodeTarget>> {
    Rc::new(
        presentation
            .nodes
            .iter()
            .filter_map(|node| {
                let task = node.task.as_ref()?;
                let tone = task_tone(work, task);
                Some(overlay::MapNodeTarget {
                    id: node.id.clone(),
                    task: task.clone(),
                    label: node.label.clone(),
                    selected: node.selected,
                    availability: tone.map(|tone| tone.availability),
                    disposition: tone.and_then(|tone| tone.disposition),
                    state: tone.map(|tone| tone.label),
                })
            })
            .collect(),
    )
}

#[derive(Clone, Copy)]
struct TaskTone {
    availability: &'static str,
    disposition: Option<&'static str>,
    label: &'static str,
}

fn task_tone(work: &WorkProjection<'_>, task: &TaskStamp) -> Option<TaskTone> {
    let task = work
        .tasks()
        .iter()
        .find(|candidate| candidate.row().statement.body.born == *task)?;
    let availability = task.availability();
    let (label, disposition) = match availability {
        WorkAvailability::InFlight => ("In flight", None),
        WorkAvailability::Ready => ("Ready", None),
        WorkAvailability::Blocked => ("Blocked", None),
        WorkAvailability::Deferred => ("Deferred", None),
        WorkAvailability::Terminal if task.row().workflow.body.status == "deleted" => {
            ("Deleted", Some("deleted"))
        }
        WorkAvailability::Terminal => ("Completed", None),
    };
    Some(TaskTone {
        availability: availability.key(),
        disposition,
        label,
    })
}

fn render_relationship_list(
    presentation: &lens::Presentation,
    work: &WorkProjection<'_>,
    selection: &TaskOverviewSelection,
    active_lens: WorkMapLens,
    activate: &Callback<TaskStamp>,
) -> Html {
    let (title, note) = match active_lens {
        WorkMapLens::Dependency => (
            "Tasks in this dependency graph",
            "Selected roots first; follow task buttons to continue the lead",
        ),
        WorkMapLens::Origin => (
            "Lineage evidence",
            "Selected roots first; task, ACK, and origin identities stay explicit",
        ),
        WorkMapLens::Review => (
            "Review chain evidence",
            "Phase authors, reviewers, and verdicts remain in graph order",
        ),
        WorkMapLens::Lifecycle => (
            "Ordered lifecycle evidence",
            "Durable lifecycle facts remain in source order",
        ),
    };
    html! {
        <section class="work-map-list" aria-labelledby="work-map-list-title">
            <div class="work-map-heading">
                <h3 id="work-map-list-title">{title}</h3>
                <span>{note}</span>
            </div>
            <div class="work-map-task-list">
                {for presentation.nodes.iter().map(|node| {
                    if let Some(task) = &node.task {
                        let activate = {
                            let activate = activate.clone();
                            let task = task.clone();
                            Callback::from(move |_| activate.emit(task.clone()))
                        };
                        let tone = task_tone(work, task);
                        html! {
                            <button
                                key={node.id.clone()}
                                type="button"
                                class="work-map-task"
                                data-task-stamp={task.to_string()}
                                data-task-selected={selection.tasks().contains(task).to_string()}
                                data-work-availability={tone.map(|tone| tone.availability)}
                                data-work-disposition={tone.and_then(|tone| tone.disposition)}
                                data-work-state={tone.map(|tone| tone.label)}
                                onclick={activate}
                            >
                                <span>{node.kind}</span>
                                <strong>{node.label.clone()}</strong>
                                <small>{node.detail.clone()}</small>
                            </button>
                        }
                    } else {
                        html! {
                            <article key={node.id.clone()} class="work-map-fact">
                                <span>{node.kind}</span>
                                <strong>{node.label.clone()}</strong>
                                <small>{node.detail.clone()}</small>
                            </article>
                        }
                    }
                })}
            </div>
        </section>
    }
}

fn render_truncation(
    truncation: Option<&TaskRelationTruncation>,
    state: &UseStateHandle<WorkMapState>,
) -> Html {
    let Some(truncation) = truncation else {
        return Html::default();
    };
    let total = u32::try_from(truncation.frontier.len())
        .unwrap_or(u32::MAX)
        .saturating_add(truncation.omitted);
    let cannot_expand =
        state.depth >= TASK_RELATION_DEPTH_LIMIT && state.max_nodes >= TASK_RELATION_NODE_LIMIT;
    let expand = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::expand))
    };
    html! {
        <section class="work-map-frontiers" aria-labelledby="work-map-frontiers-title">
            <div class="work-map-heading">
                <h3 id="work-map-frontiers-title">{"Truncated relationships"}</h3>
                <span>{format!("{total} omitted · nothing silently hidden")}</span>
            </div>
            <ul class="work-map-frontier-list">
                {for truncation.frontier.iter().map(|frontier| html! {
                    <li>{format!(
                        "Depth {} · {} · {} → {}",
                        frontier.depth,
                        lens::edge_label(frontier.edge.relation),
                        reference_label(&frontier.edge.from),
                        reference_label(&frontier.edge.to),
                    )}</li>
                })}
                if truncation.omitted > 0 {
                    <li>{format!("{} additional frontier edges exceed the metadata bound", truncation.omitted)}</li>
                }
            </ul>
            <button type="button" class="app-compact-control" disabled={cannot_expand} onclick={expand}>{"Expand typed read"}</button>
        </section>
    }
}

fn reference_label(reference: &TaskRelationNodeRef) -> String {
    match reference {
        TaskRelationNodeRef::Task { task } => format!("task {task}"),
        TaskRelationNodeRef::Origin { origin } => format!("origin {origin}"),
        TaskRelationNodeRef::Review { task, position } => {
            format!("task {task} review fact {position}")
        }
        TaskRelationNodeRef::Lifecycle { task, position } => {
            format!("task {task} lifecycle fact {position}")
        }
        TaskRelationNodeRef::Claim { task, position } => {
            format!("task {task} claim fact {position}")
        }
    }
}

fn update(state: &UseStateHandle<WorkMapState>, change: impl FnOnce(&mut WorkMapState)) {
    let mut next = (**state).clone();
    change(&mut next);
    state.set(next);
}

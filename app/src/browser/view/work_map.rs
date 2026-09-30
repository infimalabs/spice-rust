//! Bounded typed relationship lenses for the selected Work task set.

use std::rc::Rc;

use wire::TaskStamp;
use wire::intent::{
    TASK_RELATION_DEPTH_LIMIT, TASK_RELATION_NODE_LIMIT, TaskRelationKind, TaskRelationNodeRef,
    TaskRelationTruncation,
};
use yew::prelude::*;

use crate::browser::mermaid::MermaidImage;
use crate::work::{WorkAvailability, WorkProjection, WorkRelationshipDirection};
use crate::{TaskRelationsKey, TaskRelationsState};

mod lens;
mod overlay;

const DEFAULT_DEPTH: u8 = 2;
const DEFAULT_NODES: u16 = 48;
const NODE_STEP: u16 = 32;
const MAX_PINNED_ROOTS: usize = wire::intent::TASK_RELATION_ROOT_LIMIT - 1;
const ZOOM_MIN: u16 = 100;
const ZOOM_MAX: u16 = 200;
const ZOOM_STEP: u16 = 25;
const PAN_STEP: i16 = 48;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum WorkDetailMode {
    #[default]
    Details,
    Map,
}

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
                "Task ancestry includes siblings reached through their shared task parent; non-task ACK and origin identities remain exact."
            }
            Self::Review => "Completed handoffs preserve phase author → reviewer → verdict order.",
            Self::Lifecycle => {
                "Historical task transitions remain ordered by durable fact position."
            }
        }
    }
}

/// Browser-owned map controls. Durable task stamps and exact request parameters
/// intentionally outlive any one board projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::browser) struct WorkMapState {
    mode: WorkDetailMode,
    pinned: Vec<TaskStamp>,
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
            mode: WorkDetailMode::Details,
            pinned: Vec::new(),
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
    pub(super) const fn mode(&self) -> WorkDetailMode {
        self.mode
    }

    pub(in crate::browser) const fn map_open(&self) -> bool {
        matches!(self.mode, WorkDetailMode::Map)
    }

    pub(in crate::browser) fn pinned(&self, task: &TaskStamp) -> bool {
        self.pinned.contains(task)
    }

    pub(in crate::browser) fn pin_limit_reached(&self) -> bool {
        self.pinned.len() >= MAX_PINNED_ROOTS
    }

    fn roots(&self, selected: &TaskStamp) -> Vec<TaskStamp> {
        let mut roots = self.pinned.clone();
        roots.push(selected.clone());
        roots.sort();
        roots.dedup();
        roots
    }

    pub(in crate::browser) fn relations_key(&self, selected: &TaskStamp) -> TaskRelationsKey {
        TaskRelationsKey::new(
            self.roots(selected),
            self.lens.relation(),
            self.depth,
            self.max_nodes,
        )
        .expect("Work map controls stay inside typed relationship limits")
    }

    pub(in crate::browser) fn request_key(&self, selected: &TaskStamp) -> Option<TaskRelationsKey> {
        (self.mode == WorkDetailMode::Map).then(|| self.relations_key(selected))
    }

    fn toggle_pin(&mut self, task: TaskStamp) {
        if let Some(index) = self.pinned.iter().position(|pinned| pinned == &task) {
            self.pinned.remove(index);
        } else if !self.pin_limit_reached() {
            self.pinned.push(task);
            self.pinned.sort();
        }
    }

    fn center(&mut self) {
        self.pinned.clear();
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
    pub selected: &'a TaskStamp,
    pub state: &'a UseStateHandle<WorkMapState>,
    pub select_task: Callback<TaskStamp>,
    pub relations: Option<&'a TaskRelationsState>,
    pub can_read_relations: bool,
    /// Reads an unavailable lens again.
    pub retry: Callback<MouseEvent>,
    pub work: &'a WorkProjection<'work>,
}

pub(super) fn toggle_root_callback(state: UseStateHandle<WorkMapState>) -> Callback<TaskStamp> {
    Callback::from(move |task| update(&state, |next| next.toggle_pin(task)))
}

pub(super) fn render_tabs(state: &UseStateHandle<WorkMapState>, selected: &TaskStamp) -> Html {
    let details = set_mode_callback(state.clone(), WorkDetailMode::Details);
    let map = set_mode_callback(state.clone(), WorkDetailMode::Map);
    html! {
        <div class="work-detail-tabs canvas-control-rail" role="tablist" aria-label="Task view">
            <button
                id="work-detail-tab"
                type="button"
                role="tab"
                aria-controls="work-detail-panel"
                aria-selected={(state.mode() == WorkDetailMode::Details).to_string()}
                class={(state.mode() == WorkDetailMode::Details).then_some("active")}
                onclick={details}
            >{"Details"}</button>
            <button
                id="work-map-tab"
                type="button"
                role="tab"
                aria-controls="work-map-panel"
                aria-selected={(state.mode() == WorkDetailMode::Map).to_string()}
                class={(state.mode() == WorkDetailMode::Map).then_some("active")}
                onclick={map}
            >
                {format!("Map ({})", state.roots(selected).len())}
            </button>
        </div>
    }
}

pub(super) fn render(render: WorkMapRender<'_, '_>) -> Html {
    let controls = render_controls(render.state, render.selected);
    if !render.can_read_relations {
        return html! {
            <div class="work-map-content">
                {controls}
                <div class="work-map-problem" role="alert">
                    <h3>{"Relationship map unavailable"}</h3>
                    <p>{"This Canvas capability does not offer task::relations."}</p>
                </div>
            </div>
        };
    }
    let answer = match render.relations {
        None | Some(TaskRelationsState::Loading) => {
            return html! {
                <div class="work-map-content">
                    {controls}
                    <div class="work-detail-loading" role="status">
                        <span class="work-detail-spinner" aria-hidden="true"></span>
                        <p>{format!("Loading the {} lens…", render.state.lens.label())}</p>
                    </div>
                </div>
            };
        }
        Some(TaskRelationsState::Unavailable { reason, detail }) => {
            return html! {
                <div class="work-map-content">
                    {controls}
                    <div class="work-map-problem" role="alert">
                        <h3>{"Relationship map unavailable"}</h3>
                        <p><code>{reason}</code></p>
                        if let Some(detail) = detail { <p>{detail}</p> }
                        <button type="button" class="canvas-compact-control" onclick={render.retry.clone()}>{"Retry lens"}</button>
                    </div>
                </div>
            };
        }
        Some(TaskRelationsState::Available(answer) | TaskRelationsState::Refreshing(answer)) => {
            answer.as_ref()
        }
    };
    let Some(presentation) = lens::project(
        answer,
        render.state.lens.relation(),
        render.state.direction,
        render.selected,
    ) else {
        return html! {
            <div class="work-map-content">
                {controls}
                <div class="work-map-problem" role="alert">
                    <h3>{"Relationship map unavailable"}</h3>
                    <p>{"The admitted answer could not be projected without crossing typed identities."}</p>
                </div>
            </div>
        };
    };
    let targets = map_targets(&presentation, render.work);
    let node_ids = Rc::new(targets.iter().map(|target| target.id.clone()).collect());
    let viewport = overlay::ViewportTransform {
        zoom: render.state.zoom,
        pan_x: render.state.pan_x,
        pan_y: render.state.pan_y,
    };
    let render_geometry = Some(overlay::renderer(
        targets,
        render.select_task.clone(),
        viewport,
    ));
    let source = AttrValue::from(presentation.source.clone());
    let related = presentation.nodes.len().saturating_sub(answer.roots.len());
    html! {
        <div
            class="work-map-content"
            data-work-map-roots={answer.roots.len().to_string()}
            data-work-map-lens={render.state.lens.key()}
            data-work-map-snapshot={answer.snapshot.to_string()}
        >
            {controls}
            <p class="work-map-context">{render.state.lens.context()}</p>
            if related == 0 {
                <p class="work-map-empty" role="status">
                    {format!("No {} relationships are visible at the current depth.", render.state.lens.label().to_lowercase())}
                </p>
            }
            <section class="work-map-diagram" aria-labelledby="work-map-diagram-title">
                <div class="work-map-heading">
                    <div>
                        <p class="work-detail-kicker">{"Bounded typed read"}</p>
                        <h3 id="work-map-diagram-title">{format!("{} map", render.state.lens.label())}</h3>
                    </div>
                    <span>{format!("{} nodes · {} edges · snapshot {}", presentation.nodes.len(), presentation.edges.len(), answer.snapshot)}</span>
                </div>
                <div
                    class="work-map-viewport"
                    data-work-map-zoom={render.state.zoom.to_string()}
                    data-work-map-pan-x={render.state.pan_x.to_string()}
                    data-work-map-pan-y={render.state.pan_y.to_string()}
                >
                    <MermaidImage
                        {source}
                        {node_ids}
                        {render_geometry}
                        final_answer={true}
                    />
                </div>
            </section>
            {render_relationship_list(&presentation, render.work, render.selected, &render.select_task)}
            {render_truncation(answer.truncation.as_ref(), render.state)}
        </div>
    }
}

fn set_mode_callback(
    state: UseStateHandle<WorkMapState>,
    mode: WorkDetailMode,
) -> Callback<MouseEvent> {
    Callback::from(move |_| update(&state, |next| next.mode = mode))
}

fn render_controls(state: &UseStateHandle<WorkMapState>, selected: &TaskStamp) -> Html {
    let center = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::center))
    };
    let reset = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::reset_controls))
    };
    html! {
        <>
            <div class="work-map-primary-controls">
                <section class="work-map-lenses canvas-control-rail" aria-label="Relationship lens">
                    {lens_button(state, WorkMapLens::Dependency)}
                    {lens_button(state, WorkMapLens::Origin)}
                    {lens_button(state, WorkMapLens::Review)}
                    {lens_button(state, WorkMapLens::Lifecycle)}
                </section>
                if state.lens == WorkMapLens::Dependency {
                    <div class="work-map-control-group work-map-direction canvas-control-rail" aria-label="Dependency direction">
                        {direction_button(state, WorkRelationshipDirection::Prerequisites, "Prerequisites")}
                        {direction_button(state, WorkRelationshipDirection::Both, "Both")}
                        {direction_button(state, WorkRelationshipDirection::Dependents, "Dependents")}
                    </div>
                }
            </div>
            <section class="work-map-controls" aria-label="Map controls">
                <div class="work-map-control-group canvas-control-rail work-map-depth" aria-label="Relationship depth">
                    <button type="button" aria-label="Decrease map depth" disabled={state.depth == 0} onclick={depth_callback(state.clone(), false)}>{"−"}</button>
                    <span>{format!("Depth {}", state.depth)}</span>
                    <button type="button" aria-label="Increase map depth" disabled={state.depth >= TASK_RELATION_DEPTH_LIMIT} onclick={depth_callback(state.clone(), true)}>{"+"}</button>
                </div>
                {render_viewport_controls(state)}
                <div class="work-map-control-group canvas-control-rail work-map-actions">
                    <button type="button" disabled={state.pinned.is_empty()} onclick={center}>
                        {format!("Center on task {selected}")}
                    </button>
                    <button type="button" disabled={is_default_view(state)} onclick={reset}>{"Reset view"}</button>
                </div>
                <span class="work-map-bounds">{format!("Limit {} nodes", state.max_nodes)}</span>
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
        <button
            type="button"
            aria-pressed={active.to_string()}
            class={active.then_some("active")}
            onclick={change}
        >{lens.label()}</button>
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

fn render_viewport_controls(state: &UseStateHandle<WorkMapState>) -> Html {
    html! {
        <>
            <div class="work-map-control-group canvas-control-rail work-map-zoom" aria-label="Map zoom">
                <button type="button" aria-label="Zoom map out" disabled={state.zoom <= ZOOM_MIN} onclick={viewport_callback(state.clone(), ViewportAction::Zoom(false))}>{"−"}</button>
                <span>{format!("{}%", state.zoom)}</span>
                <button type="button" aria-label="Zoom map in" disabled={state.zoom >= ZOOM_MAX} onclick={viewport_callback(state.clone(), ViewportAction::Zoom(true))}>{"+"}</button>
            </div>
            <div class="work-map-control-group canvas-control-rail work-map-pan" aria-label="Map pan">
                <button type="button" aria-label="Pan map left" onclick={viewport_callback(state.clone(), ViewportAction::Pan(-PAN_STEP, 0))}>{"←"}</button>
                <button type="button" aria-label="Pan map up" onclick={viewport_callback(state.clone(), ViewportAction::Pan(0, -PAN_STEP))}>{"↑"}</button>
                <button type="button" aria-label="Pan map down" onclick={viewport_callback(state.clone(), ViewportAction::Pan(0, PAN_STEP))}>{"↓"}</button>
                <button type="button" aria-label="Pan map right" onclick={viewport_callback(state.clone(), ViewportAction::Pan(PAN_STEP, 0))}>{"→"}</button>
            </div>
        </>
    }
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
                    current: node.current,
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
        // Completed map nodes use the neutral terminal tone, matching the
        // board's History region rather than its completion accent.
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
    selected: &TaskStamp,
    select: &Callback<TaskStamp>,
) -> Html {
    html! {
        <section class="work-map-list" aria-labelledby="work-map-list-title">
            <div class="work-map-heading">
                <h3 id="work-map-list-title">{"Relationships in this map"}</h3>
                <span>{"Typed identities in stable order"}</span>
            </div>
            <div class="work-map-task-list">
                {for presentation.nodes.iter().map(|node| {
                    if let Some(task) = &node.task {
                        let activate = {
                            let select = select.clone();
                            let task = task.clone();
                            Callback::from(move |_| select.emit(task.clone()))
                        };
                        let tone = task_tone(work, task);
                        html! {
                            <button
                                key={node.id.clone()}
                                type="button"
                                class="work-map-task"
                                data-task-stamp={task.to_string()}
                                aria-current={(task == selected).then_some("true")}
                                data-work-availability={tone.map(|tone| tone.availability)}
                                data-work-disposition={tone.and_then(|tone| tone.disposition)}
                                data-work-state={tone.map(|tone| tone.label)}
                                onclick={activate}
                            >
                                <span>{node.kind}</span>
                                <strong>{node.label.clone()}</strong>
                                <small>{node.detail.clone()}</small>
                                if let Some(tone) = tone {
                                    <span class="work-map-task-state">{tone.label}</span>
                                }
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
            <button type="button" class="canvas-compact-control" disabled={cannot_expand} onclick={expand}>{"Expand typed read"}</button>
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

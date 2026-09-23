//! Bounded dependency-map presentation for the selected Work task set.

use std::fmt::Write;
use std::rc::Rc;

use super::super::mermaid::MermaidImage;
use super::*;
use crate::work::{
    WorkAvailability, WorkProjection, WorkRelationshipDirection, WorkRelationshipExpansion,
    WorkRelationshipGraph, WorkRelationshipKind, WorkRelationshipNode, WorkRelationshipOptions,
};
use wire::TaskStamp;

mod overlay;

const DEFAULT_DEPTH: u8 = 1;
const DEFAULT_NODES: usize = 48;
const NODE_STEP: usize = 32;
const MAX_PINNED_ROOTS: usize = WorkRelationshipOptions::MAX_ROOTS - 1;
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

/// Durable browser-owned map controls. Task stamps intentionally outlive any
/// one board projection so a reconnect cannot silently change the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::browser) struct WorkMapState {
    mode: WorkDetailMode,
    pinned: Vec<TaskStamp>,
    direction: WorkRelationshipDirection,
    depth: u8,
    max_nodes: usize,
    origins: bool,
    zoom: u16,
    pan_x: i16,
    pan_y: i16,
}

impl Default for WorkMapState {
    fn default() -> Self {
        Self {
            mode: WorkDetailMode::Details,
            pinned: Vec::new(),
            direction: WorkRelationshipDirection::Both,
            depth: DEFAULT_DEPTH,
            max_nodes: DEFAULT_NODES,
            origins: true,
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

    pub(super) fn pinned(&self, task: &TaskStamp) -> bool {
        self.pinned.contains(task)
    }

    pub(super) fn pin_limit_reached(&self) -> bool {
        self.pinned.len() >= MAX_PINNED_ROOTS
    }

    fn roots(&self, selected: &TaskStamp) -> Vec<TaskStamp> {
        let mut roots = self.pinned.clone();
        roots.push(selected.clone());
        roots.sort();
        roots.dedup();
        roots
    }

    fn options(&self) -> WorkRelationshipOptions {
        let options = WorkRelationshipOptions::new(self.direction, self.depth, self.max_nodes)
            .expect("Work map controls stay inside relationship limits");
        if self.origins {
            options
        } else {
            options.without_provenance()
        }
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
        self.origins = true;
        self.zoom = ZOOM_MIN;
        self.pan_x = 0;
        self.pan_y = 0;
    }

    fn expand(&mut self) {
        self.depth = self
            .depth
            .saturating_add(1)
            .min(WorkRelationshipOptions::MAX_DEPTH);
        self.max_nodes = self
            .max_nodes
            .saturating_add(NODE_STEP)
            .min(WorkRelationshipOptions::MAX_NODES);
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
    pub work: &'a WorkProjection<'work>,
    pub selected: &'a TaskStamp,
    pub state: &'a UseStateHandle<WorkMapState>,
    pub select_task: Callback<TaskStamp>,
}

pub(super) fn toggle_root_callback(state: UseStateHandle<WorkMapState>) -> Callback<TaskStamp> {
    Callback::from(move |task| update(&state, |next| next.toggle_pin(task)))
}

pub(super) fn render_tabs(state: &UseStateHandle<WorkMapState>, selected: &TaskStamp) -> Html {
    let details = set_mode_callback(state.clone(), WorkDetailMode::Details);
    let map = set_mode_callback(state.clone(), WorkDetailMode::Map);
    html! {
        <div class="work-detail-tabs" role="tablist" aria-label="Task view">
            <button
                type="button"
                role="tab"
                aria-selected={(state.mode() == WorkDetailMode::Details).to_string()}
                class={(state.mode() == WorkDetailMode::Details).then_some("active")}
                onclick={details}
            >{"Details"}</button>
            <button
                type="button"
                role="tab"
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
    let roots = render.state.roots(render.selected);
    let graph = match render.work.relationships(&roots, render.state.options()) {
        Ok(graph) => graph,
        Err(error) => {
            return html! {
                <div class="work-map-problem" role="alert">
                    <h3>{"Relationship map unavailable"}</h3>
                    <p>{error.to_string()}</p>
                </div>
            };
        }
    };
    let source = AttrValue::from(mermaid_source(render.work, &graph));
    let targets = overlay::node_targets(render.work, &graph, render.selected);
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
    let related = graph
        .nodes()
        .iter()
        .filter(|node| matches!(node, WorkRelationshipNode::Task(task) if !task.selected()))
        .count();
    html! {
        <div class="work-map-content" data-work-map-roots={roots.len().to_string()}>
            {render_controls(render.state, render.selected, &graph)}
            if related == 0 {
                <p class="work-map-empty" role="status">
                    {"No related tasks are visible at the current depth and direction."}
                </p>
            }
            <section class="work-map-diagram" aria-labelledby="work-map-diagram-title">
                <div class="work-map-heading">
                    <div>
                        <p class="work-detail-kicker">{"Bounded neighborhood"}</p>
                        <h3 id="work-map-diagram-title">{"Dependency map"}</h3>
                    </div>
                    <span>{format!("{} nodes · {} edges", graph.nodes().len(), graph.edges().len())}</span>
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
            {render_task_list(render.work, &graph, render.selected, &render.select_task)}
            {render_frontiers(render.work, &graph, render.state)}
        </div>
    }
}

fn set_mode_callback(
    state: UseStateHandle<WorkMapState>,
    mode: WorkDetailMode,
) -> Callback<MouseEvent> {
    Callback::from(move |_| update(&state, |next| next.mode = mode))
}

fn render_controls(
    state: &UseStateHandle<WorkMapState>,
    selected: &TaskStamp,
    graph: &WorkRelationshipGraph,
) -> Html {
    let center = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::center))
    };
    let reset = {
        let state = state.clone();
        Callback::from(move |_| update(&state, WorkMapState::reset_controls))
    };
    let less_depth = depth_callback(state.clone(), false);
    let more_depth = depth_callback(state.clone(), true);
    let origins = {
        let state = state.clone();
        Callback::from(move |_| update(&state, |next| next.origins = !next.origins))
    };
    html! {
        <section class="work-map-controls" aria-label="Map controls">
            <div class="work-map-control-group" aria-label="Relationship direction">
                {direction_button(state, WorkRelationshipDirection::Prerequisites, "Prerequisites")}
                {direction_button(state, WorkRelationshipDirection::Both, "Both")}
                {direction_button(state, WorkRelationshipDirection::Dependents, "Dependents")}
            </div>
            <div class="work-map-control-group work-map-depth" aria-label="Relationship depth">
                <button type="button" aria-label="Decrease map depth" disabled={state.depth == 0} onclick={less_depth}>{"−"}</button>
                <span>{format!("Depth {}", state.depth)}</span>
                <button type="button" aria-label="Increase map depth" disabled={state.depth >= WorkRelationshipOptions::MAX_DEPTH} onclick={more_depth}>{"+"}</button>
            </div>
            {render_viewport_controls(state)}
            <button type="button" aria-pressed={state.origins.to_string()} onclick={origins}>
                {if state.origins { "Origins shown" } else { "Origins hidden" }}
            </button>
            <button type="button" disabled={state.pinned.is_empty()} onclick={center}>
                {format!("Center on {}", task_label_from_stamp(selected))}
            </button>
            <button type="button" disabled={is_default_view(state)} onclick={reset}>{"Reset view"}</button>
            <span class="work-map-bounds">
                {format!("{} / {} nodes", graph.nodes().len(), state.max_nodes)}
            </span>
        </section>
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
                next.depth
                    .saturating_add(1)
                    .min(WorkRelationshipOptions::MAX_DEPTH)
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
            <div class="work-map-control-group work-map-zoom" aria-label="Map zoom">
                <button
                    type="button"
                    aria-label="Zoom map out"
                    disabled={state.zoom <= ZOOM_MIN}
                    onclick={viewport_callback(state.clone(), ViewportAction::Zoom(false))}
                >{"−"}</button>
                <span>{format!("{}%", state.zoom)}</span>
                <button
                    type="button"
                    aria-label="Zoom map in"
                    disabled={state.zoom >= ZOOM_MAX}
                    onclick={viewport_callback(state.clone(), ViewportAction::Zoom(true))}
                >{"+"}</button>
            </div>
            <div class="work-map-control-group work-map-pan" aria-label="Map pan">
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
        && state.origins
        && state.zoom == ZOOM_MIN
        && state.pan_x == 0
        && state.pan_y == 0
}

fn render_task_list(
    work: &WorkProjection<'_>,
    graph: &WorkRelationshipGraph,
    selected: &TaskStamp,
    select: &Callback<TaskStamp>,
) -> Html {
    let tasks = graph.nodes().iter().filter_map(|node| match node {
        WorkRelationshipNode::Task(task) => Some(task),
        WorkRelationshipNode::Origin(_) => None,
    });
    html! {
        <section class="work-map-list" aria-labelledby="work-map-list-title">
            <div class="work-map-heading">
                <h3 id="work-map-list-title">{"Tasks in this map"}</h3>
                <span>{"Keyboard and pointer navigation"}</span>
            </div>
            <div class="work-map-task-list">
                {for tasks.map(|task| {
                    let born = task.born().clone();
                    let activate = {
                        let select = select.clone();
                        let born = born.clone();
                        Callback::from(move |_| select.emit(born.clone()))
                    };
                    let (handle, title) = task.task().map_or_else(
                        || (format!("Task {}", task.born()), "Unavailable in this snapshot".to_owned()),
                        |index| {
                            let row = work.tasks()[index].row();
                            (row.statement.body.handle.to_string(), row.presentation_title().to_owned())
                        },
                    );
                    let relation = if task.selected() {
                        "Selected root".to_owned()
                    } else {
                        format!("{} {} away", task.depth(), if task.depth() == 1 { "hop" } else { "hops" })
                    };
                    html! {
                        <button
                            key={task.born().to_string()}
                            type="button"
                            class="work-map-task"
                            data-task-stamp={task.born().to_string()}
                            aria-current={(task.born() == selected).then_some("true")}
                            onclick={activate}
                        >
                            <span>{relation}</span>
                            <strong>{handle}</strong>
                            <small>{title}</small>
                        </button>
                    }
                })}
            </div>
        </section>
    }
}

fn render_frontiers(
    work: &WorkProjection<'_>,
    graph: &WorkRelationshipGraph,
    state: &UseStateHandle<WorkMapState>,
) -> Html {
    if graph.frontier().is_empty() {
        return Html::default();
    }
    let cannot_expand = state.depth >= WorkRelationshipOptions::MAX_DEPTH
        && state.max_nodes >= WorkRelationshipOptions::MAX_NODES;
    html! {
        <section class="work-map-frontiers" aria-labelledby="work-map-frontiers-title">
            <div class="work-map-heading">
                <h3 id="work-map-frontiers-title">{"Hidden relationships"}</h3>
                <span>{"Nothing is silently omitted"}</span>
            </div>
            <div class="work-map-frontier-list">
                {for graph.frontier().iter().map(|frontier| {
                    let label = node_label(work, &graph.nodes()[frontier.node()]);
                    let expansion = expansion_label(frontier.expansion());
                    let expand = {
                        let state = state.clone();
                        Callback::from(move |_| update(&state, WorkMapState::expand))
                    };
                    html! {
                        <button type="button" disabled={cannot_expand} onclick={expand}>
                            {format!("Show {} more {expansion} from {label}", frontier.omitted())}
                        </button>
                    }
                })}
            </div>
        </section>
    }
}

fn mermaid_source(work: &WorkProjection<'_>, graph: &WorkRelationshipGraph) -> String {
    let mut source = String::from("flowchart LR\n");
    for (index, node) in graph.nodes().iter().enumerate() {
        let label = node_label(work, node);
        writeln!(source, "  n{index}[\"{}\"]", mermaid_escape(&label))
            .expect("String writes are infallible");
    }
    for edge in graph.edges() {
        let source_index = edge.source();
        let target_index = edge.target();
        match edge.kind() {
            WorkRelationshipKind::Dependency => {
                let label = if edge.satisfied() == Some(true) {
                    "done"
                } else {
                    "requires"
                };
                writeln!(source, "  n{source_index} -->|{label}| n{target_index}")
                    .expect("String writes are infallible");
            }
            WorkRelationshipKind::Provenance => {
                writeln!(source, "  n{source_index} -.->|origin| n{target_index}")
                    .expect("String writes are infallible");
            }
        }
    }
    let selected = graph
        .nodes()
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match node {
            WorkRelationshipNode::Task(task) if task.selected() => Some(format!("n{index}")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !selected.is_empty() {
        writeln!(source, "  class {} selected", selected.join(","))
            .expect("String writes are infallible");
        source.push_str("  classDef selected stroke-width:3px\n");
    }
    source
}

fn node_label(work: &WorkProjection<'_>, node: &WorkRelationshipNode) -> String {
    match node {
        WorkRelationshipNode::Task(task) => task.task().map_or_else(
            || format!("Task {} · unavailable", task.born()),
            |index| {
                let projected = &work.tasks()[index];
                let row = projected.row();
                format!(
                    "{} · {} · {}",
                    row.statement.body.handle,
                    compact_label(row.presentation_title()),
                    availability_label(projected.availability())
                )
            },
        ),
        WorkRelationshipNode::Origin(origin) => {
            format!("Origin · {}", compact_label(origin.origin().as_str()))
        }
    }
}

fn compact_label(label: &str) -> String {
    const LIMIT: usize = 54;
    let mut compact = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > LIMIT {
        compact = compact.chars().take(LIMIT - 1).collect::<String>();
        compact.push('…');
    }
    compact
}

fn mermaid_escape(label: &str) -> String {
    label
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const fn availability_label(availability: WorkAvailability) -> &'static str {
    match availability {
        WorkAvailability::InFlight => "In flight",
        WorkAvailability::Ready => "Ready",
        WorkAvailability::Blocked => "Blocked",
        WorkAvailability::Deferred => "Deferred",
        WorkAvailability::Terminal => "Complete",
    }
}

const fn expansion_label(expansion: WorkRelationshipExpansion) -> &'static str {
    match expansion {
        WorkRelationshipExpansion::Prerequisites => "prerequisites",
        WorkRelationshipExpansion::Dependents => "dependents",
        WorkRelationshipExpansion::Provenance => "origins",
    }
}

fn task_label_from_stamp(task: &TaskStamp) -> String {
    format!("task {task}")
}

fn update(state: &UseStateHandle<WorkMapState>, change: impl FnOnce(&mut WorkMapState)) {
    let mut next = (**state).clone();
    change(&mut next);
    state.set(next);
}

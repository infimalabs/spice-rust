//! Immediate whole-set task graph for the contextual overview.

use std::fmt::Write as _;
use std::rc::Rc;

use wire::TaskStamp;
use yew::prelude::*;

use crate::browser::mermaid::MermaidImage;
use crate::work::{
    TaskOverviewContext, TaskOverviewGraph, TaskOverviewNode, TaskOverviewNodeKind,
    TaskOverviewSelection, WorkAvailability, WorkProjection,
};

mod overlay;

const ZOOM_MIN: u16 = 100;
const ZOOM_MAX: u16 = 200;
const ZOOM_STEP: u16 = 25;
const PAN_STEP: i16 = 48;

/// Browser-local viewport and board-root choices. Roots become one explicit
/// [`TaskOverviewSelection`] before the overview opens; they are never a second
/// hidden selection inside the overview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::browser) struct WorkMapState {
    pinned: Vec<TaskStamp>,
    zoom: u16,
    pan_x: i16,
    pan_y: i16,
}

impl Default for WorkMapState {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
            zoom: ZOOM_MIN,
            pan_x: 0,
            pan_y: 0,
        }
    }
}

impl WorkMapState {
    pub(in crate::browser) fn pinned(&self, task: &TaskStamp) -> bool {
        self.pinned.contains(task)
    }

    /// The former protocol-root cap no longer governs overview selection.
    pub(in crate::browser) const fn pin_limit_reached(&self) -> bool {
        false
    }

    pub(in crate::browser) fn selection(&self, task: TaskStamp) -> TaskOverviewSelection {
        let mut tasks = self.pinned.clone();
        tasks.push(task.clone());
        let context = if tasks.len() == 1 {
            TaskOverviewContext::Task
        } else {
            TaskOverviewContext::Relationships(task)
        };
        TaskOverviewSelection::new(tasks, context)
    }

    fn toggle_pin(&mut self, task: TaskStamp) {
        if let Some(index) = self.pinned.iter().position(|pinned| pinned == &task) {
            self.pinned.remove(index);
        } else {
            self.pinned.push(task);
        }
    }

    fn reset_viewport(&mut self) {
        self.zoom = ZOOM_MIN;
        self.pan_x = 0;
        self.pan_y = 0;
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
    pub work: &'a WorkProjection<'work>,
}

pub(super) fn toggle_root_callback(state: UseStateHandle<WorkMapState>) -> Callback<TaskStamp> {
    Callback::from(move |task| update(&state, |next| next.toggle_pin(task)))
}

pub(super) fn render(render: WorkMapRender<'_, '_>) -> Html {
    let graph = render.work.overview_graph(render.selection);
    let controls = render_controls(render.state);
    let presentation = present(&graph, render.work);
    let targets = Rc::new(
        presentation
            .nodes
            .iter()
            .filter_map(|node| {
                (node.task_index.is_some() && node.selected).then_some(())?;
                Some(overlay::MapNodeTarget {
                    id: node.id.clone(),
                    task: node.task.clone(),
                    label: node.label.clone(),
                    selected: node.selected,
                    availability: node.tone.map(|tone| tone.availability),
                    disposition: node.tone.and_then(|tone| tone.disposition),
                    state: node.tone.map(|tone| tone.label),
                })
            })
            .collect::<Vec<_>>(),
    );
    let node_ids = Rc::new(targets.iter().map(|target| target.id.clone()).collect());
    let viewport = overlay::ViewportTransform {
        zoom: render.state.zoom,
        pan_x: render.state.pan_x,
        pan_y: render.state.pan_y,
    };
    let render_geometry = Some(overlay::renderer(
        targets,
        render.focus_task.clone(),
        viewport,
    ));

    html! {
        <section
            class="work-map-content work-overview-map"
            aria-labelledby="work-map-diagram-title"
            data-work-map-roots={render.selection.tasks().len().to_string()}
            data-work-map-context={render.selection.context().label()}
        >
            <div class="work-map-heading">
                <div>
                    <p class="work-detail-kicker">{"Admitted board snapshot"}</p>
                    <h3 id="work-map-diagram-title">{"Task graph"}</h3>
                </div>
                <span>{format!(
                    "{} selected · {} visible nodes · {} dependencies",
                    render.selection.tasks().len(),
                    graph.nodes().len(),
                    graph.edges().len(),
                )}</span>
            </div>
            {controls}
            if render.selection.is_empty() {
                <div class="work-map-empty" role="status">
                    <strong>{"No tasks selected"}</strong>
                    <span>{"This valid context has no current tasks."}</span>
                </div>
            } else {
                <div
                    class="work-map-viewport"
                    data-work-map-zoom={render.state.zoom.to_string()}
                    data-work-map-pan-x={render.state.pan_x.to_string()}
                    data-work-map-pan-y={render.state.pan_y.to_string()}
                >
                    <MermaidImage
                        source={AttrValue::from(presentation.source.clone())}
                        {node_ids}
                        {render_geometry}
                        final_answer={true}
                    />
                </div>
                {render_node_list(&presentation, &render.focus_task)}
            }
        </section>
    }
}

struct Presentation {
    nodes: Vec<PresentedNode>,
    source: String,
}

struct PresentedNode {
    id: String,
    task: TaskStamp,
    task_index: Option<usize>,
    label: String,
    detail: String,
    selected: bool,
    kind: TaskOverviewNodeKind,
    tone: Option<TaskTone>,
}

fn present(graph: &TaskOverviewGraph, work: &WorkProjection<'_>) -> Presentation {
    let nodes = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| present_node(index, node, work))
        .collect::<Vec<_>>();
    let mut source = String::from("flowchart LR\n");
    for node in &nodes {
        let diagram = crate::mermaid_label::multiline(&format!("{}\n{}", node.label, node.detail));
        writeln!(source, "  {}[\"{diagram}\"]", node.id).expect("String writes are infallible");
    }
    for edge in graph.edges() {
        let connector = if edge.satisfied() { "-.->" } else { "-->" };
        writeln!(
            source,
            "  n{} {connector}|requires| n{}",
            edge.prerequisite(),
            edge.dependent(),
        )
        .expect("String writes are infallible");
    }
    let selected = nodes
        .iter()
        .filter(|node| node.selected)
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    if !selected.is_empty() {
        writeln!(source, "  class {} selected", selected.join(","))
            .expect("String writes are infallible");
        source.push_str("  classDef selected stroke-width:3px\n");
    }
    let missing = nodes
        .iter()
        .filter(|node| node.kind == TaskOverviewNodeKind::Missing)
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        writeln!(source, "  class {} missing", missing.join(","))
            .expect("String writes are infallible");
        source.push_str("  classDef missing stroke-dasharray:5 4\n");
    }
    Presentation { nodes, source }
}

fn present_node(index: usize, node: &TaskOverviewNode, work: &WorkProjection<'_>) -> PresentedNode {
    let row = node.task_index().map(|index| work.tasks()[index].row());
    let label = row.map_or_else(
        || node.task().to_string(),
        |row| {
            format!(
                "{} · {}",
                row.statement.body.handle,
                compact(&row.presentation_title),
            )
        },
    );
    let detail = match (node.kind(), row) {
        (TaskOverviewNodeKind::Selected, Some(row)) => format!(
            "Selected · {} · {}",
            row.workflow.body.phase, row.workflow.body.status,
        ),
        (TaskOverviewNodeKind::External, Some(row)) => format!(
            "External dependency · {} · {}",
            row.workflow.body.phase, row.workflow.body.status,
        ),
        (TaskOverviewNodeKind::Missing, _) => "Missing from the admitted board snapshot".to_owned(),
        (_, None) => "Unavailable in the admitted board snapshot".to_owned(),
    };
    PresentedNode {
        id: format!("n{index}"),
        task: node.task().clone(),
        task_index: node.task_index(),
        label,
        detail,
        selected: node.selected(),
        kind: node.kind(),
        tone: node
            .task_index()
            .map(|index| task_tone(&work.tasks()[index])),
    }
}

fn render_node_list(presentation: &Presentation, focus: &Callback<TaskStamp>) -> Html {
    html! {
        <section class="work-map-list" aria-labelledby="work-map-list-title">
            <div class="work-map-heading">
                <h3 id="work-map-list-title">{"Tasks in this graph"}</h3>
                <span>{"Selected roots first; outside neighbors are explicit"}</span>
            </div>
            <div class="work-map-task-list">
                {for presentation.nodes.iter().map(|node| {
                    let kind = match node.kind {
                        TaskOverviewNodeKind::Selected => "Selected task",
                        TaskOverviewNodeKind::External => "External dependency",
                        TaskOverviewNodeKind::Missing => "Missing dependency",
                    };
                    if node.task_index.is_some() && node.selected {
                        let activate = {
                            let focus = focus.clone();
                            let task = node.task.clone();
                            Callback::from(move |_| focus.emit(task.clone()))
                        };
                        html! {
                            <button
                                key={node.id.clone()}
                                type="button"
                                class="work-map-task"
                                data-task-stamp={node.task.to_string()}
                                data-task-selected={node.selected.to_string()}
                                data-work-availability={node.tone.map(|tone| tone.availability)}
                                data-work-disposition={node.tone.and_then(|tone| tone.disposition)}
                                data-work-state={node.tone.map(|tone| tone.label)}
                                onclick={activate}
                            >
                                <span>{kind}</span>
                                <strong>{node.label.clone()}</strong>
                                <small>{node.detail.clone()}</small>
                            </button>
                        }
                    } else {
                        html! {
                            <article key={node.id.clone()} class="work-map-fact" data-task-neighbor={kind}>
                                <span>{kind}</span>
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

#[derive(Clone, Copy)]
struct TaskTone {
    availability: &'static str,
    disposition: Option<&'static str>,
    label: &'static str,
}

fn task_tone(task: &crate::work::WorkTask<'_>) -> TaskTone {
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
    TaskTone {
        availability: availability.key(),
        disposition,
        label,
    }
}

fn compact(value: &str) -> String {
    const LIMIT: usize = 48;
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > LIMIT {
        value = value.chars().take(LIMIT - 1).collect();
        value.push('…');
    }
    value
}

#[derive(Clone, Copy)]
enum ViewportAction {
    Zoom(bool),
    Pan(i16, i16),
    Reset,
}

fn render_controls(state: &UseStateHandle<WorkMapState>) -> Html {
    html! {
        <section class="work-map-controls" aria-label="Map controls">
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
                <button type="button" disabled={is_default_view(state)} onclick={viewport_callback(state.clone(), ViewportAction::Reset)}>{"Reset view"}</button>
            </div>
        </section>
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
            ViewportAction::Reset => next.reset_viewport(),
        });
    })
}

fn is_default_view(state: &WorkMapState) -> bool {
    state.zoom == ZOOM_MIN && state.pan_x == 0 && state.pan_y == 0
}

fn update(state: &UseStateHandle<WorkMapState>, change: impl FnOnce(&mut WorkMapState)) {
    let mut next = (**state).clone();
    change(&mut next);
    state.set(next);
}

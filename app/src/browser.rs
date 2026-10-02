use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use futures_channel::mpsc;
use futures_util::{FutureExt, SinkExt, StreamExt, select};
use gloo_net::http::Request;
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, WebSocketError};
use gloo_timers::future::TimeoutFuture;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use wire::body::DirectiveAttachment;
use wire::intent::{
    BoardRead, GlobalSettingsRead, GlobalSettingsSet, LaneAction, LaneEffortRetune, LaneRead,
    LaneRenewal, LaneRetune, LaneState, SteerHistory, SteerInbox, SteerSend, TaskAnswer,
    TeamArrangement, TeamConfigure, TeamLife, VendorCatalog,
};
use wire::{
    CanonicalUuid, CommandKind, Response, Seq, StreamRequest, TaskHandle, TaskStamp, WireVersion,
    Witness,
};
use yew::prelude::*;

use crate::browser_branding::{ConnectionState, CountState, use_browser_branding};
use crate::client_action::ClientAction;
use crate::compose::{self, Notice};
use crate::composer_submissions::{Submission, SubmissionQueue, SubmissionStatus};
use crate::runtime::{CanvasSurface, topology_mutation_requires_refresh};
use crate::speech_queue::{NarrationMessage, SpeechQueue, SpeechRequest};
use crate::view::{CanvasRoute, RouteEvidence};
use crate::work::{WorkArrangement, WorkProjection, WorkScope};
use crate::{
    Axis, CanvasDestination, CanvasRuntime, Principal, Session, SteerPane, StreamDirective,
    TaskDetailState, TeamDestination, stale_reads,
};

mod archive_status;
mod canvas_menu;
mod card_target;
mod clipboard;
mod clock;
mod connection;
mod diagnostics;
mod direction_card;
mod direction_history;
mod image_viewer;
mod mermaid;
mod message_task_inspector;
mod navigation;
mod overlay;
mod sender;
mod speech_runtime;
mod task_detail;
mod task_feedback;
mod task_relations;
mod team_settled;
mod toast;
mod view;
mod watchband;
mod work_card;
mod work_history;

use crate::PendingKind;
use crate::action_mailbox::{ActionLease, ActionQueue};
use connection::run_surface;
use sender::{
    SenderHandle, action_identity, admit_action, queue_submission, queue_submissions, random_uuid,
};
use speech_runtime::{
    AgentSpeechControls, NarrationBatch, SpeechRuntime, SpeechStatus, load_narration_cursors,
    use_agent_speech_controls, use_page_hide_speech_stop, use_speech_failure_toast,
    use_team_speech_abandonment,
};
use view::{CanvasRender, render_canvas};
use watchband::ComposerAction;
use watchband::{ComposerDrafts, TeamControlAction, TeamControls};

/// Source identity injected by installed Spice `dev xtask -- app` and verified before publish.
const APP_SOURCE_IDENTITY: &str = match option_env!("SPICE_APP_SOURCE_IDENTITY") {
    // env-policy: allow
    Some(identity) => identity,
    None => "unbound-development-build",
};
const CONNECTION_HANDSHAKE_TIMEOUT_MILLIS: u32 = 5_000;

/// How long the socket may stay silent before the resident is probed.
///
/// A quiet board sends nothing, so silence alone says nothing about the socket.
/// The probe is what makes the next interval meaningful: after it, silence is a
/// resident that did not answer rather than a resident with nothing to say.
const LIVENESS_PROBE_MILLIS: u32 = 15_000;

type SocketWriter = futures_util::stream::SplitSink<WebSocket, Message>;
type SocketReader = futures_util::stream::SplitStream<WebSocket>;

struct EstablishedConnection {
    session: Session,
    writer: SocketWriter,
    reader: SocketReader,
    principal: Principal,
    hydrating: bool,
}

#[derive(Clone)]
struct SubmissionHandle {
    queue: Rc<RefCell<SubmissionQueue>>,
    status: UseStateHandle<SubmissionStatus>,
}

impl SubmissionHandle {
    fn publish(&self) {
        let status = self.queue.borrow().status();
        // This handle is retained by the connection effect from its first
        // render. Its dereferenced value is therefore that render's snapshot,
        // not a live comparison point; comparing here suppresses transitions
        // back to the initial empty state. Publish the queue fold
        // unconditionally so every stage transition reaches the current tree.
        self.status.set(status);
    }
}
/// The durable submission one task's observation composer is waiting on.
#[derive(Clone, Default, PartialEq, Eq)]
enum NoteSubmission {
    #[default]
    Idle,
    Pending {
        text: String,
        after: Option<Seq>,
    },
    Reconciling {
        text: String,
        after: Option<Seq>,
    },
    /// The composed text was sent as this task's closing validation.
    Closing,
    Failed(String),
}

impl NoteSubmission {
    const fn busy(&self) -> bool {
        matches!(
            self,
            Self::Pending { .. } | Self::Reconciling { .. } | Self::Closing
        )
    }
}

/// One task's observation composer: the draft it retains, the local refusal
/// of that draft, and the submission it waits on. Every surface showing the
/// task shows this composer, and no other task's.
#[derive(Clone, Default, PartialEq, Eq)]
struct NoteComposer {
    draft: String,
    notice: Notice,
    submission: NoteSubmission,
}

/// Observation composers by task. The connection keeps this handle from its
/// first render, so the shared cell is the live value and `published` is the
/// snapshot each render reads.
#[derive(Clone)]
struct NoteComposers {
    state: Rc<RefCell<BTreeMap<TaskStamp, NoteComposer>>>,
    published: UseStateHandle<BTreeMap<TaskStamp, NoteComposer>>,
}

impl NoteComposers {
    fn update(&self, task: &TaskStamp, change: impl FnOnce(&mut NoteComposer)) {
        let mut state = self.state.borrow_mut();
        let composer = state.entry(task.clone()).or_default();
        change(composer);
        if *composer == NoteComposer::default() {
            state.remove(task);
        }
        self.published.set(state.clone());
    }

    fn submission(&self, task: &TaskStamp) -> NoteSubmission {
        self.state
            .borrow()
            .get(task)
            .map(|composer| composer.submission.clone())
            .unwrap_or_default()
    }

    fn shown(&self, task: &TaskStamp) -> NoteComposer {
        self.published.get(task).cloned().unwrap_or_default()
    }
}

#[derive(Clone)]
struct SurfaceHandles {
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    identity: UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: UseStateHandle<Option<CanvasUpdate>>,
    team_controls: UseReducerHandle<TeamControls>,
    composer_drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
    notes: NoteComposers,
    /// Refusals the resident reports for actions no control is waiting on.
    refusals: toast::Toaster,
}

#[derive(Clone, PartialEq, Eq)]
struct CanvasUpdate {
    version: wire::CanvasVersion,
    generation: wire::Digest,
    loaded_generation: wire::Digest,
}

#[derive(Clone, PartialEq, Eq)]
struct CanvasIdentity {
    version: wire::Version,
    loaded_app: wire::Digest,
}

#[derive(Clone, PartialEq)]
enum Connection {
    Starting,
    Connecting,
    Online { epoch: String },
    Offline(String),
}

impl Connection {
    /// The socket is admitted and the published runtime is the one the
    /// resident serves now. Only this state lends the runtime's offers to a
    /// composer: through an outage and through the hydration of a restarted
    /// resident the last runtime stays on screen, and a draft submitted
    /// against its remembered occupant and epoch would only be refused.
    const fn is_online(&self) -> bool {
        matches!(self, Self::Online { .. })
    }

    const fn stage(&self) -> crate::view::ConnectionStage {
        match self {
            Self::Starting => crate::view::ConnectionStage::Starting,
            Self::Connecting => crate::view::ConnectionStage::Connecting,
            Self::Online { .. } => crate::view::ConnectionStage::Online,
            Self::Offline(_) => crate::view::ConnectionStage::Offline,
        }
    }

    fn indicator_state(&self) -> &'static str {
        match self {
            Self::Online { .. } => "online",
            Self::Offline(_) => "reconnecting",
            Self::Starting => "starting",
            Self::Connecting => "connecting",
        }
    }

    const fn branding_state(&self) -> ConnectionState {
        match self {
            Self::Starting => ConnectionState::Starting,
            Self::Connecting => ConnectionState::Connecting,
            Self::Online { .. } => ConnectionState::Online,
            Self::Offline(_) => ConnectionState::Offline,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Starting => "Loading spice".to_owned(),
            Self::Connecting => "Opening current resident".to_owned(),
            Self::Online { epoch } => format!("Connected to resident {epoch}"),
            Self::Offline(reason) => format!("Reconnecting / {reason}"),
        }
    }
}

type SurfaceData = CanvasSurface;

#[derive(Deserialize)]
struct ResponseIdentity {
    id: CanonicalUuid,
}

#[hook]
fn use_composer_drag(surface: &SurfaceData) -> watchband::PrimaryComposerDragState {
    let context = {
        let surface = surface.read();
        surface
            .as_ref()
            .and_then(|runtime| {
                runtime.lanes().map(|lanes| {
                    (
                        runtime.offers(CommandKind::TeamArrange),
                        Some(lanes.topology_revision),
                    )
                })
            })
            .unwrap_or((false, None))
    };
    watchband::use_primary_composer_drag_state(context.0, context.1)
}

#[hook]
fn use_team_drag(surface: &SurfaceData) -> watchband::TeamDragState {
    let context = {
        let surface = surface.read();
        surface
            .as_ref()
            .and_then(|runtime| {
                runtime.lanes().map(|lanes| {
                    (
                        runtime.offers(CommandKind::TeamArrange),
                        Some(lanes.topology_revision),
                    )
                })
            })
            .unwrap_or((false, None))
    };
    watchband::use_team_drag_state(context.0, context.1)
}

#[function_component(CanvasRouter)]
fn canvas_router() -> Html {
    let (route, notice, navigate) = navigation::use_route();
    html! {
        <toast::ToastProvider>
            <RouteNotice {notice} />
            <CanvasApp {route} {navigate} />
        </toast::ToastProvider>
    }
}

#[derive(Properties, PartialEq)]
struct RouteNoticeProperties {
    notice: Option<navigation::RouteNotice>,
}

#[function_component(RouteNotice)]
fn route_notice(properties: &RouteNoticeProperties) -> Html {
    let toaster = toast::use_toaster();
    use_effect_with(properties.notice.clone(), move |notice| {
        if let Some(notice) = notice {
            toaster.settled(notice.text.clone());
        }
    });
    Html::default()
}

#[derive(Properties, PartialEq)]
struct CanvasAppProperties {
    route: CanvasRoute,
    navigate: Callback<navigation::Navigation>,
}

struct CanvasAppState {
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    canvas_menu_open: UseStateHandle<bool>,
    destination: CanvasDestination,
    route: CanvasRoute,
    route_navigate: Callback<navigation::Navigation>,
    card_target: Option<String>,
    card_target_status: UseStateHandle<Option<card_target::Status>>,
    navigate: Callback<navigation::Intent>,
    work_scope: WorkScope,
    work_arrangement: UseStateHandle<WorkArrangement>,
    identity: UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: UseStateHandle<Option<CanvasUpdate>>,
    team_controls: UseReducerHandle<TeamControls>,
    team_fleet: NodeRef,
    composer_drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
    sender: SenderHandle,
    composer_drag: watchband::PrimaryComposerDragState,
    team_drag: watchband::TeamDragState,
    work_detail: task_detail::Controls,
    task_inspector: message_task_inspector::Controls,
    direction_history: direction_history::Controls,
    work_history: work_history::Controls,
    team_settled: team_settled::Controls,
    work_map: UseStateHandle<view::WorkMapState>,
    speech_status: UseStateHandle<SpeechStatus>,
    agents: view::AgentControls,
    unsafe_intent: bool,
}

impl CanvasAppState {
    fn render(self) -> Html {
        let _trace = diagnostics::span("render.canvas", || diagnostics::Metadata {
            cards: self
                .surface
                .read()
                .as_ref()
                .map_or(0, |runtime| runtime.messages().len())
                .into(),
            ..diagnostics::Metadata::default()
        });
        let surface_view = self.surface.read();
        let work = project_work(surface_view.as_ref());
        let retry_relations = task_relations::retry_callback(
            self.surface.clone(),
            self.work_detail.selected.clone(),
            self.work_map.clone(),
        );
        let card_target_notice = (*self.card_target_status)
            .as_ref()
            .filter(|status| {
                status.available && self.card_target.as_deref() == Some(status.target.as_str())
            })
            .map(|_| "Linked card focused.".to_owned());
        let canvas = render_canvas(
            CanvasRender {
                connection: &self.connection,
                surface: &self.surface,
                destination: self.destination,
                card_target_notice,
                navigate: &self.navigate,
                canvas_menu_open: &self.canvas_menu_open,
                work_scope: &self.work_scope,
                work_arrangement: &self.work_arrangement,
                identity: &self.identity,
                canvas_update: &self.canvas_update,
                team_controls: &self.team_controls,
                composer_drafts: &self.composer_drafts,
                submissions: &self.submissions,
                composer_drag: &self.composer_drag,
                team_drag: &self.team_drag,
                team_fleet: &self.team_fleet,
                unsafe_intent: self.unsafe_intent,
                sender: &self.sender,
                agents: &self.agents,
                selected_task: &self.work_detail.selected,
                detail_presented: *self.work_detail.presented,
                detail_closing: *self.work_detail.closing,
                direction_history: &self.direction_history,
                work_map: &self.work_map,
                work_history: &self.work_history,
                team_settled: &self.team_settled,
                detail_close: &self.work_detail.close_ref,
                detail_sheet: &self.work_detail.sheet_ref,
                notes: &self.work_detail.notes,
                submit_note: self.work_detail.submit_note.clone(),
                select_task: self.navigate.reform(navigation::Intent::Task),
                request_task: self.work_detail.request.clone(),
                close_task: self.work_detail.close.clone(),
                retry_relations,
            },
            work.as_ref(),
        );
        message_task_inspector::provide(self.task_inspector, canvas, self.composer_drafts)
    }
}

#[hook]
fn use_canvas_app_state(properties: &CanvasAppProperties) -> (CanvasAppState, SpeechRuntime) {
    let route = properties.route.clone();
    let connection = use_state(|| Connection::Starting);
    let surface = use_state(SurfaceData::default);
    let canvas_menu_open = use_state(|| false);
    let card_target_status = use_state(|| None::<card_target::Status>);
    let local_task = use_local_task(&route);
    let work_arrangement = use_state(WorkArrangement::default);
    let identity = use_state(|| None::<CanvasIdentity>);
    let canvas_update = use_state(|| None::<CanvasUpdate>);
    let team_controls = use_reducer(TeamControls::restore);
    let team_fleet = use_node_ref();
    let composer_drafts = use_reducer(ComposerDrafts::default);
    watchband::use_composer_issue_toast(&composer_drafts);
    let submission_status = use_state(SubmissionStatus::default);
    let submissions = SubmissionHandle {
        queue: use_mut_ref(SubmissionQueue::default),
        status: submission_status.clone(),
    };
    let sender = SenderHandle {
        queue: use_mut_ref(|| None::<ActionQueue>),
        refusals: toast::use_toaster(),
    };
    let composer_drag = use_composer_drag(&surface);
    let team_drag = use_team_drag(&surface);
    let navigate = navigation::intent_callback(
        route.clone(),
        properties.navigate.clone(),
        local_task.clone(),
        surface.clone(),
    );
    let work_detail = task_detail::use_controls(
        sender.clone(),
        surface.clone(),
        work_detail_mounted(&surface),
        task_detail::Request {
            task: route.task().cloned().or_else(|| (*local_task).clone()),
            leaving: route.card_target().is_some(),
        },
        navigate.reform(|()| navigation::Intent::CloseTask),
    );
    let task_inspector = message_task_inspector::use_controls(&surface, &work_detail, &navigate);
    let direction_history = direction_history::use_controls(
        sender.clone(),
        surface.clone(),
        route.canvas_destination() == CanvasDestination::Work,
        route.directions().flatten().cloned(),
        properties.navigate.clone(),
    );
    let work_history = work_history::use_controls(sender.clone(), surface.clone());
    let team_settled = team_settled::use_controls(
        sender.clone(),
        surface.clone(),
        watchband::work_shown_teams(
            &surface,
            &team_controls,
            route.canvas_destination() == CanvasDestination::Chat,
        ),
    );
    let work_map = use_state(view::WorkMapState::default);
    let speech_status = use_state(SpeechStatus::default);
    let speech_message = use_state(|| None::<String>);
    let speech_runtime = SpeechRuntime {
        queue: use_mut_ref(SpeechQueue::default),
        controller: use_mut_ref(crate::speech::Controller::default),
        status: speech_status.clone(),
        message: speech_message.clone(),
    };
    let speech_ref = use_mut_ref(|| speech_runtime.clone());
    *speech_ref.borrow_mut() = speech_runtime.clone();
    let narration_cursors = use_mut_ref(load_narration_cursors);
    let agent_speech = use_agent_speech_controls(speech_ref, narration_cursors, speech_message);
    let agents = use_agent_controls(sender.clone(), surface.clone(), agent_speech);
    let unsafe_intent = use_unsafe_intent(&composer_drafts, &submission_status);
    (
        CanvasAppState {
            connection,
            surface,
            canvas_menu_open,
            destination: route.canvas_destination(),
            card_target: route.card_target().map(str::to_owned),
            work_scope: route.work_scope().cloned().unwrap_or_default(),
            route,
            route_navigate: properties.navigate.clone(),
            card_target_status,
            navigate,
            work_arrangement,
            identity,
            canvas_update,
            team_controls,
            team_fleet,
            composer_drafts,
            submissions,
            sender,
            composer_drag,
            team_drag,
            work_detail,
            task_inspector,
            direction_history,
            work_history,
            team_settled,
            work_map,
            speech_status,
            agents,
            unsafe_intent,
        },
        speech_runtime,
    )
}

#[hook]
fn use_canvas_app_effects(state: &CanvasAppState, speech_runtime: SpeechRuntime) {
    let remaining = state
        .surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::board)
        .map(|board| board.active_public_tasks().count());
    use_browser_branding(CountState::resolve(
        state.connection.branding_state(),
        remaining,
    ));
    use_surface_connection(
        state.sender.clone(),
        SurfaceHandles {
            connection: state.connection.clone(),
            surface: state.surface.clone(),
            identity: state.identity.clone(),
            canvas_update: state.canvas_update.clone(),
            team_controls: state.team_controls.clone(),
            composer_drafts: state.composer_drafts.clone(),
            submissions: state.submissions.clone(),
            notes: state.work_detail.notes.clone(),
            refusals: state.sender.refusals.clone(),
        },
    );
    task_feedback::use_feedback(state);
    task_relations::use_request(
        &state.sender,
        &state.surface,
        &state.work_detail.selected,
        &state.work_map,
    );
    use_team_speech_abandonment(&state.surface, speech_runtime.clone());
    use_page_hide_speech_stop(speech_runtime);
    use_speech_failure_toast(&state.speech_status);
    let chat_visible = state.destination == CanvasDestination::Chat;
    card_target::use_target(
        state.card_target.clone(),
        &state.surface,
        &state.sender,
        state.card_target_status.clone(),
        state.team_controls.clone(),
        chat_visible,
    );
    card_target::use_direction_target(
        state.route.direction_target().cloned(),
        state.team_controls.clone(),
        chat_visible,
    );
    card_target::use_global_direction_target(
        global_direction_card(&state.route, &state.surface),
        state.team_controls.clone(),
        state.destination == CanvasDestination::Work,
    );
    use_mosaic_root_scroll(chat_visible);
    canvas_menu::use_dismissal(state.canvas_menu_open.clone());
    message_task_inspector::use_surface_lifetime(&state.task_inspector, chat_visible);
    watchband::use_team_menu_escape(&state.team_controls, chat_visible);
    watchband::use_command_modifier_preview();
    watchband::use_team_fleet_right_edge(
        state.team_fleet.clone(),
        teamed_lane_count(&state.surface),
    );
    let surface_view = state.surface.read();
    let work = project_work(surface_view.as_ref());
    use_route_settlement(
        work.as_ref(),
        &state.route,
        &state.surface,
        state.route_navigate.clone(),
    );
}

#[function_component(CanvasApp)]
fn canvas_app(properties: &CanvasAppProperties) -> Html {
    let (state, speech_runtime) = use_canvas_app_state(properties);
    use_canvas_app_effects(&state, speech_runtime);
    state.render()
}

/// The detail layer renders over every destination once a board is available.
fn work_detail_mounted(surface: &SurfaceData) -> bool {
    surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::board)
        .is_some()
}

fn project_work(runtime: Option<&CanvasRuntime>) -> Option<WorkProjection<'_>> {
    runtime
        .and_then(CanvasRuntime::board)
        .map(WorkProjection::from_board)
}

fn teamed_lane_count(surface: &SurfaceData) -> usize {
    surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map_or(0, |lanes| {
            lanes.lanes.iter().filter(|team| team.teamed()).count()
        })
}

/// The element a Directions route brings into view: its direction's card
/// once the anchored page holds it, otherwise the Directions section.
fn global_direction_card(route: &CanvasRoute, surface: &SurfaceData) -> Option<String> {
    match route.directions()? {
        Some(crate::view::DirectionsFocus::Direction(target)) => {
            let surface = surface.read();
            let group = surface.as_ref()?.direction_history().group(target)?;
            Some(crate::view::global_direction_card_id(group.fanout))
        }
        Some(crate::view::DirectionsFocus::Team(_)) | None => {
            Some(crate::view::DIRECTIONS_SECTION_ID.to_owned())
        }
    }
}

/// A task opened over Dash or Chat: browser-local, so any route change ends it.
#[hook]
fn use_local_task(route: &CanvasRoute) -> UseStateHandle<Option<TaskStamp>> {
    let local_task = use_state(|| None::<TaskStamp>);
    {
        let local_task = local_task.clone();
        use_effect_with(route.clone(), move |_| {
            if local_task.is_some() {
                local_task.set(None);
            }
        });
    }
    local_task
}

/// Validate the route's data-backed segments once authoritative state
/// settles them, replacing a refused route with its containing view.
#[hook]
fn use_route_settlement(
    work: Option<&WorkProjection<'_>>,
    route: &CanvasRoute,
    surface: &SurfaceData,
    navigate: Callback<navigation::Navigation>,
) {
    let card = card_target::use_retention(route.card_target(), surface);
    let evidence = RouteEvidence {
        scope: route
            .work_scope()
            .zip(work)
            .map(|(scope, work)| work.contains_scope(scope)),
        task: route.task().and_then(|task| {
            surface
                .read()
                .as_ref()?
                .task_detail(task)
                .and_then(TaskDetailState::known)
        }),
        card,
        direction: route.directions().flatten().and_then(|focus| {
            let surface = surface.read();
            let runtime = surface.as_ref()?;
            match focus {
                crate::view::DirectionsFocus::Team(team) => runtime
                    .lanes()
                    .map(|topology| topology.lanes.iter().any(|row| row.lane == *team)),
                crate::view::DirectionsFocus::Direction(target) => {
                    runtime.direction_history().locate(target)
                }
            }
        }),
    };
    use_effect_with((route.clone(), evidence), move |(route, evidence)| {
        if let Some((next, notice)) = route.unavailable(*evidence) {
            navigate.emit(navigation::Navigation::Refuse(next, notice));
        }
    });
}

#[hook]
fn use_surface_connection(sender: SenderHandle, handles: SurfaceHandles) {
    use_effect_with((), move |()| {
        let active = Rc::new(Cell::new(true));
        let (queue, wakes) = ActionQueue::new();
        *sender.queue.borrow_mut() = Some(queue.clone());
        spawn_local(run_surface(active.clone(), handles, queue, wakes));
        move || {
            active.set(false);
            sender.queue.borrow_mut().take();
        }
    });
}

#[hook]
fn use_unsafe_intent(
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &UseStateHandle<SubmissionStatus>,
) -> bool {
    let unsafe_intent =
        ComposerDrafts::is_unsafe(drafts) || SubmissionStatus::is_unsafe(submissions);
    use_unsafe_intent_guard(unsafe_intent);
    unsafe_intent
}

#[hook]
fn use_unsafe_intent_guard(unsafe_intent: bool) {
    use_effect_with(unsafe_intent, move |unsafe_intent| {
        let unsafe_intent = *unsafe_intent;
        let registration = web_sys::window().and_then(|window| {
            let callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    let has_native_draft = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|document| {
                            document
                                .query_selector_all(
                                    ".team-composer textarea, .work-note-composer textarea",
                                )
                                .ok()
                        })
                        .is_some_and(|textareas| {
                            (0..textareas.length()).any(|index| {
                                textareas
                                    .get(index)
                                    .and_then(|node| {
                                        node.dyn_into::<web_sys::HtmlTextAreaElement>().ok()
                                    })
                                    .is_some_and(|textarea| !textarea.value().is_empty())
                            })
                        });
                    if unsafe_intent || has_native_draft {
                        event.prevent_default();
                        let _ = js_sys::Reflect::set(
                            event.as_ref(),
                            &JsValue::from_str("returnValue"),
                            &JsValue::from_str(""),
                        );
                    }
                });
            window
                .add_event_listener_with_callback("beforeunload", callback.as_ref().unchecked_ref())
                .ok()?;
            Some((window, callback))
        });
        move || {
            if let Some((window, callback)) = registration {
                let _ = window.remove_event_listener_with_callback(
                    "beforeunload",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

fn clamp_fixed_mosaic_root(window: &web_sys::Window) {
    let Some(document) = window.document() else {
        return;
    };
    let Ok(Some(shell)) = document.query_selector(".shell--mosaic") else {
        return;
    };
    let fixed = window
        .get_computed_style(&shell)
        .ok()
        .flatten()
        .is_some_and(|style| {
            style
                .get_property_value("position")
                .is_ok_and(|value| value == "fixed")
        });
    if fixed && window.scroll_y().unwrap_or_default() != 0.0 {
        window.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

#[hook]
fn use_mosaic_root_scroll(mosaic: bool) {
    use_effect_with(mosaic, move |mosaic| {
        let registration = (*mosaic)
            .then(|| {
                let window = web_sys::window()?;
                clamp_fixed_mosaic_root(&window);
                let callback_window = window.clone();
                let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                    clamp_fixed_mosaic_root(&callback_window);
                });
                window
                    .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref())
                    .ok()?;
                if window
                    .add_event_listener_with_callback("scroll", callback.as_ref().unchecked_ref())
                    .is_err()
                {
                    let _ = window.remove_event_listener_with_callback(
                        "resize",
                        callback.as_ref().unchecked_ref(),
                    );
                    return None;
                }
                Some((window, callback))
            })
            .flatten();
        move || {
            if let Some((window, callback)) = registration {
                for event in ["resize", "scroll"] {
                    let _ = window.remove_event_listener_with_callback(
                        event,
                        callback.as_ref().unchecked_ref(),
                    );
                }
            }
        }
    });
}

#[hook]
fn use_agent_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    speech: AgentSpeechControls,
) -> view::AgentControls {
    let seat = view::SeatDraft {
        life: use_state(|| wire::intent::TeamLife::Steer.as_str().to_owned()),
        effort: use_state(String::new),
        notice: use_state(Notice::none),
    };
    view::AgentControls {
        submit_seat: seat_submit(sender, surface, seat.clone()),
        seat,
        speak: speech.speak,
        narrate: speech.narrate,
        suppress_automatic_speech: speech.suppress_automatic,
        speech_message: speech.message,
    }
}

fn seat_submit(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    draft: view::SeatDraft,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let value = |id: &str| {
            document
                .get_element_by_id(id)
                .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|input| input.value())
                .unwrap_or_default()
        };
        let name = value("seat-name");
        let worktree = value("seat-worktree");
        let model = value("seat-model");
        let (Some(topology_revision), Some(vendor)) =
            surface.read().as_ref().map_or((None, None), |runtime| {
                (
                    runtime.lanes().map(|lanes| lanes.topology_revision),
                    runtime
                        .vendor_catalog()
                        .and_then(compose::seat_vendor)
                        .map(|vendor| vendor.name.clone()),
                )
            })
        else {
            return;
        };
        match compose::seat(&name, &worktree, vendor, &draft.life) {
            Ok(seat) => {
                let Ok(lane) = random_uuid() else {
                    return;
                };
                let selection = compose::selection(&model, &draft.effort);
                if admit_action(
                    &sender,
                    ClientAction::Seat {
                        topology_revision,
                        lane,
                        name: seat.name,
                        worktree: seat.worktree,
                        life: seat.life,
                        vendor: seat.vendor,
                        model: selection.model,
                        effort: selection.effort,
                    },
                ) {
                    draft.notice.set(Notice::none());
                }
            }
            Err(refusal) => draft.notice.set(refusal.into()),
        }
    })
}

fn apply_team_configuration(
    session: &Session,
    team_controls: &UseReducerHandle<TeamControls>,
    runtime: &mut CanvasRuntime,
    team: CanonicalUuid,
    life: Option<TeamLife>,
    bytes: &[u8],
) -> Result<(bool, bool), String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let refresh = topology_mutation_requires_refresh(&admitted.get().outcome);
    let applied = runtime.apply_lanes(&admitted);
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) if applied => {
            team_controls.dispatch(TeamControlAction::SyncLanes {
                answer: reply.body.clone(),
                settle: life.map(|life| (team, life.into())),
            });
        }
        wire::Outcome::Reply(_) => {
            if let Some(life) = life {
                team_controls.dispatch(TeamControlAction::SettleStaleLife {
                    team,
                    life: life.into(),
                });
            }
        }
        wire::Outcome::Refusal(refusal) => {
            team_controls.dispatch(TeamControlAction::SettleIssue {
                team,
                life: life.map(Into::into),
                issue: format!("{}: {}", refusal.reason(), refusal.explanation()),
            });
        }
        wire::Outcome::Fault(fault) => {
            team_controls.dispatch(TeamControlAction::SettleIssue {
                team,
                life: life.map(Into::into),
                issue: format!("{}: {}", fault.reason, fault.detail),
            });
        }
    }
    Ok((refresh, applied))
}

fn admit_response<T: for<'de> Deserialize<'de>>(
    session: &Session,
    bytes: &[u8],
) -> Result<crate::Admitted<Response<T>>, String> {
    session
        .admit_frame(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))
}

fn websocket_error(error: WebSocketError) -> String {
    format!("socket failed: {error}")
}

fn binary_message(message: Message) -> Result<Vec<u8>, String> {
    match message {
        Message::Bytes(bytes) => Ok(bytes),
        Message::Text(_) => Err("resident sent a text frame".to_owned()),
    }
}

fn empty(message: &'static str) -> Html {
    html! { <div class="empty">{message}</div> }
}

fn phase_class(status: &str) -> Classes {
    if matches!(status, "completed" | "deleted") {
        classes!("tag")
    } else {
        classes!("tag", "active")
    }
}

fn state_from_select(state: UseStateHandle<String>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
        state.set(select.value());
    })
}

#[wasm_bindgen(start)]
pub fn start() {
    if connection::capture_opening_auth_token().expect("capture Canvas opening auth token") {
        navigation::replace_location(&crate::view::CanvasRoute::destination(
            CanvasDestination::Chat,
        ));
    }
    diagnostics::install();
    let root = web_sys::window()
        .expect("browser window")
        .document()
        .expect("browser document")
        .get_element_by_id("app")
        .expect("canvas app root");
    root.set_attribute("data-app-source", APP_SOURCE_IDENTITY)
        .expect("stamp canvas app source identity");
    yew::Renderer::<CanvasRouter>::with_root(root).render();
}

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
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
    BoardRead, GlobalSettingsRead, GlobalSettingsSet, LaneAction, LaneRead, LaneRenewal,
    LaneRetune, LaneState, SteerInbox, SteerSend, TaskAnswer, TeamArrangement, TeamConfigure,
    TeamLife, VendorCatalog,
};
use wire::{
    CanonicalUuid, CommandKind, Response, Seq, StreamRequest, TaskHandle, TaskStamp, WireVersion,
    Witness,
};
use yew::prelude::*;

use crate::client_action::ClientAction;
use crate::compose::{self, Notice};
use crate::composer_submissions::{Submission, SubmissionQueue, SubmissionStatus};
use crate::runtime::{CanvasSurface, topology_mutation_requires_refresh};
use crate::speech::{SystemObservers, SystemStart};
use crate::speech_queue::{
    Advance, Enqueue, ExternalPause, NarrationCursors, NarrationMessage, SpeechEntry, SpeechQueue,
    SpeechRequest, SuppressAutomatic,
};
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
mod image_viewer;
mod mermaid;
mod message_task_inspector;
mod navigation;
mod overlay;
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
use crate::action_mailbox::{self, ActionLease, ActionQueue};
use connection::{request_external_speech, run_surface};
use view::{CanvasRender, render_canvas};
use watchband::ComposerAction;
use watchband::{ComposerDrafts, TeamControlAction, TeamControls, TeamSpeech};

/// Source identity injected by installed Spice `dev xtask -- app` and verified before publish.
const APP_SOURCE_IDENTITY: &str = match option_env!("SPICE_APP_SOURCE_IDENTITY") {
    // env-policy: allow
    Some(identity) => identity,
    None => "unbound-development-build",
};
const CONNECTION_HANDSHAKE_TIMEOUT_MILLIS: u32 = 5_000;
const NARRATION_CURSOR_STORAGE: &str = "spice:narration-cursors:v1";

/// How long the socket may stay silent before the resident is probed.
///
/// A quiet board sends nothing, so silence alone says nothing about the socket.
/// The probe is what makes the next interval meaningful: after it, silence is a
/// resident that did not answer rather than a resident with nothing to say.
const LIVENESS_PROBE_MILLIS: u32 = 15_000;

/// How long a system request waits for the browser to publish its voices.
///
/// Engines enumerate voices once per page and announce the list with
/// `voiceschanged`; the wait covers that first-load window. An engine with no
/// voices never announces anything, so the wait needs an end.
const VOICE_PUBLICATION_DEADLINE_MILLIS: u32 = 5_000;

#[derive(Clone)]
struct SenderHandle {
    queue: Rc<RefCell<Option<ActionQueue>>>,
    issue: UseStateHandle<Option<String>>,
}
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
    Failed(String),
}

impl NoteSubmission {
    const fn busy(&self) -> bool {
        matches!(self, Self::Pending { .. } | Self::Reconciling { .. })
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

#[derive(Clone, Default, PartialEq)]
enum SpeechStatus {
    #[default]
    Idle,
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct NarrationBatch {
    team: String,
    materialization: bool,
    mode: TeamSpeech,
    messages: Vec<NarrationMessage>,
}

#[derive(Clone)]
struct AgentSpeechControls {
    speak: Callback<SpeechRequest>,
    narrate: Callback<NarrationBatch>,
    suppress_automatic: Callback<()>,
    message: UseStateHandle<Option<String>>,
}

#[derive(Clone)]
struct SpeechRuntime {
    queue: Rc<RefCell<SpeechQueue>>,
    controller: Rc<RefCell<crate::speech::Controller>>,
    status: UseStateHandle<SpeechStatus>,
    message: UseStateHandle<Option<String>>,
}

impl SpeechRuntime {
    fn enqueue(&self, request: SpeechRequest) {
        let decision = self.queue.borrow_mut().enqueue(request);
        if let Enqueue::Start(entry) = decision {
            self.start(entry);
        }
    }

    fn toggle_message(&self, request: SpeechRequest) {
        let was_playing = self.queue.borrow().active_message_key() == request.message_key();
        self.stop();
        if !was_playing {
            self.enqueue(request);
        }
    }

    fn start(&self, entry: SpeechEntry) {
        let generation = self.controller.borrow_mut().begin();
        self.message.set(entry.message_key().map(str::to_owned));
        self.status.set(SpeechStatus::Idle);
        match entry.playback().clone() {
            wire::SpeechPlayback::Disabled {} => self.finish(entry.id(), None),
            wire::SpeechPlayback::System {
                voice,
                words_per_minute,
            } => self.start_system(entry, generation, &voice, words_per_minute),
            wire::SpeechPlayback::External {
                content_type,
                available,
                ..
            } => {
                if available {
                    self.start_external(entry, generation, content_type);
                } else {
                    self.finish(
                        entry.id(),
                        Some("external speech backend is unavailable".to_owned()),
                    );
                }
            }
        }
    }

    fn start_system(
        &self,
        entry: SpeechEntry,
        generation: u64,
        voice: &str,
        words_per_minute: u32,
    ) {
        let ended = self.clone();
        let failed = self.clone();
        let awaiting = self.clone();
        let id = entry.id();
        let resumed = entry.clone();
        let resumed_voice = voice.to_owned();
        let result = self.controller.borrow_mut().start_system(
            generation,
            entry.text(),
            voice,
            words_per_minute,
            SystemObservers {
                voices: move || {
                    awaiting.resume_system(
                        resumed.clone(),
                        generation,
                        resumed_voice.clone(),
                        words_per_minute,
                    );
                },
                ended: move || ended.defer_finish(id, None),
                failed: move |issue| {
                    failed.defer_finish(id, Some(issue));
                },
            },
        );
        match result {
            Ok(SystemStart::Speaking) => {}
            // The browser has published no voices yet. This entry stays the
            // loading queue head until `voiceschanged` resumes it, so the
            // first utterance of a page load speaks instead of refusing a
            // voice the browser had not enumerated.
            Ok(SystemStart::AwaitingVoices) => self.refuse_unpublished_voices(id, generation),
            Err(error) => self.finish(id, Some(error)),
        }
    }

    /// Retry system playback once the browser publishes its voice list.
    ///
    /// The resume runs one task after the event: the observing closure is
    /// owned by the controller, and re-entering [`Self::start_system`]
    /// directly would drop that closure while it is still running. Stopping
    /// or superseding the request in the meantime both detaches the observer
    /// and fails this ownership check, so nothing stale ever starts.
    fn resume_system(
        &self,
        entry: SpeechEntry,
        generation: u64,
        voice: String,
        words_per_minute: u32,
    ) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(0).await;
            if !runtime.is_current(entry.id(), generation) {
                return;
            }
            runtime.start_system(entry, generation, &voice, words_per_minute);
        });
    }

    /// Refuse a request whose browser never publishes any voice.
    ///
    /// Waiting on `voiceschanged` is what lets a configured voice arrive
    /// late, but an engine carrying no voices at all never fires it. Without
    /// this deadline such a request would hold the queue head silently until
    /// a global stop, which reads as speech that vanished rather than speech
    /// this browser cannot give.
    fn refuse_unpublished_voices(&self, id: u64, generation: u64) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(VOICE_PUBLICATION_DEADLINE_MILLIS).await;
            let awaiting = runtime.controller.borrow().is_awaiting_voices(generation);
            if !awaiting || !runtime.is_current(id, generation) {
                return;
            }
            runtime.finish(
                id,
                Some("this browser published no system voices".to_owned()),
            );
        });
    }

    fn start_external(&self, entry: SpeechEntry, generation: u64, content_type: String) {
        let runtime = self.clone();
        spawn_local(async move {
            let rendered =
                request_external_speech(entry.worktree(), entry.text(), &content_type).await;
            if !runtime.is_current(entry.id(), generation) {
                return;
            }
            let bytes = match rendered {
                Ok(bytes) => bytes,
                Err(error) => {
                    runtime.finish(entry.id(), Some(error));
                    return;
                }
            };
            let ended = runtime.clone();
            let failed = runtime.clone();
            let paused = runtime.clone();
            let id = entry.id();
            let playing = runtime.controller.borrow_mut().start_external(
                generation,
                &bytes,
                &content_type,
                move || ended.defer_finish(id, None),
                move || {
                    failed.defer_finish(id, Some("rendered speech playback failed".to_owned()));
                },
                move || paused.external_pause(id),
            );
            let playing = match playing {
                Ok(playing) => playing,
                Err(error) => {
                    runtime.finish(id, Some(error));
                    return;
                }
            };
            if let Err(error) = crate::speech::await_play(playing).await
                && runtime.is_current(id, generation)
            {
                runtime.finish(id, Some(error));
            }
        });
    }

    fn defer_finish(&self, id: u64, failure: Option<String>) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(0).await;
            runtime.finish(id, failure);
        });
    }

    fn finish(&self, id: u64, failure: Option<String>) {
        let advance = self.queue.borrow_mut().finish(id);
        match advance {
            Advance::Ignored => {}
            Advance::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status
                    .set(failure.map_or(SpeechStatus::Idle, SpeechStatus::Failed));
            }
            Advance::Start(entry) => self.start(entry),
        }
    }

    fn is_current(&self, id: u64, generation: u64) -> bool {
        self.queue.borrow().is_active(id) && self.controller.borrow().is_current(generation)
    }

    fn external_pause(&self, id: u64) {
        let pause = self.queue.borrow_mut().external_pause(id);
        match pause {
            ExternalPause::Ignored => {}
            ExternalPause::Stopped | ExternalPause::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status.set(SpeechStatus::Idle);
            }
            ExternalPause::Start(entry) => self.start(entry),
        }
    }

    fn stop(&self) {
        self.queue.borrow_mut().stop();
        self.controller.borrow_mut().stop();
        self.message.set(None);
        self.status.set(SpeechStatus::Idle);
    }

    fn suppress_automatic(&self) {
        match self.queue.borrow_mut().suppress_automatic() {
            SuppressAutomatic::Preserved => {}
            SuppressAutomatic::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status.set(SpeechStatus::Idle);
            }
            SuppressAutomatic::Start(entry) => self.start(entry),
        }
    }

    fn abandon_lane(&self, lane: &str) {
        self.queue.borrow_mut().abandon_lane(lane);
    }
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

    fn label(&self) -> String {
        match self {
            Self::Starting => "Loading embedded canvas".to_owned(),
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

#[function_component(CanvasRouter)]
fn canvas_router() -> Html {
    let (route, navigate) = navigation::use_destination();
    if let Some(route) = route {
        let destination = route.canvas_destination();
        let card_target = route.card_target().map(str::to_owned);
        html! {
            <toast::ToastProvider>
                <CanvasApp {destination} {card_target} {navigate} />
            </toast::ToastProvider>
        }
    } else {
        html! {
            <main class="shell">
                <section class="empty" role="alert" data-route-unavailable="true">
                    <h1>{"Canvas route unavailable"}</h1>
                    <p>{"Use #/ (Chat), #/work (Work), #/dash (Dash), or a card permalink."}</p>
                </section>
            </main>
        }
    }
}

#[derive(Properties, PartialEq)]
struct CanvasAppProperties {
    destination: CanvasDestination,
    card_target: Option<String>,
    navigate: Callback<CanvasDestination>,
}

struct CanvasAppState {
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    canvas_menu_open: UseStateHandle<bool>,
    destination: CanvasDestination,
    card_target: Option<String>,
    card_target_status: UseStateHandle<Option<card_target::Status>>,
    navigate: Callback<navigation::Intent>,
    work_scope: UseStateHandle<WorkScope>,
    work_scope_notice: UseStateHandle<Option<String>>,
    work_arrangement: UseStateHandle<WorkArrangement>,
    identity: UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: UseStateHandle<Option<CanvasUpdate>>,
    team_controls: UseReducerHandle<TeamControls>,
    team_fleet: NodeRef,
    composer_drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
    action_issue: UseStateHandle<Option<String>>,
    sender: SenderHandle,
    composer_drag: watchband::PrimaryComposerDragState,
    work_detail: task_detail::Controls,
    task_inspector: message_task_inspector::Controls,
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
            .filter(|status| self.card_target.as_deref() == Some(status.target.as_str()))
            .map(|status| {
                if status.available {
                    "Linked card focused.".to_owned()
                } else {
                    "Linked card is unavailable in retained message history.".to_owned()
                }
            });
        let canvas = render_canvas(
            CanvasRender {
                connection: &self.connection,
                surface: &self.surface,
                destination: self.destination,
                card_target_notice,
                navigate: &self.navigate,
                canvas_menu_open: &self.canvas_menu_open,
                work_scope: &self.work_scope,
                work_scope_notice: &self.work_scope_notice,
                work_arrangement: &self.work_arrangement,
                identity: &self.identity,
                canvas_update: &self.canvas_update,
                team_controls: &self.team_controls,
                composer_drafts: &self.composer_drafts,
                submissions: &self.submissions,
                composer_drag: &self.composer_drag,
                team_fleet: &self.team_fleet,
                unsafe_intent: self.unsafe_intent,
                sender: &self.sender,
                agents: &self.agents,
                selected_task: &self.work_detail.selected,
                detail_presented: *self.work_detail.presented,
                detail_closing: *self.work_detail.closing,
                work_map: &self.work_map,
                work_history: &self.work_history,
                team_settled: &self.team_settled,
                detail_close: &self.work_detail.close_ref,
                detail_sheet: &self.work_detail.sheet_ref,
                notes: &self.work_detail.notes,
                submit_note: self.work_detail.submit_note.clone(),
                select_task: self.work_detail.select.clone(),
                select_dash_task: self.work_detail.select_history.clone(),
                close_task: self.work_detail.close.clone(),
                retry_relations,
            },
            work.as_ref(),
        );
        message_task_inspector::provide(self.task_inspector, canvas)
    }
}

#[hook]
fn use_canvas_app_state(properties: &CanvasAppProperties) -> (CanvasAppState, SpeechRuntime) {
    let connection = use_state(|| Connection::Starting);
    let surface = use_state(SurfaceData::default);
    let canvas_menu_open = use_state(|| false);
    let card_target_status = use_state(|| None::<card_target::Status>);
    let work_scope = use_state(WorkScope::default);
    let work_scope_notice = use_state(|| None::<String>);
    let work_arrangement = use_state(WorkArrangement::default);
    let identity = use_state(|| None::<CanvasIdentity>);
    let canvas_update = use_state(|| None::<CanvasUpdate>);
    let team_controls = use_reducer(TeamControls::restore);
    let team_fleet = use_node_ref();
    let composer_drafts = use_reducer(ComposerDrafts::default);
    let submission_status = use_state(SubmissionStatus::default);
    let submissions = SubmissionHandle {
        queue: use_mut_ref(SubmissionQueue::default),
        status: submission_status.clone(),
    };
    let action_issue = use_state(|| None::<String>);
    let sender = SenderHandle {
        queue: use_mut_ref(|| None::<ActionQueue>),
        issue: action_issue.clone(),
    };
    let composer_drag = use_composer_drag(&surface);
    let work_detail = task_detail::use_controls(
        sender.clone(),
        surface.clone(),
        work_detail_mounted(&surface),
    );
    let navigate = navigation::intent_callback(
        properties.destination,
        properties.navigate.clone(),
        work_scope.clone(),
        work_scope_notice.clone(),
        work_detail.select.clone(),
        surface.clone(),
    );
    let task_inspector = message_task_inspector::use_controls(&surface, &work_detail, &navigate);
    let work_history = work_history::use_controls(sender.clone(), surface.clone());
    let team_settled = team_settled::use_controls(
        sender.clone(),
        surface.clone(),
        watchband::work_shown_teams(
            &surface,
            &team_controls,
            properties.destination == CanvasDestination::Chat,
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
            destination: properties.destination,
            card_target: properties.card_target.clone(),
            card_target_status,
            navigate,
            work_scope,
            work_scope_notice,
            work_arrangement,
            identity,
            canvas_update,
            team_controls,
            team_fleet,
            composer_drafts,
            submissions,
            action_issue,
            sender,
            composer_drag,
            work_detail,
            task_inspector,
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
    use_transient_result_toast(&state.action_issue, &state.speech_status);
    let chat_visible = state.destination == CanvasDestination::Chat;
    card_target::use_target(
        state.card_target.clone(),
        &state.surface,
        state.card_target_status.clone(),
        state.team_controls.clone(),
        chat_visible,
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
    use_work_scope_reconciliation(
        work.as_ref(),
        state.work_scope.clone(),
        state.work_scope_notice.clone(),
    );
}

#[hook]
fn use_transient_result_toast(
    action_issue: &UseStateHandle<Option<String>>,
    speech_status: &UseStateHandle<SpeechStatus>,
) {
    let toaster = toast::use_toaster();
    use_effect_with((**action_issue).clone(), {
        let toaster = toaster.clone();
        move |issue| {
            if let Some(issue) = issue {
                toaster.settled(issue.clone());
            }
        }
    });
    use_effect_with((**speech_status).clone(), move |status| {
        if let SpeechStatus::Failed(issue) = status {
            toaster.settled(issue.clone());
        }
    });
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

#[hook]
fn use_work_scope_reconciliation(
    work: Option<&WorkProjection<'_>>,
    scope: UseStateHandle<WorkScope>,
    notice: UseStateHandle<Option<String>>,
) {
    let available = work.map(|projection| projection.contains_scope(&scope));
    use_effect_with(
        ((*scope).clone(), available),
        move |(requested, available)| {
            if matches!(requested, WorkScope::Stem(_)) && *available == Some(false) {
                let missing = requested
                    .stem()
                    .expect("a selected scope has a stem")
                    .to_string();
                scope.set(WorkScope::AllOpen);
                notice.set(Some(format!(
                    "{missing} is no longer available; showing All open work."
                )));
            }
        },
    );
}

#[hook]
fn use_agent_speech_controls(
    runtime: Rc<RefCell<SpeechRuntime>>,
    narration_cursors: Rc<RefCell<NarrationCursors>>,
    message: UseStateHandle<Option<String>>,
) -> AgentSpeechControls {
    let enqueue = {
        let runtime = runtime.clone();
        use_memo((), move |_| {
            Callback::from(move |request: SpeechRequest| runtime.borrow().enqueue(request))
        })
    };
    let speak = {
        let runtime = runtime.clone();
        use_memo((), move |_| {
            Callback::from(move |request: SpeechRequest| runtime.borrow().toggle_message(request))
        })
    };
    let suppress_automatic = use_memo((), move |_| {
        Callback::from(move |()| runtime.borrow().suppress_automatic())
    });
    let narrate = {
        let enqueue = (*enqueue).clone();
        use_memo((), move |_| narration_callback(narration_cursors, enqueue))
    };
    AgentSpeechControls {
        speak: (*speak).clone(),
        narrate: (*narrate).clone(),
        suppress_automatic: (*suppress_automatic).clone(),
        message,
    }
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

/// Stop every clip the moment the page is hidden for navigation or close.
///
/// System synthesis runs in the operating system speech server, so an
/// utterance queued by this page keeps playing after the window that queued
/// it is gone unless the page cancels it on the way out. `pagehide` is the
/// last event every browser dispatches before a document is unloaded or
/// frozen, including tab and window close.
#[hook]
fn use_page_hide_speech_stop(speech_runtime: SpeechRuntime) {
    use_effect_with((), move |()| {
        let registration = web_sys::window().and_then(|window| {
            let callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    speech_runtime.stop()
                });
            window
                .add_event_listener_with_callback("pagehide", callback.as_ref().unchecked_ref())
                .ok()
                .map(|()| (window, callback))
        });
        move || {
            if let Some((window, callback)) = registration {
                let _ = window.remove_event_listener_with_callback(
                    "pagehide",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

#[hook]
fn use_team_speech_abandonment(
    surface: &UseStateHandle<SurfaceData>,
    speech_runtime: SpeechRuntime,
) {
    let teams = surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| {
            lanes
                .lanes
                .iter()
                .map(|team| team.lane.to_string())
                .collect::<BTreeSet<_>>()
        });
    let previous = use_mut_ref(|| None::<BTreeSet<String>>);
    use_effect_with(teams, move |teams| {
        let Some(current) = teams else {
            return;
        };
        if let Some(previous) = previous.borrow().as_ref() {
            for removed in previous.difference(current) {
                speech_runtime.abandon_lane(removed);
            }
        }
        *previous.borrow_mut() = Some(current.clone());
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

fn load_narration_cursors() -> NarrationCursors {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(NARRATION_CURSOR_STORAGE).ok().flatten())
        .and_then(|value| serde_json::from_str::<Vec<(String, String)>>(&value).ok())
        .map_or_else(NarrationCursors::default, NarrationCursors::restore)
}

fn store_narration_cursors(cursors: &NarrationCursors) {
    let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    else {
        return;
    };
    let Ok(value) = serde_json::to_string(&cursors.rows()) else {
        return;
    };
    let _ = storage.set_item(NARRATION_CURSOR_STORAGE, &value);
}

fn narration_callback(
    cursors: Rc<RefCell<NarrationCursors>>,
    speak: Callback<SpeechRequest>,
) -> Callback<NarrationBatch> {
    Callback::from(move |batch: NarrationBatch| {
        let lines = {
            let mut cursors = cursors.borrow_mut();
            let lines = cursors.observe(batch.materialization, &batch.messages);
            store_narration_cursors(&cursors);
            lines
        };
        for line in lines {
            let text = match batch.mode {
                TeamSpeech::Quiet => None,
                TeamSpeech::Speak => line.automatic_speech,
                TeamSpeech::Narrate => line.narration,
            };
            if let Some(text) = text {
                speak.emit(SpeechRequest::narration(
                    line.worktree,
                    line.playback,
                    batch.team.clone(),
                    line.key,
                    text,
                ));
            }
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

fn random_uuid() -> Result<CanonicalUuid, String> {
    let mut bytes = [0_u8; 16];
    web_sys::window()
        .ok_or_else(|| "browser window is unavailable".to_owned())?
        .crypto()
        .map_err(|_| "browser crypto is unavailable".to_owned())?
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|_| "browser random source failed".to_owned())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        concat!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-",
            "{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        ),
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
    .parse()
    .map_err(|error| format!("browser command identity was invalid: {error}"))
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

fn admit_action(sender: &SenderHandle, action: ClientAction) -> bool {
    let clears_issue = action.is_mutation();
    let topology_id = if action.is_topology_mutation() {
        let Some(id) = action_identity(sender) else {
            return false;
        };
        Some(id)
    } else {
        None
    };
    let result = sender
        .queue
        .borrow()
        .as_ref()
        .ok_or(action_mailbox::ActionAdmissionError::Unavailable)
        .and_then(|queue| queue.admit(action, topology_id));
    match result {
        Ok(()) => {
            if clears_issue && sender.issue.is_some() {
                sender.issue.set(None);
            }
            true
        }
        Err(error) => {
            sender.issue.set(Some(error.to_string()));
            false
        }
    }
}

fn action_identity(sender: &SenderHandle) -> Option<CanonicalUuid> {
    match random_uuid() {
        Ok(id) => Some(id),
        Err(error) => {
            sender.issue.set(Some(error));
            None
        }
    }
}

fn queue_submission(
    sender: &SenderHandle,
    submissions: &SubmissionHandle,
    submission: Submission,
) -> bool {
    let id = submission.id;
    if let Err(error) = submissions.queue.borrow_mut().enqueue(submission) {
        sender.issue.set(Some(error));
        return false;
    }
    submissions.publish();
    if admit_action(sender, ClientAction::DrainSubmissions) {
        return true;
    }
    let removed = submissions.queue.borrow_mut().rollback(id);
    assert!(
        removed.is_some(),
        "a failed submission wake rolls back the enqueue it just made"
    );
    submissions.publish();
    false
}

fn queue_submissions(
    sender: &SenderHandle,
    submissions: &SubmissionHandle,
    batch: Vec<Submission>,
) -> bool {
    if batch.is_empty() {
        return false;
    }
    let identities = batch
        .iter()
        .map(|submission| submission.id)
        .collect::<Vec<_>>();
    if let Err(error) = submissions.queue.borrow_mut().enqueue_all(batch) {
        sender.issue.set(Some(error));
        return false;
    }
    submissions.publish();
    if admit_action(sender, ClientAction::DrainSubmissions) {
        return true;
    }
    for id in identities {
        let removed = submissions.queue.borrow_mut().rollback(id);
        assert!(
            removed.is_some(),
            "a failed submission wake rolls back every enqueue it just made"
        );
    }
    submissions.publish();
    false
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

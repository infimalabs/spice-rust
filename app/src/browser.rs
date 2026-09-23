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
    BoardRead, GlobalSettingsRead, GlobalSettingsSet, LaneAction, LaneArrange, LaneRead,
    LaneRenewal, LaneState, SteerInbox, SteerSend, TaskAnswer, TeamArrangement, TeamConfigure,
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
use crate::work::{WorkProjection, WorkScope};
use crate::{
    Axis, CanvasDestination, CanvasRuntime, Principal, Session, SteerPane, StreamDirective,
    TaskDetailState, TeamDestination, stale_reads,
};

mod clock;
mod connection;
mod image_viewer;
mod mermaid;
mod task_detail;
mod view;
mod watchband;

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
#[derive(Clone, Default, PartialEq, Eq)]
enum NoteSubmission {
    #[default]
    Idle,
    Pending {
        task: TaskStamp,
        text: String,
        after: Option<Seq>,
    },
    Reconciling {
        task: TaskStamp,
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
#[derive(Clone)]
struct NoteSubmissionHandle {
    state: Rc<RefCell<NoteSubmission>>,
    status: UseStateHandle<NoteSubmission>,
}

impl NoteSubmissionHandle {
    fn replace(&self, status: NoteSubmission) {
        *self.state.borrow_mut() = status.clone();
        self.status.set(status);
    }

    fn current(&self) -> NoteSubmission {
        self.state.borrow().clone()
    }
}

#[derive(Clone)]
struct SurfaceHandles {
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    speech: UseStateHandle<wire::SpeechPlayback>,
    identity: UseStateHandle<Option<CanvasIdentity>>,
    canvas_update: UseStateHandle<Option<CanvasUpdate>>,
    team_controls: UseReducerHandle<TeamControls>,
    composer_drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
    note_text: UseStateHandle<String>,
    note_notice: UseStateHandle<Notice>,
    note_submission: NoteSubmissionHandle,
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
    speech: UseStateHandle<wire::SpeechPlayback>,
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
        match &*self.speech {
            wire::SpeechPlayback::Disabled {} => self.finish(entry.id(), None),
            wire::SpeechPlayback::System {
                voice,
                words_per_minute,
            } => self.start_system(entry, generation, voice, *words_per_minute),
            wire::SpeechPlayback::External {
                content_type,
                available,
                ..
            } => {
                if *available {
                    self.start_external(entry, generation, content_type.clone());
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
            let rendered = request_external_speech(entry.text(), &content_type).await;
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

#[function_component(CanvasApp)]
fn canvas_app() -> Html {
    let connection = use_state(|| Connection::Starting);
    let surface = use_state(SurfaceData::default);
    let axis = use_state(|| Axis::Topology);
    let mosaic = use_state(|| true);
    let canvas_menu_open = use_state(|| false);
    let work_scope = use_state(WorkScope::default);
    let work_scope_notice = use_state(|| None::<String>);
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
    let work_detail = task_detail::use_controls(sender.clone(), surface.clone());
    let speech = use_state(|| wire::SpeechPlayback::Disabled {});
    let speech_status = use_state(SpeechStatus::default);
    let speech_message = use_state(|| None::<String>);
    let speech_controller = use_mut_ref(crate::speech::Controller::default);
    let speech_queue = use_mut_ref(SpeechQueue::default);
    let speech_runtime = SpeechRuntime {
        queue: speech_queue,
        controller: speech_controller,
        speech: speech.clone(),
        status: speech_status.clone(),
        message: speech_message.clone(),
    };
    let current_speech_runtime = use_mut_ref(|| speech_runtime.clone());
    *current_speech_runtime.borrow_mut() = speech_runtime.clone();
    let narration_cursors = use_mut_ref(load_narration_cursors);
    let agent_speech =
        use_agent_speech_controls(current_speech_runtime, narration_cursors, speech_message);
    let agents = use_agent_controls(sender.clone(), surface.clone(), agent_speech);
    let unsafe_intent = use_unsafe_intent(&composer_drafts, &submission_status);
    use_surface_connection(
        sender.clone(),
        SurfaceHandles {
            connection: connection.clone(),
            surface: surface.clone(),
            speech: speech.clone(),
            identity: identity.clone(),
            canvas_update: canvas_update.clone(),
            team_controls: team_controls.clone(),
            composer_drafts: composer_drafts.clone(),
            submissions: submissions.clone(),
            note_text: work_detail.note_text.clone(),
            note_notice: work_detail.notice.clone(),
            note_submission: work_detail.submission.clone(),
        },
    );
    use_team_speech_abandonment(&surface, speech_runtime.clone());
    use_speech_reset(speech.clone(), speech_runtime.clone());
    use_page_hide_speech_stop(speech_runtime.clone());
    use_mosaic_root_scroll(*mosaic);
    use_canvas_menu_dismissal(canvas_menu_open.clone());
    watchband::use_command_modifier_preview();
    let team_count = surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map_or(0, |lanes| {
            lanes.lanes.iter().filter(|team| team.teamed()).count()
        });
    watchband::use_team_fleet_right_edge(team_fleet.clone(), team_count);
    let surface_view = surface.read();
    let work = project_work(surface_view.as_ref());
    let work_scope_available = work
        .as_ref()
        .map(|projection| projection.contains_scope(&work_scope));
    use_work_scope_reconciliation(
        work_scope_available,
        work_scope.clone(),
        work_scope_notice.clone(),
    );

    let refresh = refresh_callback(sender.clone(), axis.clone(), work_detail.selected.clone());
    let canvas = render_canvas(
        CanvasRender {
            connection: &connection,
            surface: &surface,
            axis: &axis,
            mosaic: &mosaic,
            canvas_menu_open: &canvas_menu_open,
            work_scope: &work_scope,
            work_scope_notice: &work_scope_notice,
            identity: &identity,
            canvas_update: &canvas_update,
            team_controls: &team_controls,
            composer_drafts: &composer_drafts,
            submissions: &submissions,
            team_fleet: &team_fleet,
            unsafe_intent,
            sender: &sender,
            agents: &agents,
            selected_task: &work_detail.selected,
            detail_close: &work_detail.close_ref,
            note_text: &work_detail.note_text,
            note_notice: &work_detail.notice,
            note_submission: &work_detail.submission.status,
            action_issue: &action_issue,
            speech_status: &speech_status,
            submit_note: work_detail.submit_note.clone(),
            select_task: work_detail.select.clone(),
            close_task: work_detail.close.clone(),
            refresh,
        },
        work.as_ref(),
    );
    html! { <image_viewer::Provider><mermaid::Provider>{canvas}</mermaid::Provider></image_viewer::Provider> }
}
fn project_work(runtime: Option<&CanvasRuntime>) -> Option<WorkProjection<'_>> {
    runtime
        .and_then(CanvasRuntime::board)
        .map(WorkProjection::from_board)
}

#[hook]
fn use_work_scope_reconciliation(
    available: Option<bool>,
    scope: UseStateHandle<WorkScope>,
    notice: UseStateHandle<Option<String>>,
) {
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
fn use_canvas_menu_dismissal(open: UseStateHandle<bool>) {
    use_effect_with(*open, move |is_open| {
        let registration = (*is_open)
            .then(|| {
                let document = web_sys::window()?.document()?;
                let task_pill_focused = document.active_element().is_some_and(|element| {
                    element.closest(".task-pill-strip").ok().flatten().is_some()
                });
                if !task_pill_focused
                    && let Some(first_destination) = document
                        .query_selector(".canvas-navigation .axis-button")
                        .ok()
                        .flatten()
                        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = first_destination.focus();
                }
                let pointer_open = open.clone();
                let pointer = Closure::<dyn FnMut(web_sys::Event)>::new(move |event| {
                    if !canvas_disclosure_contains(&event) {
                        pointer_open.set(false);
                    }
                });
                let focus_open = open.clone();
                let focus = Closure::<dyn FnMut(web_sys::Event)>::new(move |event| {
                    if !canvas_disclosure_contains(&event) {
                        focus_open.set(false);
                    }
                });
                let key_open = open.clone();
                let key_document = document.clone();
                let key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                    move |event: web_sys::KeyboardEvent| {
                        if event.key() != "Escape" {
                            return;
                        }
                        event.prevent_default();
                        key_open.set(false);
                        if let Some(trigger) = key_document
                            .query_selector(".canvas-menu > summary")
                            .ok()
                            .flatten()
                            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                        {
                            let _ = trigger.focus();
                        }
                    },
                );
                let _ = document.add_event_listener_with_callback(
                    "pointerdown",
                    pointer.as_ref().unchecked_ref(),
                );
                let _ = document
                    .add_event_listener_with_callback("focusin", focus.as_ref().unchecked_ref());
                let _ = document
                    .add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref());
                Some((document, pointer, focus, key))
            })
            .flatten();
        move || {
            if let Some((document, pointer, focus, key)) = registration {
                let _ = document.remove_event_listener_with_callback(
                    "pointerdown",
                    pointer.as_ref().unchecked_ref(),
                );
                let _ = document
                    .remove_event_listener_with_callback("focusin", focus.as_ref().unchecked_ref());
                let _ = document
                    .remove_event_listener_with_callback("keydown", key.as_ref().unchecked_ref());
            }
        }
    });
}

fn canvas_disclosure_contains(event: &web_sys::Event) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|target| {
            target
                .closest(".canvas-disclosure")
                .ok()
                .flatten()
                .is_some()
        })
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
fn use_speech_reset(speech: UseStateHandle<wire::SpeechPlayback>, speech_runtime: SpeechRuntime) {
    let effective = (*speech).clone();
    use_effect_with(effective, move |_| {
        speech_runtime.stop();
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

fn refresh_callback(
    sender: SenderHandle,
    _axis: UseStateHandle<Axis>,
    selected: UseStateHandle<Option<TaskStamp>>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let _admitted = admit_action(&sender, ClientAction::Refresh);
        if let Some(task) = (*selected).clone() {
            let _admitted = admit_action(&sender, ClientAction::TaskDetail { task });
        }
    })
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
        let registration = (*unsafe_intent)
            .then(|| {
                let window = web_sys::window()?;
                let callback =
                    Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                        event.prevent_default();
                        let _ = js_sys::Reflect::set(
                            event.as_ref(),
                            &JsValue::from_str("returnValue"),
                            &JsValue::from_str(""),
                        );
                    });
                window
                    .add_event_listener_with_callback(
                        "beforeunload",
                        callback.as_ref().unchecked_ref(),
                    )
                    .ok()?;
                Some((window, callback))
            })
            .flatten();
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
        name: use_state(String::new),
        worktree: use_state(String::new),
        vendor: use_state(String::new),
        life: use_state(|| wire::intent::TeamLife::Steer.as_str().to_owned()),
        model: use_state(String::new),
        effort: use_state(String::new),
        notice: use_state(Notice::none),
    };
    use_seat_vendor(&surface, seat.vendor.clone());
    view::AgentControls {
        submit_seat: seat_submit(sender, surface, seat.clone()),
        seat,
        speak: speech.speak,
        narrate: speech.narrate,
        suppress_automatic_speech: speech.suppress_automatic,
        speech_message: speech.message,
    }
}

/// Hold the seat draft's vendor on a name the resident actually offers.
///
/// The catalogue arrives with hydration and can change with the resident
/// generation, so the draft adopts the first offered vendor whenever the one
/// it holds is not in the current catalogue. Without it the form would open
/// on a vendor spelling nothing admits.
#[hook]
fn use_seat_vendor(surface: &UseStateHandle<SurfaceData>, vendor: UseStateHandle<String>) {
    let offered: Vec<String> = surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::vendor_catalog)
        .map(|catalog| {
            catalog
                .vendors
                .iter()
                .map(|choice| choice.name.to_string())
                .collect()
        })
        .unwrap_or_default();
    use_effect_with(offered, move |offered| {
        if let Some(first) = offered.first()
            && !offered.contains(&*vendor)
        {
            vendor.set(first.clone());
        }
    });
}

fn seat_submit(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    draft: view::SeatDraft,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(topology_revision) = surface
            .read()
            .as_ref()
            .and_then(|runtime| runtime.lanes())
            .map(|lanes| lanes.topology_revision)
        else {
            return;
        };
        match compose::seat(&draft.name, &draft.worktree, &draft.vendor, &draft.life) {
            Ok(seat) => {
                let Ok(lane) = random_uuid() else {
                    return;
                };
                let selection = compose::selection(&draft.model, &draft.effort);
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
                TeamSpeech::Narrate => Some(line.text),
            };
            if let Some(text) = text {
                speak.emit(SpeechRequest::narration(batch.team.clone(), line.key, text));
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

fn short(value: &str) -> String {
    value.get(..8).unwrap_or(value).to_owned()
}

fn phase_class(status: &str) -> Classes {
    if matches!(status, "completed" | "deleted") {
        classes!("tag")
    } else {
        classes!("tag", "active")
    }
}

fn state_from_input(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlInputElement = event.target_unchecked_into();
        state.set(input.value());
    })
}

fn state_from_select(state: UseStateHandle<String>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
        state.set(select.value());
    })
}

fn state_from_textarea(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
        state.set(input.value());
    })
}

#[wasm_bindgen(start)]
pub fn start() {
    let root = web_sys::window()
        .expect("browser window")
        .document()
        .expect("browser document")
        .get_element_by_id("app")
        .expect("canvas app root");
    root.set_attribute("data-app-source", APP_SOURCE_IDENTITY)
        .expect("stamp canvas app source identity");
    yew::Renderer::<CanvasApp>::with_root(root).render();
}

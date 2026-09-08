use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use futures_channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
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
    LaneRenewal, LaneSeat, LaneState, LineageArrange, LineageArrangement, LineageConfigure,
    LineageLife, SteerInbox, SteerSend, TaskAnswer, TaskNote,
};
use wire::{CanonicalUuid, CommandKind, Response, StreamRequest, TaskHandle, WireVersion, Witness};
use yew::prelude::*;

use crate::command_demands::{DemandQueue, DemandResponse, classify_demand_outcome};
use crate::compose::{self, Notice};
use crate::composer_submissions::{Submission, SubmissionQueue, SubmissionStatus};
use crate::runtime::{CanvasSurface, topology_mutation_requires_refresh};
use crate::speech_queue::{
    Advance, Enqueue, ExternalPause, NarrationCursors, NarrationMessage, SpeechEntry, SpeechQueue,
    SpeechRequest, SuppressAutomatic,
};
use crate::{
    Axis, CanvasDestination, CanvasRuntime, OrgDestination, Principal, Session, SteerPane,
    StreamDirective, stale_reads,
};

mod clock;
mod connection;
mod view;
mod watchband;

use connection::{request_external_speech, run_surface};
use view::{CanvasRender, render_canvas};
use watchband::ComposerAction;
use watchband::{ComposerDrafts, LineageControlAction, LineageControls, LineageSpeech};

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

type SenderHandle = Rc<RefCell<Option<UnboundedSender<ClientAction>>>>;
type SocketWriter = futures_util::stream::SplitSink<WebSocket, Message>;
type SocketReader = futures_util::stream::SplitStream<WebSocket>;

struct EstablishedConnection {
    session: Session,
    writer: SocketWriter,
    reader: SocketReader,
    principal: Principal,
    pending: BTreeMap<CanonicalUuid, PendingKind>,
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

#[derive(Clone)]
struct SurfaceHandles {
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    speech: UseStateHandle<wire::SpeechPlayback>,
    lineage_controls: UseReducerHandle<LineageControls>,
    composer_drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
}

#[derive(Clone, PartialEq)]
enum Connection {
    Starting,
    Connecting,
    Online { actor: String, epoch: String },
    Offline(String),
}

#[derive(Clone, Default, PartialEq)]
enum SpeechStatus {
    #[default]
    Idle,
    Loading,
    Playing,
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct NarrationBatch {
    lineage: String,
    materialization: bool,
    mode: LineageSpeech,
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
        self.status.set(SpeechStatus::Loading);
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
        let id = entry.id();
        let result = self.controller.borrow_mut().start_system(
            generation,
            entry.text(),
            voice,
            words_per_minute,
            move || ended.defer_finish(id, None),
            move || {
                failed.defer_finish(id, Some("system speech playback failed".to_owned()));
            },
        );
        match result {
            Ok(()) => self.status.set(SpeechStatus::Playing),
            Err(error) => self.finish(id, Some(error)),
        }
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
            runtime.status.set(SpeechStatus::Playing);
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
    fn class(&self) -> &'static str {
        match self {
            Self::Online { .. } => "status online",
            Self::Offline(_) => "status offline",
            Self::Starting | Self::Connecting => "status",
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Starting => "Loading embedded canvas".to_owned(),
            Self::Connecting => "Opening current resident".to_owned(),
            Self::Online { actor, epoch } => format!("{actor} / {epoch}"),
            Self::Offline(reason) => format!("Reconnecting / {reason}"),
        }
    }
}

type SurfaceData = CanvasSurface;

#[derive(Clone)]
enum ClientAction {
    /// Wake the connection vendor after a composer synchronously queues work.
    DrainSubmissions,
    Refresh,
    FastMode(bool),
    /// Fetch the next retained page behind one member's oldest visible card.
    MessageHistory {
        lane: CanonicalUuid,
        before: wire::MessageLocus,
    },
    Seat {
        topology_revision: u64,
        lane: CanonicalUuid,
        name: Option<String>,
        worktree: String,
        life: String,
        vendor: wire::Name,
    },
    ArrangeLane {
        topology_revision: u64,
        lane: CanonicalUuid,
        before: CanonicalUuid,
    },
    RenewLane {
        topology_revision: u64,
        lineage: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
    },
    ConfigureLineage {
        topology_revision: u64,
        lineage: CanonicalUuid,
        member: Option<CanonicalUuid>,
        life: Option<LineageLife>,
    },
    /// Where every organization and every agent stands after one organization drag.
    ///
    /// The whole topology travels because that is the only thing the resident
    /// admits: a drop states an arrangement, not a move, so the surface that
    /// drew the organizations is the one that says where they all now stand.
    ArrangeLineages {
        topology_revision: u64,
        lineages: Vec<LineageArrangement>,
    },
    Note {
        task: TaskHandle,
        note: String,
    },
    /// Direction the operator aimed at one seated lane from its tile.
    ///
    /// The occupant and epoch travel with the message because the residency
    /// witness the resident re-proves is built from the same inbox snapshot the
    /// tile was drawn from. Recomputing them at the socket would let the tile
    /// name one lane while the command named another.
    Steer {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
        message: String,
        attachments: Vec<DirectiveAttachment>,
    },
}

#[derive(Clone)]
enum PendingKind {
    Board,
    Lanes {
        /// Wake submissions this read was issued after and may therefore bind.
        wakes: Vec<CanonicalUuid>,
    },
    Inbox {
        /// Accepted keys a successful answer can reconstruct as settled.
        covered: BTreeMap<CanonicalUuid, String>,
    },
    Settings,
    LaneSeat,
    /// One member arrangement whose stale refusal must refresh the roster.
    LaneArrange,
    /// One typed agent-succession request or pre-handoff cancellation.
    LaneRenewal {
        lineage: CanonicalUuid,
        lane: CanonicalUuid,
        requested: bool,
    },
    WakeSubmission {
        lane: CanonicalUuid,
        submission: CanonicalUuid,
    },
    LineageConfigure {
        lineage: CanonicalUuid,
        life: Option<LineageLife>,
    },
    /// An organization arrangement in flight.
    ///
    /// No lineage travels with it: an arrangement names every organization, so there is
    /// no one organization a refusal belongs to. A successful answer replaces the
    /// whole lane table; a refusal re-reads it so a losing client redraws the
    /// winning topology and revision.
    LineageArrange,
    /// Direction in flight, carrying the lane it was aimed at.
    ///
    /// The receipt names that lane, and the only moment it is known for certain
    /// is the send: by the time the answer lands the selection may have moved,
    /// so reading it back off the surface would attribute the receipt to
    /// whichever lane the operator is looking at now.
    Steer,
    SteerSubmission {
        lane: CanonicalUuid,
    },
    Note,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentRead {
    Board,
    Lanes,
    Inbox,
    Settings,
}

impl PendingKind {
    const fn current_read(&self) -> Option<CurrentRead> {
        match self {
            Self::Board => Some(CurrentRead::Board),
            Self::Lanes { .. } => Some(CurrentRead::Lanes),
            Self::Inbox { .. } => Some(CurrentRead::Inbox),
            Self::Settings => Some(CurrentRead::Settings),
            Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneRenewal { .. }
            | Self::WakeSubmission { .. }
            | Self::LineageConfigure { .. }
            | Self::LineageArrange
            | Self::Steer
            | Self::SteerSubmission { .. }
            | Self::Note => None,
        }
    }

    const fn is_demand(&self) -> bool {
        matches!(
            self,
            Self::LaneSeat
                | Self::LaneArrange
                | Self::LineageConfigure { .. }
                | Self::LineageArrange
        )
    }
}

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
    let lineage_controls = use_reducer(LineageControls::restore);
    let lineage_fleet = use_node_ref();
    let composer_drafts = use_reducer(ComposerDrafts::default);
    let submission_status = use_state(SubmissionStatus::default);
    let submissions = SubmissionHandle {
        queue: use_mut_ref(SubmissionQueue::default),
        status: submission_status.clone(),
    };
    let note_task = use_state(String::new);
    let note_text = use_state(String::new);
    let note_notice = use_state(Notice::none);
    let sender = use_mut_ref(|| None::<UnboundedSender<ClientAction>>);
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
            lineage_controls: lineage_controls.clone(),
            composer_drafts: composer_drafts.clone(),
            submissions: submissions.clone(),
        },
    );
    use_lineage_speech_abandonment(&surface, speech_runtime.clone());
    use_speech_reset(speech.clone(), speech_runtime.clone());
    use_mosaic_root_scroll(*mosaic);
    use_canvas_menu_dismissal(canvas_menu_open.clone());
    watchband::use_command_modifier_preview();
    let lineage_count = surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map_or(0, |lanes| lanes.lanes.len());
    watchband::use_lineage_fleet_right_edge(lineage_fleet.clone(), lineage_count);

    let refresh = refresh_callback(sender.clone(), axis.clone());
    let submit_note = note_submit_callback(
        sender.clone(),
        note_task.clone(),
        note_text.clone(),
        note_notice.clone(),
    );
    render_canvas(CanvasRender {
        connection: &connection,
        surface: &surface,
        axis: &axis,
        mosaic: &mosaic,
        canvas_menu_open: &canvas_menu_open,
        lineage_controls: &lineage_controls,
        composer_drafts: &composer_drafts,
        submissions: &submissions,
        lineage_fleet: &lineage_fleet,
        unsafe_intent,
        sender: &sender,
        agents: &agents,
        note_task: &note_task,
        note_text: &note_text,
        note_notice: &note_notice,
        submit_note,
        refresh,
    })
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
        let (tx, rx) = mpsc::unbounded();
        *sender.borrow_mut() = Some(tx);
        spawn_local(run_surface(active.clone(), handles, rx));
        move || {
            active.set(false);
            sender.borrow_mut().take();
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

#[hook]
fn use_lineage_speech_abandonment(
    surface: &UseStateHandle<SurfaceData>,
    speech_runtime: SpeechRuntime,
) {
    let lineages = surface
        .read()
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| {
            lanes
                .lanes
                .iter()
                .map(|lineage| lineage.lane.to_string())
                .collect::<BTreeSet<_>>()
        });
    let previous = use_mut_ref(|| None::<BTreeSet<String>>);
    use_effect_with(lineages, move |lineages| {
        let Some(current) = lineages else {
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

fn note_submit_callback(
    sender: SenderHandle,
    note_task: UseStateHandle<String>,
    note_text: UseStateHandle<String>,
    note_notice: UseStateHandle<Notice>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        match compose::note(&note_task, &note_text) {
            Ok(note) => {
                note_notice.set(Notice::none());
                send_action(
                    &sender,
                    ClientAction::Note {
                        task: note.task,
                        note: note.text,
                    },
                );
                note_text.set(String::new());
            }
            Err(refusal) => note_notice.set(refusal.into()),
        }
    })
}

fn refresh_callback(sender: SenderHandle, _axis: UseStateHandle<Axis>) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        send_action(&sender, ClientAction::Refresh);
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
    let seat_name = use_state(String::new);
    let seat_worktree = use_state(String::new);
    let seat_vendor = use_state(|| "codex".to_owned());
    let seat_life = use_state(|| "steer".to_owned());
    let seat_notice = use_state(Notice::none);
    view::AgentControls {
        submit_seat: seat_submit(
            sender,
            surface,
            seat_name.clone(),
            seat_worktree.clone(),
            seat_vendor.clone(),
            seat_life.clone(),
            seat_notice.clone(),
        ),
        seat_name,
        seat_worktree,
        seat_vendor,
        seat_life,
        seat_notice,
        speak: speech.speak,
        narrate: speech.narrate,
        suppress_automatic_speech: speech.suppress_automatic,
        speech_message: speech.message,
    }
}

fn seat_submit(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    name: UseStateHandle<String>,
    worktree: UseStateHandle<String>,
    vendor: UseStateHandle<String>,
    life: UseStateHandle<String>,
    notice: UseStateHandle<Notice>,
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
        match compose::seat(&name, &worktree, &vendor, &life) {
            Ok(seat) => {
                let Ok(lane) = random_uuid() else {
                    return;
                };
                notice.set(Notice::none());
                send_action(
                    &sender,
                    ClientAction::Seat {
                        topology_revision,
                        lane,
                        name: seat.name,
                        worktree: seat.worktree,
                        life: seat.life,
                        vendor: seat.vendor,
                    },
                );
            }
            Err(refusal) => notice.set(refusal.into()),
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
                LineageSpeech::Quiet => None,
                LineageSpeech::Speak => line.automatic_speech,
                LineageSpeech::Narrate => Some(line.text),
            };
            if let Some(text) = text {
                speak.emit(SpeechRequest::narration(
                    batch.lineage.clone(),
                    line.key,
                    text,
                ));
            }
        }
    })
}

fn apply_lineage_configuration(
    session: &Session,
    lineage_controls: &UseReducerHandle<LineageControls>,
    runtime: &mut CanvasRuntime,
    lineage: CanonicalUuid,
    life: Option<LineageLife>,
    bytes: &[u8],
) -> Result<(bool, bool), String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let refresh = topology_mutation_requires_refresh(&admitted.get().outcome);
    let applied = runtime.apply_lanes(&admitted);
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) if applied => {
            lineage_controls.dispatch(LineageControlAction::SyncLanes {
                answer: reply.body.clone(),
                settle: life.map(|life| (lineage, life.into())),
            });
        }
        wire::Outcome::Reply(_) => {
            if let Some(life) = life {
                lineage_controls.dispatch(LineageControlAction::SettleStaleLife {
                    lineage,
                    life: life.into(),
                });
            }
        }
        wire::Outcome::Refusal(refusal) => {
            lineage_controls.dispatch(LineageControlAction::SettleIssue {
                lineage,
                life: life.map(Into::into),
                issue: format!("{}: {}", refusal.reason(), refusal.explanation()),
            });
        }
        wire::Outcome::Fault(fault) => {
            lineage_controls.dispatch(LineageControlAction::SettleIssue {
                lineage,
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

fn send_action(sender: &SenderHandle, action: ClientAction) -> bool {
    sender
        .borrow_mut()
        .as_mut()
        .is_some_and(|sender| sender.unbounded_send(action).is_ok())
}

fn queue_submission(
    sender: &SenderHandle,
    submissions: &SubmissionHandle,
    submission: Submission,
) -> bool {
    let id = submission.id;
    if submissions.queue.borrow_mut().enqueue(submission).is_err() {
        return false;
    }
    submissions.publish();
    if send_action(sender, ClientAction::DrainSubmissions) {
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
    if submissions.queue.borrow_mut().enqueue_all(batch).is_err() {
        return false;
    }
    submissions.publish();
    if send_action(sender, ClientAction::DrainSubmissions) {
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
    if status == "completed" {
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

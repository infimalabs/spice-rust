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
    BoardRead, ConsentPolicy, ConsentRead, EffortRead, LaneArrange, LaneRead, LaneSeat,
    LineageArrange, LineageArrangement, LineageConfigure, LineageLife, SteerInbox, SteerSend,
    TaskAnswer, TaskNote,
};
use wire::{
    Answer, CanonicalUuid, CommandKind, Response, StreamRequest, TaskHandle, WireVersion, Witness,
};
use yew::prelude::*;

use crate::compose::{self, Notice};
use crate::composer_submissions::{Submission, SubmissionQueue, SubmissionStatus};
use crate::speech_queue::{
    Advance, Enqueue, ExternalPause, NarrationCursors, NarrationMessage, SpeechEntry, SpeechQueue,
    SpeechRequest,
};
use crate::{
    Axis, CanvasRuntime, ChainEnd, Principal, Session, SteerPane, StreamDirective, stale_reads,
};

mod clock;
mod connection;
mod view;
mod watchband;

use clock::WallClock;
use connection::{request_external_speech, run_surface};
use view::{CanvasRender, render_canvas};
use watchband::ComposerAction;
use watchband::{ComposerDrafts, LineageControlAction, LineageControls, LineageMode};

/// Source identity injected by `cargo xtask app` and verified before publish.
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
        // back to the initial empty state. Publish the queue projection
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
    enabled: bool,
    messages: Vec<NarrationMessage>,
}

#[derive(Clone)]
struct SpeechRuntime {
    queue: Rc<RefCell<SpeechQueue>>,
    controller: Rc<RefCell<crate::speech::Controller>>,
    speech: UseStateHandle<wire::SpeechPlayback>,
    status: UseStateHandle<SpeechStatus>,
}

impl SpeechRuntime {
    fn enqueue(&self, request: SpeechRequest) {
        let decision = self.queue.borrow_mut().enqueue(request);
        if let Enqueue::Start(entry) = decision {
            self.start(entry);
        }
    }

    fn start(&self, entry: SpeechEntry) {
        let generation = self.controller.borrow_mut().begin();
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
                self.status.set(SpeechStatus::Idle);
            }
            ExternalPause::Start(entry) => self.start(entry),
        }
    }

    fn active(&self) -> bool {
        self.queue.borrow().active()
    }

    fn stop(&self) {
        self.queue.borrow_mut().stop();
        self.controller.borrow_mut().stop();
        self.status.set(SpeechStatus::Idle);
    }

    fn abandon_lane(&self, lane: &str) {
        self.queue.borrow_mut().abandon_lane(lane);
    }
}

impl SpeechStatus {
    fn active(&self) -> bool {
        matches!(self, Self::Loading | Self::Playing)
    }

    fn state(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Playing => "playing",
            Self::Failed(_) => "error",
        }
    }

    fn label(&self) -> &'static str {
        if self.active() {
            "Stop reading"
        } else {
            "Read current canvas"
        }
    }

    fn title(&self) -> String {
        match self {
            Self::Idle => "Read current canvas".to_owned(),
            Self::Loading => "Stop speech rendering".to_owned(),
            Self::Playing => "Stop reading".to_owned(),
            Self::Failed(reason) => format!("Speech unavailable: {reason}"),
        }
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

type SurfaceData = Option<CanvasRuntime>;

#[derive(Clone)]
enum ClientAction {
    /// Wake the connection driver after a composer synchronously queues work.
    DrainSubmissions,
    Refresh,
    Seat {
        name: Option<String>,
        worktree: String,
        life: String,
        vendor: wire::Name,
    },
    ArrangeLane {
        lane: CanonicalUuid,
        before: CanonicalUuid,
    },
    ConfigureLineage {
        lineage: CanonicalUuid,
        member: Option<CanonicalUuid>,
        life: LineageLife,
    },
    /// Where every team and every agent stands after one team drag.
    ///
    /// The whole topology travels because that is the only thing the resident
    /// admits: a drop states an arrangement, not a move, so the surface that
    /// drew the teams is the one that says where they all now stand.
    ArrangeLineages {
        lineages: Vec<LineageArrangement>,
    },
    Note {
        task: TaskHandle,
        note: String,
    },
    Consent {
        request: CanonicalUuid,
        rule: wire::Name,
        answer: Answer,
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
    Lanes,
    Inbox {
        /// Accepted keys a successful answer can reconstruct as settled.
        covered: BTreeMap<CanonicalUuid, String>,
    },
    Consent,
    Policy,
    Effort,
    LaneSeat,
    LineageConfigure(CanonicalUuid),
    /// A team arrangement in flight.
    ///
    /// No lineage travels with it: an arrangement names every team, so there is
    /// no one team a refusal belongs to. The answer replaces the whole lane
    /// table, which is exactly what a refused arrangement leaves unchanged.
    LineageArrange,
    /// Direction in flight, carrying the lane it was aimed at.
    ///
    /// The receipt names that lane, and the only moment it is known for certain
    /// is the send: by the time the answer lands the selection may have moved,
    /// so reading it back off the surface would attribute the receipt to
    /// whichever lane the operator is looking at now.
    Steer {
        lane: CanonicalUuid,
    },
    SteerSubmission {
        lane: CanonicalUuid,
    },
    Note(TaskHandle),
    ConsentDecision,
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
    let lineage_mode = use_state(|| LineageMode::Compose);
    let lineage_controls = use_reducer(LineageControls::default);
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
    let speech_controller = use_mut_ref(crate::speech::Controller::default);
    let speech_queue = use_mut_ref(SpeechQueue::default);
    let speech_runtime = SpeechRuntime {
        queue: speech_queue,
        controller: speech_controller,
        speech: speech.clone(),
        status: speech_status.clone(),
    };
    let speak = {
        let speech_runtime = speech_runtime.clone();
        Callback::from(move |request| speech_runtime.enqueue(request))
    };
    let narration_cursors = use_mut_ref(load_narration_cursors);
    let narrate = narration_callback(narration_cursors, speak.clone());
    let agents = use_agent_controls(sender.clone(), speak.clone(), narrate);
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

    let refresh = refresh_callback(sender.clone());
    let submit_note = note_submit_callback(
        sender.clone(),
        note_task.clone(),
        note_text.clone(),
        note_notice.clone(),
    );
    let toggle_speech = speech_toggle(speech_status.clone(), speech_runtime, speak);

    // The clock wraps the canvas rather than living inside it so that a tick
    // re-renders `WallClock` and the `Age` leaves that read its context, and
    // never `CanvasApp` itself. Sampling the time as a hook here instead would
    // rebuild the whole tree every second, which is how a once-a-second label
    // turns into a once-a-second Mosaic re-measurement.
    html! {
        <WallClock>{render_canvas(CanvasRender {
        connection: &connection,
        surface: &surface,
        axis: &axis,
        mosaic: &mosaic,
        lineage_mode: &lineage_mode,
        lineage_controls: &lineage_controls,
        composer_drafts: &composer_drafts,
        submissions: &submissions,
        unsafe_intent,
        sender: &sender,
        agents: &agents,
        note_task: &note_task,
        note_text: &note_text,
        note_notice: &note_notice,
        submit_note,
        refresh,
        speech: &speech,
        speech_status: &speech_status,
        toggle_speech,
        })}</WallClock>
    }
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
    let lineages = (**surface)
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

fn refresh_callback(sender: SenderHandle) -> Callback<MouseEvent> {
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

#[hook]
fn use_agent_controls(
    sender: SenderHandle,
    speak: Callback<SpeechRequest>,
    narrate: Callback<NarrationBatch>,
) -> view::AgentControls {
    let seat_name = use_state(String::new);
    let seat_worktree = use_state(String::new);
    let seat_vendor = use_state(|| "codex".to_owned());
    let seat_life = use_state(|| "steer".to_owned());
    let seat_notice = use_state(Notice::none);
    view::AgentControls {
        submit_seat: seat_submit(
            sender,
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
        speak,
        narrate,
    }
}

fn seat_submit(
    sender: SenderHandle,
    name: UseStateHandle<String>,
    worktree: UseStateHandle<String>,
    vendor: UseStateHandle<String>,
    life: UseStateHandle<String>,
    notice: UseStateHandle<Notice>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        match compose::seat(&name, &worktree, &vendor, &life) {
            Ok(seat) => {
                notice.set(Notice::none());
                send_action(
                    &sender,
                    ClientAction::Seat {
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

/// Stop what is playing, or read the whole rendered surface aloud.
///
/// The topbar control is a toggle because it speaks one thing; a card speaks
/// its own text and both enter through the same browser-global runtime.
fn speech_toggle(
    speech_status: UseStateHandle<SpeechStatus>,
    speech_runtime: SpeechRuntime,
    speak: Callback<SpeechRequest>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        if speech_runtime.active() {
            speech_runtime.stop();
            return;
        }
        match crate::speech::rendered_text() {
            Ok(text) => speak.emit(SpeechRequest::manual(None, text)),
            Err(error) => speech_status.set(SpeechStatus::Failed(error)),
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
        if !batch.enabled {
            return;
        }
        for line in lines {
            speak.emit(SpeechRequest::narration(batch.lineage.clone(), line.text));
        }
    })
}

fn apply_lineage_configuration(
    session: &Session,
    lineage_controls: &UseReducerHandle<LineageControls>,
    runtime: &mut CanvasRuntime,
    lineage: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let refresh = match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => {
            lineage_controls.dispatch(LineageControlAction::SyncLanes {
                answer: reply.body.clone(),
                settle: true,
            });
            false
        }
        wire::Outcome::Refusal(refusal) => {
            lineage_controls.dispatch(LineageControlAction::SettleIssue {
                lineage,
                issue: format!("{}: {}", refusal.reason, refusal.remedy),
            });
            true
        }
        wire::Outcome::Fault(fault) => {
            lineage_controls.dispatch(LineageControlAction::SettleIssue {
                lineage,
                issue: format!("{}: {}", fault.reason, fault.detail),
            });
            true
        }
    };
    runtime.apply_lanes(&admitted);
    Ok(refresh)
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

fn metric(label: &'static str, value: String, detail: String) -> Html {
    html! {
        <div class="metric">
            <div class="metric-label">{label}</div>
            <div class="metric-value">{value}</div>
            <div class="metric-detail">{detail}</div>
        </div>
    }
}

fn empty(message: &'static str) -> Html {
    html! { <div class="empty">{message}</div> }
}

fn title(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for (index, word) in text.split('-').enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            output.extend(first.to_uppercase());
            output.extend(characters);
        }
    }
    output
}

fn short(value: &str) -> String {
    value.get(..8).unwrap_or(value).to_owned()
}

fn resident_state(surface: &SurfaceData) -> String {
    surface
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| format!("{:?}", lanes.state).to_lowercase())
        .unwrap_or_else(|| "resident state".to_owned())
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

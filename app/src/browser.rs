use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
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
    BoardRead, ConsentPolicy, ConsentRead, LaneRead, LaneSeat, SteerInbox, SteerSend, TaskAnswer,
    TaskNote,
};
use wire::{
    Answer, CanonicalUuid, CommandKind, Response, StreamRequest, TaskHandle, WireVersion, Witness,
};
use yew::prelude::*;

use crate::compose::{self, Notice};
use crate::{
    Axis, CanvasRuntime, ChainEnd, Principal, Session, SteerPane, StreamDirective, stale_reads,
};

mod composer;
mod view;

use composer::{ComposerDrafts, LineageLife, LineageMode};
use view::{CanvasRender, render_canvas};

/// Source identity injected by `cargo xtask app` and verified before publish.
const APP_SOURCE_IDENTITY: &str = match option_env!("SPICE_APP_SOURCE_IDENTITY") {
    // env-policy: allow
    Some(identity) => identity,
    None => "unbound-development-build",
};

type SenderHandle = Rc<RefCell<Option<UnboundedSender<ClientAction>>>>;
type SocketWriter = futures_util::stream::SplitSink<WebSocket, Message>;
type SocketReader = futures_util::stream::SplitStream<WebSocket>;

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
    Refresh,
    Seat {
        worktree: String,
        life: String,
        vendor: wire::Name,
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
    Inbox,
    Consent,
    Policy,
    LaneSeat,
    /// Direction in flight, carrying the lane it was aimed at.
    ///
    /// The receipt names that lane, and the only moment it is known for certain
    /// is the send: by the time the answer lands the selection may have moved,
    /// so reading it back off the surface would attribute the receipt to
    /// whichever lane the operator is looking at now.
    Steer(CanonicalUuid),
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
    let lineage_life = use_state(|| LineageLife::Steer);
    let composer_drafts = use_reducer(ComposerDrafts::default);
    let note_task = use_state(String::new);
    let note_text = use_state(String::new);
    let sender = use_mut_ref(|| None::<UnboundedSender<ClientAction>>);
    let speech = use_state(|| wire::SpeechPlayback::Disabled {});
    let speech_status = use_state(SpeechStatus::default);
    let speech_controller = use_mut_ref(crate::speech::Controller::default);
    let agents = use_agent_controls(
        sender.clone(),
        lineage_life.clone(),
        speech_speak(
            speech.clone(),
            speech_status.clone(),
            speech_controller.clone(),
        ),
    );

    {
        let connection = connection.clone();
        let surface = surface.clone();
        let sender = sender.clone();
        let speech = speech.clone();
        use_effect_with((), move |()| {
            let active = Rc::new(Cell::new(true));
            let (tx, rx) = mpsc::unbounded();
            *sender.borrow_mut() = Some(tx);
            spawn_local(run_surface(active.clone(), connection, surface, speech, rx));
            move || {
                active.set(false);
                sender.borrow_mut().take();
            }
        });
    }
    {
        let effective = (*speech).clone();
        let speech_controller = speech_controller.clone();
        let speech_status = speech_status.clone();
        use_effect_with(effective, move |_| {
            speech_controller.borrow_mut().stop();
            speech_status.set(SpeechStatus::Idle);
        });
    }

    let refresh = {
        let sender = sender.clone();
        Callback::from(move |_| send_action(&sender, ClientAction::Refresh))
    };
    let submit_note = {
        let sender = sender.clone();
        let note_task = note_task.clone();
        let note_text = note_text.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Ok(task) = (*note_task).parse::<TaskHandle>() else {
                return;
            };
            let note = note_text.trim().to_owned();
            if note.is_empty() {
                return;
            }
            send_action(&sender, ClientAction::Note { task, note });
            note_text.set(String::new());
        })
    };
    let toggle_speech = speech_toggle(
        speech_status.clone(),
        speech_controller.clone(),
        agents.speak.clone(),
    );

    render_canvas(CanvasRender {
        connection: &connection,
        surface: &surface,
        axis: &axis,
        mosaic: &mosaic,
        lineage_mode: &lineage_mode,
        lineage_life: &lineage_life,
        composer_drafts: &composer_drafts,
        sender: &sender,
        agents: &agents,
        note_task: &note_task,
        note_text: &note_text,
        submit_note,
        refresh,
        speech: &speech,
        speech_status: &speech_status,
        toggle_speech,
    })
}

#[hook]
fn use_agent_controls(
    sender: SenderHandle,
    lineage_life: UseStateHandle<LineageLife>,
    speak: Callback<String>,
) -> view::AgentControls {
    let seat_worktree = use_state(String::new);
    let seat_vendor = use_state(|| "codex".to_owned());
    let seat_life = use_state(|| "steer".to_owned());
    let seat_notice = use_state(Notice::none);
    view::AgentControls {
        submit_seat: seat_submit(
            sender,
            seat_worktree.clone(),
            seat_vendor.clone(),
            seat_life.clone(),
            seat_notice.clone(),
            lineage_life,
        ),
        seat_worktree,
        seat_vendor,
        seat_life,
        seat_notice,
        speak,
    }
}

fn seat_submit(
    sender: SenderHandle,
    worktree: UseStateHandle<String>,
    vendor: UseStateHandle<String>,
    life: UseStateHandle<String>,
    notice: UseStateHandle<Notice>,
    lineage_life: UseStateHandle<LineageLife>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        match compose::seat(&worktree, &vendor, &life) {
            Ok(seat) => {
                notice.set(Notice::none());
                if let Some(selected_life) = LineageLife::from_wire(&seat.life) {
                    lineage_life.set(selected_life);
                }
                send_action(
                    &sender,
                    ClientAction::Seat {
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
/// its own text and shares only the second half of that, so the two enter
/// through [`speech_speak`] rather than each driving the controller.
fn speech_toggle(
    speech_status: UseStateHandle<SpeechStatus>,
    speech_controller: Rc<RefCell<crate::speech::Controller>>,
    speak: Callback<String>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        if speech_status.active() {
            speech_controller.borrow_mut().stop();
            speech_status.set(SpeechStatus::Idle);
            return;
        }
        match crate::speech::rendered_text() {
            Ok(text) => speak.emit(text),
            Err(error) => speech_status.set(SpeechStatus::Failed(error)),
        }
    })
}

fn speech_speak(
    speech: UseStateHandle<wire::SpeechPlayback>,
    speech_status: UseStateHandle<SpeechStatus>,
    speech_controller: Rc<RefCell<crate::speech::Controller>>,
) -> Callback<String> {
    Callback::from(move |text: String| {
        let generation = speech_controller.borrow_mut().begin();
        speech_status.set(SpeechStatus::Loading);
        match &*speech {
            wire::SpeechPlayback::Disabled {} => speech_status.set(SpeechStatus::Idle),
            wire::SpeechPlayback::System {
                voice,
                words_per_minute,
            } => {
                let ended = speech_status.clone();
                let failed = speech_status.clone();
                let result = speech_controller.borrow_mut().start_system(
                    generation,
                    &text,
                    voice,
                    *words_per_minute,
                    move || ended.set(SpeechStatus::Idle),
                    move || {
                        failed.set(SpeechStatus::Failed(
                            "system speech playback failed".to_owned(),
                        ));
                    },
                );
                match result {
                    Ok(()) => speech_status.set(SpeechStatus::Playing),
                    Err(error) => speech_status.set(SpeechStatus::Failed(error)),
                }
            }
            wire::SpeechPlayback::External {
                content_type,
                available,
                ..
            } => {
                if !available {
                    speech_status.set(SpeechStatus::Failed(
                        "external speech backend is unavailable".to_owned(),
                    ));
                    return;
                }
                let content_type = content_type.clone();
                let speech_status = speech_status.clone();
                let speech_controller = speech_controller.clone();
                spawn_local(async move {
                    let result = request_external_speech(&text, &content_type).await;
                    if !speech_controller.borrow().is_current(generation) {
                        return;
                    }
                    let bytes = match result {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            speech_status.set(SpeechStatus::Failed(error));
                            return;
                        }
                    };
                    let ended = speech_status.clone();
                    let failed = speech_status.clone();
                    let playing = speech_controller.borrow_mut().start_external(
                        generation,
                        &bytes,
                        &content_type,
                        move || ended.set(SpeechStatus::Idle),
                        move || {
                            failed.set(SpeechStatus::Failed(
                                "rendered speech playback failed".to_owned(),
                            ));
                        },
                    );
                    let playing = match playing {
                        Ok(playing) => playing,
                        Err(error) => {
                            speech_status.set(SpeechStatus::Failed(error));
                            return;
                        }
                    };
                    speech_status.set(SpeechStatus::Playing);
                    if let Err(error) = crate::speech::await_play(playing).await
                        && speech_controller.borrow().is_current(generation)
                    {
                        speech_controller.borrow_mut().stop();
                        speech_status.set(SpeechStatus::Failed(error));
                    }
                });
            }
        }
    })
}

async fn run_surface(
    active: Rc<Cell<bool>>,
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    speech: UseStateHandle<wire::SpeechPlayback>,
    mut actions: UnboundedReceiver<ClientAction>,
) {
    let data = Rc::new(RefCell::new(SurfaceData::default()));
    while active.get() {
        if matches!(&*connection, Connection::Starting) {
            connection.set(Connection::Connecting);
        }
        let result =
            run_connection(&active, &connection, &surface, &speech, &data, &mut actions).await;
        if !active.get() {
            break;
        }
        let reason = result.unwrap_err_or_else();
        connection.set(Connection::Offline(reason.clone()));
        TimeoutFuture::new(1_200).await;
    }
}

trait ConnectionResult {
    fn unwrap_err_or_else(self) -> String;
}

impl ConnectionResult for Result<(), String> {
    fn unwrap_err_or_else(self) -> String {
        match self {
            Ok(()) => "resident closed the socket".to_owned(),
            Err(error) => error,
        }
    }
}

async fn run_connection(
    active: &Rc<Cell<bool>>,
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    speech: &UseStateHandle<wire::SpeechPlayback>,
    data: &Rc<RefCell<SurfaceData>>,
    actions: &mut UnboundedReceiver<ClientAction>,
) -> Result<(), String> {
    let (session, mut writer, mut reader) = open_socket(speech).await?;
    let principal = admit_runtime(&session, &mut reader, connection, surface, data).await?;
    let mut pending = BTreeMap::new();
    send_subscription(&principal, data, &mut writer).await?;
    send_refresh(&principal, &mut writer, &mut pending).await?;

    while active.get() {
        let incoming = reader.next().fuse();
        let action = actions.next().fuse();
        futures_util::pin_mut!(incoming, action);
        select! {
            frame = incoming => {
                let frame = frame
                    .ok_or_else(|| "resident closed the socket".to_owned())?
                    .map_err(websocket_error)?;
                let bytes = binary_message(frame)?;
                let (directives, refresh) =
                    handle_incoming(&session, surface, data, &mut pending, &bytes)?;
                relay_directives(&principal, &mut writer, directives).await?;
                if refresh {
                    send_refresh(&principal, &mut writer, &mut pending).await?;
                }
            },
            action = action => {
                let action = action.ok_or_else(|| "canvas command channel closed".to_owned())?;
                dispatch_action(&principal, &mut writer, &mut pending, action).await?;
            }
        }
    }
    Ok(())
}

/// Discover the resident endpoint and open the socket its bootstrap names.
async fn open_socket(
    speech: &UseStateHandle<wire::SpeechPlayback>,
) -> Result<(Session, SocketWriter, SocketReader), String> {
    let discovery: wire::CanvasBootstrap = Request::get(&authenticated_path(
        "/bootstrap",
        browser_auth_token()?.as_deref(),
    ))
    .send()
    .await
    .map_err(|error| format!("bootstrap request failed: {error}"))?
    .json()
    .await
    .map_err(|error| format!("bootstrap response was invalid: {error}"))?;
    speech.set(discovery.speech.clone());
    let session = Session::open(discovery.capability, discovery.epoch);
    let endpoint = discovery
        .endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "bootstrap endpoint was not the current HTTP origin".to_owned())?;
    let socket = WebSocket::open(&format!(
        "ws://{endpoint}/socket?auth={}",
        discovery.capability
    ))
    .map_err(|error| format!("socket did not open: {error}"))?;
    let (mut writer, reader) = socket.split();
    writer
        .send(Message::Bytes(
            wire::encode(&session.hello()).map_err(|error| error.to_string())?,
        ))
        .await
        .map_err(websocket_error)?;
    Ok((session, writer, reader))
}

/// Admit the acceptance frame and publish the runtime that frame names.
async fn admit_runtime(
    session: &Session,
    reader: &mut SocketReader,
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
) -> Result<Principal, String> {
    let accepted_bytes = binary_message(
        reader
            .next()
            .await
            .ok_or_else(|| "socket closed before acceptance".to_owned())?
            .map_err(websocket_error)?,
    )?;
    let accepted = session
        .admit_accepted(&accepted_bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    let principal = {
        let mut data = data.borrow_mut();
        match data.as_mut() {
            Some(runtime) => {
                runtime.reconnect(&accepted);
            }
            None => *data = Some(CanvasRuntime::new(&accepted)),
        }
        data.as_ref()
            .expect("accepted connection creates runtime")
            .principal()
            .clone()
    };
    surface.set(data.borrow().clone());
    connection.set(Connection::Online {
        actor: principal.actor().to_string(),
        epoch: principal.epoch().to_string(),
    });
    Ok(principal)
}

/// Carry back the stream directives one admitted frame produced.
async fn relay_directives(
    principal: &Principal,
    writer: &mut SocketWriter,
    directives: Vec<StreamDirective>,
) -> Result<(), String> {
    for directive in directives {
        if send_stream_directive(principal, writer, directive).await? {
            return Err("resident stream requested reconnect".to_owned());
        }
    }
    Ok(())
}

/// Send one operator action as the typed command the resident admits.
async fn dispatch_action(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    action: ClientAction,
) -> Result<(), String> {
    match action {
        ClientAction::Refresh => send_refresh(principal, writer, pending).await,
        ClientAction::Seat {
            worktree,
            life,
            vendor,
        } => {
            let lane = random_uuid()?;
            let body = LaneSeat {
                lane,
                lineage_id: lane,
                worktree,
                life,
                vendor,
            };
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::LaneSeat,
                CommandKind::LaneSeat,
                vec![],
                body,
            )
            .await
        }
        ClientAction::Note { task, note } => {
            let body = TaskNote {
                task: task.clone(),
                note,
            };
            let expected = vec![Witness::Existence { task: task.clone() }];
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::Note(task),
                CommandKind::TaskNote,
                expected,
                body,
            )
            .await
        }
        ClientAction::Consent {
            request,
            rule,
            answer,
        } => {
            let body = wire::intent::ConsentDecide { request, answer };
            let expected = vec![
                Witness::Consent {
                    request,
                    answer: Answer::Asked,
                },
                Witness::Rule { name: rule },
            ];
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::ConsentDecision,
                CommandKind::ConsentDecide,
                expected,
                body,
            )
            .await
        }
        ClientAction::Steer {
            lane,
            occupant,
            epoch,
            message,
            attachments,
        } => {
            let body = SteerSend {
                lane,
                message,
                attachments,
            };
            let expected = vec![Witness::Residency {
                lane,
                occupant,
                epoch,
            }];
            send_typed(
                principal,
                writer,
                pending,
                PendingKind::Steer(lane),
                CommandKind::SteerSend,
                expected,
                body,
            )
            .await
        }
    }
}

async fn request_external_speech(text: &str, content_type: &str) -> Result<Vec<u8>, String> {
    let request = Request::post(&authenticated_path(
        "/speech",
        browser_auth_token()?.as_deref(),
    ))
    .json(&wire::SpeechRequest {
        text: text.to_owned(),
    })
    .map_err(|error| format!("speech request could not be encoded: {error}"))?;
    let response = request
        .send()
        .await
        .map_err(|error| format!("speech request failed: {error}"))?;
    if !response.ok() {
        let status = response.status();
        let detail = response
            .text()
            .await
            .unwrap_or_else(|_| "speech renderer returned no detail".to_owned());
        return Err(bounded_speech_error(&format!(
            "speech renderer returned {status}: {detail}"
        )));
    }
    let actual = response.headers().get("content-type").unwrap_or_default();
    if actual != content_type {
        return Err(format!(
            "speech renderer returned {actual:?}, expected {content_type:?}"
        ));
    }
    let bytes = response
        .binary()
        .await
        .map_err(|error| format!("speech audio response was invalid: {error}"))?;
    if bytes.is_empty() {
        return Err("speech renderer returned empty audio".to_owned());
    }
    Ok(bytes)
}

fn browser_auth_token() -> Result<Option<String>, String> {
    let hash = web_sys::window()
        .ok_or_else(|| "browser window is unavailable".to_owned())?
        .location()
        .hash()
        .map_err(|_| "browser URL fragment is unavailable".to_owned())?;
    Ok(hash
        .strip_prefix('#')
        .filter(|token| !token.is_empty())
        .map(str::to_owned))
}

fn authenticated_path(path: &str, token: Option<&str>) -> String {
    token.map_or_else(|| path.to_owned(), |token| format!("{path}?auth={token}"))
}

fn bounded_speech_error(error: &str) -> String {
    let mut end = error.len().min(240);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_owned()
}

async fn send_refresh(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Board,
        CommandKind::BoardRead,
        vec![],
        BoardRead { family: None },
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Lanes,
        CommandKind::LaneRead,
        vec![],
        LaneRead {},
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Inbox,
        CommandKind::SteerInbox,
        vec![],
        SteerInbox { lane: None },
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Consent,
        CommandKind::ConsentRead,
        vec![],
        ConsentRead {},
    )
    .await?;
    send_if_offered(
        principal,
        writer,
        pending,
        PendingKind::Policy,
        CommandKind::ConsentPolicy,
        vec![],
        ConsentPolicy {},
    )
    .await
}

async fn send_if_offered<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    if !SteerPane::over(principal).offers(kind) {
        return Ok(());
    }
    send_typed(
        principal,
        writer,
        pending,
        pending_kind,
        kind,
        expected,
        body,
    )
    .await
}

async fn send_typed<B: Serialize>(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    pending_kind: PendingKind,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: B,
) -> Result<(), String> {
    let id = random_uuid()?;
    let command = SteerPane::over(principal)
        .command(id, kind, expected, body)
        .map_err(|error| error.to_string())?;
    let bytes = wire::encode(&command).map_err(|error| error.to_string())?;
    pending.insert(id, pending_kind);
    writer
        .send(Message::Bytes(bytes))
        .await
        .map_err(websocket_error)
}

async fn send_subscription(
    principal: &Principal,
    data: &Rc<RefCell<SurfaceData>>,
    writer: &mut SocketWriter,
) -> Result<(), String> {
    let (subjects, after, pending_thread) = {
        let data = data.borrow();
        let runtime = data
            .as_ref()
            .ok_or_else(|| "canvas runtime is absent".to_owned())?;
        (
            runtime.subjects(),
            runtime.resume(),
            runtime.pending_thread(),
        )
    };
    send_stream_request(
        writer,
        &StreamRequest::Subscribe {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            subjects,
            after,
        },
    )
    .await?;
    if let Some(cursor) = pending_thread {
        send_stream_request(
            writer,
            &StreamRequest::Thread {
                v: WireVersion::CURRENT,
                id: random_uuid()?,
                epoch: principal.epoch(),
                cursor,
            },
        )
        .await?;
    }
    Ok(())
}

async fn send_stream_directive(
    principal: &Principal,
    writer: &mut SocketWriter,
    directive: StreamDirective,
) -> Result<bool, String> {
    let request = match directive {
        StreamDirective::Continue { subject, cursor } => StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            subject,
            cursor,
        },
        StreamDirective::Thread { cursor } => StreamRequest::Thread {
            v: WireVersion::CURRENT,
            id: random_uuid()?,
            epoch: principal.epoch(),
            cursor,
        },
        StreamDirective::Reconnect => return Ok(true),
    };
    send_stream_request(writer, &request).await?;
    Ok(false)
}

async fn send_stream_request(
    writer: &mut SocketWriter,
    request: &StreamRequest,
) -> Result<(), String> {
    writer
        .send(Message::Bytes(
            wire::encode(request).map_err(|error| error.to_string())?,
        ))
        .await
        .map_err(websocket_error)
}

fn handle_incoming(
    session: &Session,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    bytes: &[u8],
) -> Result<(Vec<StreamDirective>, bool), String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        let admitted = session
            .admit_stream(bytes)
            .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
        let refresh = stale_reads(&admitted).any();
        let directives = data
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?
            .apply_stream(&admitted);
        surface.set(data.borrow().clone());
        return Ok((directives, refresh));
    }

    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let mut data = data.borrow_mut();
    let runtime = data
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    let refresh = match kind {
        PendingKind::Board => {
            runtime.apply_board(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Lanes => {
            runtime.apply_lanes(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Inbox => {
            runtime.apply_inbox(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Consent => {
            runtime.apply_consent(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Policy => {
            runtime.apply_policy(&admit_response(session, bytes)?);
            false
        }
        PendingKind::LaneSeat => {
            let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_lane_change(&admitted);
            true
        }
        PendingKind::Steer(lane) => {
            let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_steer(&admitted, lane);
            true
        }
        PendingKind::Note(task) => {
            let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            runtime.apply_note(&admitted, &task.to_string());
            false
        }
        PendingKind::ConsentDecision => {
            let admitted: crate::Admitted<Response<wire::intent::ConsentAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_decision(&admitted);
            false
        }
    };
    surface.set(data.clone());
    Ok((Vec::new(), refresh))
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

fn send_action(sender: &SenderHandle, action: ClientAction) {
    if let Some(sender) = sender.borrow_mut().as_mut() {
        let _ = sender.unbounded_send(action);
    }
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

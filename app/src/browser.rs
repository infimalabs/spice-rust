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
    Axis, CanvasRuntime, ChainEnd, LaneStatus, Principal, Session, Stale, SteerPane,
    StreamDirective, stale_reads,
};

mod view;
mod watchband;

use view::render_canvas;

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
    /// A new lane the operator asked the console to seat.
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
    Note(TaskHandle),
    ConsentDecision,
    Steer(CanonicalUuid),
}

#[derive(Deserialize)]
struct ResponseIdentity {
    id: CanonicalUuid,
}

#[function_component(CanvasApp)]
fn canvas_app() -> Html {
    let connection = use_state(|| Connection::Starting);
    let surface = use_state(SurfaceData::default);
    // The mosaic opens first because it is the surface the operator works from;
    // the axes are what they consult when a read is what they wanted.
    let destination = use_state(|| view::Destination::Mosaic);
    let note_task = use_state(String::new);
    let note_text = use_state(String::new);
    let focus = use_state(|| None::<CanonicalUuid>);
    let compose = use_state(String::new);
    let sender = use_mut_ref(|| None::<UnboundedSender<ClientAction>>);
    let agents = use_agent_controls(sender.clone(), surface.clone());
    let speech = use_state(|| wire::SpeechPlayback::Disabled {});
    let speech_status = use_state(SpeechStatus::default);
    let speech_controller = use_mut_ref(crate::speech::Controller::default);

    use_socket(
        connection.clone(),
        surface.clone(),
        speech.clone(),
        sender.clone(),
    );
    use_speech_reset(
        &speech,
        speech_status.clone(),
        Rc::clone(&speech_controller),
    );

    let refresh = {
        let sender = sender.clone();
        Callback::from(move |_| send_action(&sender, ClientAction::Refresh))
    };
    let submit_note = note_submit(sender.clone(), note_task.clone(), note_text.clone());
    let submit_steer = tile_steer_submit(
        sender.clone(),
        surface.clone(),
        focus.clone(),
        compose.clone(),
    );
    let toggle_speech = speech_toggle(
        speech.clone(),
        speech_status.clone(),
        speech_controller.clone(),
    );

    render_canvas(
        &connection,
        &surface,
        &destination,
        &sender,
        &view::Chrome {
            notes: view::Notes {
                task: note_task,
                text: note_text,
                submit: submit_note,
            },
            topbar: view::Topbar {
                speech,
                status: speech_status,
                toggle: toggle_speech,
                refresh,
            },
            steering: watchband::Steering {
                focus,
                compose,
                submit: submit_steer,
            },
            agents,
        },
    )
}

/// Own the socket task for as long as the component is mounted.
///
/// The sender is published into the shared cell only while the task is alive,
/// so a callback that fires after teardown finds no sender and drops its
/// action rather than sending into a closed channel.
#[hook]
fn use_socket(
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    speech: UseStateHandle<wire::SpeechPlayback>,
    sender: SenderHandle,
) {
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

/// Stop playback whenever the resident's speech policy changes.
#[hook]
fn use_speech_reset(
    speech: &UseStateHandle<wire::SpeechPlayback>,
    status: UseStateHandle<SpeechStatus>,
    controller: Rc<RefCell<crate::speech::Controller>>,
) {
    use_effect_with((**speech).clone(), move |_| {
        controller.borrow_mut().stop();
        status.set(SpeechStatus::Idle);
    });
}

/// The note composer's submit: a handle, a body, and the task they name.
fn note_submit(
    sender: SenderHandle,
    task: UseStateHandle<String>,
    text: UseStateHandle<String>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Ok(handle) = task.parse::<TaskHandle>() else {
            return;
        };
        let note = text.trim().to_owned();
        if note.is_empty() {
            return;
        }
        send_action(&sender, ClientAction::Note { task: handle, note });
        text.set(String::new());
    })
}

/// The watchband compose bar's submit, aimed at the focused tile.
fn tile_steer_submit(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    focus: UseStateHandle<Option<CanonicalUuid>>,
    compose: UseStateHandle<String>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(lane) = *focus else {
            return;
        };
        let message = compose.trim().to_owned();
        if message.is_empty() {
            return;
        }
        // The witness is read at send time rather than at draw time: the
        // roster can reseat between the click and the submit, and a stale
        // occupant would be refused by the resident instead of retargeted.
        let Some((occupant, epoch)) = watchband::residency(&surface, lane) else {
            return;
        };
        send_action(
            &sender,
            ClientAction::Steer {
                lane,
                occupant,
                epoch,
                message,
            },
        );
        compose.set(String::new());
    })
}

#[hook]
fn use_agent_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
) -> view::AgentControls {
    let selected_lane = use_state(String::new);
    let seat_worktree = use_state(String::new);
    let seat_vendor = use_state(|| "codex".to_owned());
    let seat_life = use_state(|| "steer".to_owned());
    let steer_text = use_state(String::new);
    let seat_notice = use_state(Notice::none);
    let steer_notice = use_state(Notice::none);
    view::AgentControls {
        submit_seat: seat_submit(
            sender.clone(),
            seat_worktree.clone(),
            seat_vendor.clone(),
            seat_life.clone(),
            seat_notice.clone(),
        ),
        submit_steer: steer_submit(
            sender,
            surface,
            selected_lane.clone(),
            steer_text.clone(),
            steer_notice.clone(),
        ),
        selected_lane,
        seat_worktree,
        seat_vendor,
        seat_life,
        steer_text,
        seat_notice,
        steer_notice,
    }
}

fn seat_submit(
    sender: SenderHandle,
    worktree: UseStateHandle<String>,
    vendor: UseStateHandle<String>,
    life: UseStateHandle<String>,
    notice: UseStateHandle<Notice>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        match compose::seat(&worktree, &vendor, &life) {
            Ok(seat) => {
                notice.set(Notice::none());
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

fn steer_submit(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    selected_lane: UseStateHandle<String>,
    message: UseStateHandle<String>,
    notice: UseStateHandle<Notice>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let selected = selected_lane.parse::<CanonicalUuid>().ok();
        // The console sends through the same door as a lane tile, and reads its
        // residency witness the same way: from the inbox, at send time. A
        // console that built the witness from its own epoch would keep sending
        // after the lane reseated, and the resident would refuse each one. An
        // unwitnessed lane is the same refusal as an unselected one -- there is
        // nothing to send to -- so it arrives at the operator as one notice
        // rather than as a submit that silently does nothing.
        let seated = console_lane(&surface, selected).and_then(|lane| {
            watchband::residency(&surface, lane).map(|(occupant, epoch)| (lane, occupant, epoch))
        });
        match compose::steer(&message, seated) {
            Ok(((lane, occupant, epoch), text)) => {
                notice.set(Notice::none());
                send_action(
                    &sender,
                    ClientAction::Steer {
                        lane,
                        occupant,
                        epoch,
                        message: text,
                    },
                );
                message.set(String::new());
            }
            Err(refusal) => notice.set(refusal.into()),
        }
    })
}

/// The lane the console is aimed at: the selected one, else the first seated.
///
/// A lane with no occupant is not a candidate, because the resident refuses
/// `steer::lane-vacant` for one.
fn console_lane(surface: &SurfaceData, selected: Option<CanonicalUuid>) -> Option<CanonicalUuid> {
    surface
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .and_then(|lanes| {
            selected
                .and_then(|selected| lanes.lanes.iter().find(|row| row.lane == selected))
                .or_else(|| lanes.lanes.first())
        })
        .filter(|row| row.occupant.is_some())
        .map(|row| row.lane)
}

fn speech_toggle(
    speech: UseStateHandle<wire::SpeechPlayback>,
    speech_status: UseStateHandle<SpeechStatus>,
    speech_controller: Rc<RefCell<crate::speech::Controller>>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        if speech_status.active() {
            speech_controller.borrow_mut().stop();
            speech_status.set(SpeechStatus::Idle);
            return;
        }
        let text = match crate::speech::rendered_text() {
            Ok(text) => text,
            Err(error) => {
                speech_status.set(SpeechStatus::Failed(error));
                return;
            }
        };
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
                let (directives, stale) =
                    handle_incoming(&session, surface, data, &mut pending, &bytes)?;
                relay_directives(&principal, &mut writer, directives).await?;
                send_stale(&principal, &mut writer, &mut pending, stale).await?;
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
            let body = LaneSeat {
                lane: random_uuid()?,
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
        } => {
            let body = SteerSend {
                lane,
                message,
                attachments: Vec::new(),
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
) -> Result<(Vec<StreamDirective>, Stale), String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        let admitted = session
            .admit_stream(bytes)
            .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
        let stale = stale_reads(&admitted);
        let directives = data
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?
            .apply_stream(&admitted);
        surface.set(data.borrow().clone());
        return Ok((directives, stale));
    }

    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let mut data = data.borrow_mut();
    let runtime = data
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    // A reply carries a receipt, not a snapshot, and it never widens the stale
    // set: the resident appended a fact for whatever this command changed, and
    // that fact reaches this socket on its own subscription, where
    // `stale_reads` decides what to re-read. Refreshing here as well would put
    // a second round of reads on the wire for the same event.
    match kind {
        PendingKind::Board => runtime.apply_board(&admit_response(session, bytes)?),
        PendingKind::Lanes => runtime.apply_lanes(&admit_response(session, bytes)?),
        PendingKind::Inbox => runtime.apply_inbox(&admit_response(session, bytes)?),
        PendingKind::Consent => runtime.apply_consent(&admit_response(session, bytes)?),
        PendingKind::Policy => runtime.apply_policy(&admit_response(session, bytes)?),
        PendingKind::LaneSeat => {
            let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_lane_change(&admitted);
        }
        PendingKind::Note(task) => {
            let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            runtime.apply_note(&admitted, &task.to_string());
        }
        PendingKind::ConsentDecision => {
            let admitted: crate::Admitted<Response<wire::intent::ConsentAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_decision(&admitted);
        }
        PendingKind::Steer(lane) => {
            let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_steer(&admitted, lane);
        }
    }
    surface.set(data.clone());
    Ok((Vec::new(), Stale::default()))
}

/// Re-read what a stream advance invalidated.
///
/// A read already in flight is not reissued. Facts arrive in bursts — one
/// command can append several — and without this a burst would put one
/// redundant read on the socket per fact, all of them answering with the same
/// snapshot.
async fn send_stale(
    principal: &Principal,
    writer: &mut SocketWriter,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    stale: Stale,
) -> Result<(), String> {
    if !stale.any() {
        return Ok(());
    }
    if stale.board() && !awaiting(pending, |kind| matches!(kind, PendingKind::Board)) {
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
    }
    if stale.lanes() && !awaiting(pending, |kind| matches!(kind, PendingKind::Lanes)) {
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
    }
    if stale.inbox() && !awaiting(pending, |kind| matches!(kind, PendingKind::Inbox)) {
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
    }
    if stale.consent() && !awaiting(pending, |kind| matches!(kind, PendingKind::Consent)) {
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
    }
    Ok(())
}

/// Whether a read of this kind is already outstanding.
fn awaiting(
    pending: &BTreeMap<CanonicalUuid, PendingKind>,
    want: impl Fn(&PendingKind) -> bool,
) -> bool {
    pending.values().any(want)
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
    yew::Renderer::<CanvasApp>::with_root(
        web_sys::window()
            .expect("browser window")
            .document()
            .expect("browser document")
            .get_element_by_id("app")
            .expect("canvas app root"),
    )
    .render();
}

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
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use wire::intent::{
    BoardAnswer, BoardRead, ConsentPolicy, ConsentPolicyAnswer, ConsentQueueAnswer, ConsentRead,
    InboxAnswer, LaneAnswer, LaneRead, SteerInbox, TaskAnswer, TaskNote,
};
use wire::{Answer, CanonicalUuid, CommandKind, Outcome, Response, TaskHandle, Witness};
use yew::prelude::*;

use crate::{Axis, Principal, Session, SteerPane};

type SenderHandle = Rc<RefCell<Option<UnboundedSender<ClientAction>>>>;

#[derive(Clone, PartialEq)]
enum Connection {
    Starting,
    Connecting,
    Online { actor: String, epoch: String },
    Offline(String),
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

#[derive(Clone, PartialEq)]
struct EventRow {
    class: &'static str,
    title: String,
    detail: String,
}

#[derive(Clone, Default, PartialEq)]
struct SurfaceData {
    board: Option<BoardAnswer>,
    lanes: Option<LaneAnswer>,
    inbox: Option<InboxAnswer>,
    consent: Option<ConsentQueueAnswer>,
    policy: Option<ConsentPolicyAnswer>,
    events: Vec<EventRow>,
}

impl SurfaceData {
    fn push(&mut self, row: EventRow) {
        self.events.insert(0, row);
        self.events.truncate(12);
    }
}

#[derive(Clone)]
enum ClientAction {
    Refresh,
    Note {
        task: TaskHandle,
        note: String,
    },
    Consent {
        request: CanonicalUuid,
        rule: wire::Name,
        answer: Answer,
    },
}

#[derive(Clone, Copy)]
enum PendingKind {
    Board,
    Lanes,
    Inbox,
    Consent,
    Policy,
    Note,
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
    let axis = use_state(|| Axis::Flow);
    let note_task = use_state(String::new);
    let note_text = use_state(String::new);
    let sender = use_mut_ref(|| None::<UnboundedSender<ClientAction>>);

    {
        let connection = connection.clone();
        let surface = surface.clone();
        let sender = sender.clone();
        use_effect_with((), move |()| {
            let active = Rc::new(Cell::new(true));
            let (tx, rx) = mpsc::unbounded();
            *sender.borrow_mut() = Some(tx);
            spawn_local(run_surface(active.clone(), connection, surface, rx));
            move || {
                active.set(false);
                sender.borrow_mut().take();
            }
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

    render_canvas(
        &connection,
        &surface,
        &axis,
        &sender,
        &note_task,
        &note_text,
        submit_note,
        refresh,
    )
}

fn render_canvas(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    axis: &UseStateHandle<Axis>,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
    refresh: Callback<MouseEvent>,
) -> Html {
    let active_axis = **axis;
    let cursor = surface
        .board
        .as_ref()
        .map(|board| board.cursor.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <div class="shell">
            {render_rail(axis, active_axis, &cursor)}
            {render_workspace(
                connection,
                surface,
                active_axis,
                sender,
                note_task,
                note_text,
                submit_note,
                refresh,
            )}
        </div>
    }
}

fn render_rail(axis: &UseStateHandle<Axis>, active_axis: Axis, cursor: &str) -> Html {
    html! {
        <aside class="rail">
            <div class="brand">
                <span class="brand-mark">{"SP"}</span>
                <span class="brand-name">{"Spice Canvas"}</span>
            </div>
            <nav class="axis-nav" aria-label="Canvas axes">
                {for Axis::ALL.iter().copied().map(|candidate| {
                    let axis = axis.clone();
                    let selected = candidate == active_axis;
                    html! {
                        <button
                            class="axis-button"
                            aria-current={selected.then_some("page")}
                            title={candidate.question()}
                            onclick={Callback::from(move |_| axis.set(candidate))}
                        >
                            <span class="axis-glyph"></span>
                            <span class="axis-label">{title(candidate.as_str())}</span>
                        </button>
                    }
                })}
            </nav>
            <div class="rail-meta">{format!("cursor {cursor}")}</div>
        </aside>
    }
}

fn render_workspace(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    active_axis: Axis,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
    refresh: Callback<MouseEvent>,
) -> Html {
    let task_count = surface.board.as_ref().map_or(0, |board| board.tasks.len());
    let active_tasks = surface.board.as_ref().map_or(0, |board| {
        board
            .tasks
            .iter()
            .filter(|task| task.workflow.body.status != "completed")
            .count()
    });
    let lane_count = surface.lanes.as_ref().map_or(0, |lanes| lanes.lanes.len());
    let pending_count = surface
        .consent
        .as_ref()
        .map_or(0, |consent| consent.pending.len());
    let cursor = surface
        .board
        .as_ref()
        .map(|board| board.cursor.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <div class="workspace">
            <header class="topbar">
                <div class="mobile-brand">{"Spice Canvas"}</div>
                <div class={connection.class()}>
                    <span class="status-dot"></span>
                    <span class="status-copy">{connection.label()}</span>
                </div>
                <button class="refresh" type="button" onclick={refresh}>{"Refresh"}</button>
            </header>
            <main class="content">
                <div class="page-heading">
                    <div>
                        <h1>{title(active_axis.as_str())}</h1>
                        <p>{active_axis.question()}</p>
                    </div>
                </div>
                <section class="metrics" aria-label="Current resident metrics">
                    {metric("Tasks", task_count.to_string(), format!("{active_tasks} active"))}
                    {metric("Lanes", lane_count.to_string(), resident_state(surface))}
                    {metric(
                        "Consent",
                        pending_count.to_string(),
                        "pending decisions".to_owned(),
                    )}
                    {metric("Journal", cursor, "snapshot cursor".to_owned())}
                </section>
                <div class="pane-strip" aria-label="Panes on this axis">
                    {for active_axis.panes().map(|pane| html! {
                        <span class="pane-chip">{title(pane.as_str())}</span>
                    })}
                </div>
                {render_axis(
                    active_axis,
                    surface,
                    sender,
                    note_task,
                    note_text,
                    submit_note,
                )}
            </main>
        </div>
    }
}

fn render_axis(
    axis: Axis,
    surface: &SurfaceData,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    match axis {
        Axis::Magnitude => render_magnitude(surface),
        Axis::Flow => render_flow(surface, note_task, note_text, submit_note),
        Axis::Topology => render_topology(surface),
        Axis::Chronology => render_chronology(surface, sender),
    }
}

fn render_magnitude(surface: &SurfaceData) -> Html {
    let Some(consent) = &surface.consent else {
        return empty("No magnitude snapshot is available.");
    };
    html! {
        <section class="section">
            <div class="section-heading">
                <h2>{"Consent budgets"}</h2>
                <span class="section-count">{format!("{} rules", consent.budgets.len())}</span>
            </div>
            if consent.budgets.is_empty() {
                {empty("No bounded consent rules are active.")}
            } else {
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Rule"}</th><th>{"Remaining"}</th><th>{"Bound"}</th>
                    </tr></thead>
                    <tbody>{for consent.budgets.iter().map(|budget| html! {
                        <tr>
                            <td class="mono">{budget.rule.to_string()}</td>
                            <td>{budget.remaining}</td>
                            <td>{budget.bound}</td>
                        </tr>
                    })}</tbody>
                </table></div>
            }
        </section>
    }
}

fn render_flow(
    surface: &SurfaceData,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    let Some(board) = &surface.board else {
        return empty("No board snapshot is available.");
    };
    let selected = (**note_task).clone();
    let note = (**note_text).clone();
    html! {
        <div class="split">
            <section class="section">
                <div class="section-heading">
                    <h2>{"Current work"}</h2>
                    <span class="section-count">{format!("{} tasks", board.tasks.len())}</span>
                </div>
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Task"}</th><th>{"Phase"}</th><th>{"Owner"}</th><th>{"Project"}</th>
                    </tr></thead>
                    <tbody>{for board.tasks.iter().map(|task| {
                        let statement = &task.statement.body;
                        let workflow = &task.workflow.body;
                        html! {
                            <tr>
                                <td class="title-cell">
                                    <div class="mono muted">{statement.handle.to_string()}</div>
                                    <div>{statement.title.clone()}</div>
                                </td>
                                <td><span class={phase_class(&workflow.status)}>
                                    {workflow.phase.to_string()}
                                </span></td>
                                <td class="mono">{
                                    workflow
                                        .assignment
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| "-".to_owned())
                                }</td>
                                <td>{statement.project.clone()}</td>
                            </tr>
                        }
                    })}</tbody>
                </table></div>
            </section>
            <section class="section">
                <div class="section-heading"><h2>{"Append observation"}</h2></div>
                <form class="form" onsubmit={submit_note}>
                    <label for="note-task">{"Task"}</label>
                    <input
                        id="note-task"
                        list="note-tasks"
                        value={selected}
                        placeholder="SPICE-aBcD"
                        oninput={state_from_input(note_task.clone())}
                    />
                    <datalist id="note-tasks">
                        {for board.tasks.iter().map(|task| {
                            let task = &task.statement.body;
                            html! {
                                <option value={task.handle.to_string()}>
                                    {task.title.clone()}
                                </option>
                            }
                        })}
                    </datalist>
                    <label for="note-body">{"Observation"}</label>
                    <textarea
                        id="note-body"
                        value={note}
                        oninput={state_from_textarea(note_text.clone())}
                    ></textarea>
                    <button
                        type="submit"
                        disabled={note_task.is_empty() || note_text.trim().is_empty()}
                    >{"Add note"}</button>
                </form>
            </section>
        </div>
    }
}

fn render_topology(surface: &SurfaceData) -> Html {
    let Some(lanes) = &surface.lanes else {
        return empty("No lane snapshot is available.");
    };
    html! {
        <section class="section">
            <div class="section-heading">
                <h2>{"Resident topology"}</h2>
                <span class="section-count">{format!("{} lanes", lanes.lanes.len())}</span>
            </div>
            if lanes.lanes.is_empty() {
                {empty("No lanes are seated.")}
            } else {
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Lane"}</th><th>{"Standing"}</th><th>{"Occupant"}</th>
                        <th>{"Worktree"}</th><th>{"Vendor"}</th>
                    </tr></thead>
                    <tbody>{for lanes.lanes.iter().map(|lane| html! {
                        <tr>
                            <td class="mono">{short(&lane.lane.to_string())}</td>
                            <td><span class="tag active">{lane.disposition.clone()}</span></td>
                            <td class="mono">{
                                lane
                                    .occupant
                                    .map(|value| short(&value.to_string()))
                                    .unwrap_or_else(|| "vacant".to_owned())
                            }</td>
                            <td class="mono">{lane.worktree.clone()}</td>
                            <td>{lane.vendor.to_string()}</td>
                        </tr>
                    })}</tbody>
                </table></div>
            }
        </section>
    }
}

fn render_chronology(surface: &SurfaceData, sender: &SenderHandle) -> Html {
    let pending = surface
        .consent
        .as_ref()
        .map(|value| value.pending.as_slice())
        .unwrap_or(&[]);
    let directives = surface
        .inbox
        .as_ref()
        .map(|inbox| {
            inbox
                .lanes
                .iter()
                .flat_map(|lane| lane.pending.iter())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    html! {
        <div class="split">
            <div>
                <section class="section">
                    <div class="section-heading">
                        <h2>{"Pending consent"}</h2>
                        <span class="section-count">{pending.len()}</span>
                    </div>
                    if pending.is_empty() {
                        {empty("No consent decisions are pending.")}
                    } else {
                        <div class="table-wrap"><table>
                            <thead><tr>
                                <th>{"Operation"}</th><th>{"Rule"}</th>
                                <th>{"Lane"}</th><th>{"Decision"}</th>
                            </tr></thead>
                            <tbody>{for pending.iter().map(|request| {
                                let allow = ClientAction::Consent {
                                    request: request.request,
                                    rule: request.rule.clone(),
                                    answer: Answer::Granted,
                                };
                                let deny = ClientAction::Consent {
                                    request: request.request,
                                    rule: request.rule.clone(),
                                    answer: Answer::Denied,
                                };
                                let allow_sender = sender.clone();
                                let deny_sender = sender.clone();
                                html! {
                                    <tr>
                                        <td class="title-cell">{request.operation.clone()}</td>
                                        <td class="mono">{request.rule.to_string()}</td>
                                        <td class="mono">{short(&request.lane.to_string())}</td>
                                        <td><div class="decision-row">
                                            <button
                                                type="button"
                                                onclick={Callback::from(move |_| {
                                                    send_action(&allow_sender, allow.clone());
                                                })}
                                            >{"Allow"}</button>
                                            <button
                                                class="deny"
                                                type="button"
                                                onclick={Callback::from(move |_| {
                                                    send_action(&deny_sender, deny.clone());
                                                })}
                                            >{"Deny"}</button>
                                        </div></td>
                                    </tr>
                                }
                            })}</tbody>
                        </table></div>
                    }
                </section>
                <section class="section">
                    <div class="section-heading">
                        <h2>{"Pending direction"}</h2>
                        <span class="section-count">{directives.len()}</span>
                    </div>
                    if directives.is_empty() {
                        {empty("No directives are pending.")}
                    } else {
                        <div class="event-list">{for directives.iter().map(|directive| html! {
                            <article class="event">
                                <strong>
                                    {format!("{} / {}", directive.key, directive.expires)}
                                </strong>
                                <p>{directive.text.clone()}</p>
                            </article>
                        })}</div>
                    }
                </section>
            </div>
            <section class="section">
                <div class="section-heading"><h2>{"Command outcomes"}</h2></div>
                if surface.events.is_empty() {
                    {empty("No command outcomes have been recorded.")}
                } else {
                    <div class="event-list">{for surface.events.iter().map(|event| html! {
                        <article class={classes!("event", event.class)}>
                            <strong>{event.title.clone()}</strong>
                            <p>{event.detail.clone()}</p>
                        </article>
                    })}</div>
                }
            </section>
        </div>
    }
}

async fn run_surface(
    active: Rc<Cell<bool>>,
    connection: UseStateHandle<Connection>,
    surface: UseStateHandle<SurfaceData>,
    mut actions: UnboundedReceiver<ClientAction>,
) {
    let data = Rc::new(RefCell::new(SurfaceData::default()));
    while active.get() {
        connection.set(Connection::Connecting);
        let result = run_connection(&active, &connection, &surface, &data, &mut actions).await;
        if !active.get() {
            break;
        }
        let reason = result.unwrap_err_or_else();
        connection.set(Connection::Offline(reason.clone()));
        data.borrow_mut().push(EventRow {
            class: "fault",
            title: "Canvas connection reset".to_owned(),
            detail: reason,
        });
        surface.set(data.borrow().clone());
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
    data: &Rc<RefCell<SurfaceData>>,
    actions: &mut UnboundedReceiver<ClientAction>,
) -> Result<(), String> {
    let discovery: wire::CanvasBootstrap = Request::get("/bootstrap")
        .send()
        .await
        .map_err(|error| format!("bootstrap request failed: {error}"))?
        .json()
        .await
        .map_err(|error| format!("bootstrap response was invalid: {error}"))?;
    let session = Session::open(discovery.capability, discovery.epoch);
    let endpoint = discovery
        .endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "bootstrap endpoint was not loopback HTTP".to_owned())?;
    let socket = WebSocket::open(&format!("ws://{endpoint}/socket"))
        .map_err(|error| format!("socket did not open: {error}"))?;
    let (mut writer, mut reader) = socket.split();
    writer
        .send(Message::Bytes(
            wire::encode(&session.hello()).map_err(|error| error.to_string())?,
        ))
        .await
        .map_err(websocket_error)?;
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
    let principal = Principal::admitted(&accepted);
    *data.borrow_mut() = SurfaceData::default();
    surface.set(data.borrow().clone());
    connection.set(Connection::Online {
        actor: principal.actor().to_string(),
        epoch: principal.epoch().to_string(),
    });
    let mut pending = BTreeMap::new();
    send_refresh(&principal, &mut writer, &mut pending).await?;

    while active.get() {
        let incoming = reader.next().fuse();
        let action = actions.next().fuse();
        let refresh = TimeoutFuture::new(2_000).fuse();
        futures_util::pin_mut!(incoming, action, refresh);
        select! {
            frame = incoming => {
                let frame = frame
                    .ok_or_else(|| "resident closed the socket".to_owned())?
                    .map_err(websocket_error)?;
                let bytes = binary_message(frame)?;
                handle_response(&session, surface, data, &mut pending, &bytes)?;
            },
            action = action => {
                let action = action.ok_or_else(|| "canvas command channel closed".to_owned())?;
                match action {
                    ClientAction::Refresh => {
                        send_refresh(&principal, &mut writer, &mut pending).await?
                    }
                    ClientAction::Note { task, note } => {
                        let body = TaskNote { task: task.clone(), note };
                        let expected = vec![Witness::Existence { task }];
                        send_typed(
                            &principal,
                            &mut writer,
                            &mut pending,
                            PendingKind::Note,
                            CommandKind::TaskNote,
                            expected,
                            body,
                        ).await?;
                    }
                    ClientAction::Consent { request, rule, answer } => {
                        let body = wire::intent::ConsentDecide { request, answer };
                        let expected = vec![
                            Witness::Consent { request, answer: Answer::Asked },
                            Witness::Rule { name: rule },
                        ];
                        send_typed(
                            &principal,
                            &mut writer,
                            &mut pending,
                            PendingKind::ConsentDecision,
                            CommandKind::ConsentDecide,
                            expected,
                            body,
                        ).await?;
                    }
                }
            },
            () = refresh => {
                send_refresh(&principal, &mut writer, &mut pending).await?;
            }
        }
    }
    Ok(())
}

async fn send_refresh(
    principal: &Principal,
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
) -> Result<(), String> {
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::Board,
        CommandKind::BoardRead,
        vec![],
        BoardRead { family: None },
    )
    .await?;
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::Lanes,
        CommandKind::LaneRead,
        vec![],
        LaneRead {},
    )
    .await?;
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::Inbox,
        CommandKind::SteerInbox,
        vec![],
        SteerInbox { lane: None },
    )
    .await?;
    send_typed(
        principal,
        writer,
        pending,
        PendingKind::Consent,
        CommandKind::ConsentRead,
        vec![],
        ConsentRead {},
    )
    .await?;
    send_typed(
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

async fn send_typed<B: Serialize>(
    principal: &Principal,
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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

fn handle_response(
    session: &Session,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    bytes: &[u8],
) -> Result<(), String> {
    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    match kind {
        PendingKind::Board => apply(session, surface, data, bytes, |data, body: BoardAnswer| {
            data.board = Some(body)
        }),
        PendingKind::Lanes => apply(session, surface, data, bytes, |data, body: LaneAnswer| {
            data.lanes = Some(body)
        }),
        PendingKind::Inbox => apply(session, surface, data, bytes, |data, body: InboxAnswer| {
            data.inbox = Some(body)
        }),
        PendingKind::Consent => apply(
            session,
            surface,
            data,
            bytes,
            |data, body: ConsentQueueAnswer| data.consent = Some(body),
        ),
        PendingKind::Policy => apply(
            session,
            surface,
            data,
            bytes,
            |data, body: ConsentPolicyAnswer| data.policy = Some(body),
        ),
        PendingKind::Note => apply(session, surface, data, bytes, |data, body: TaskAnswer| {
            data.push(EventRow {
                class: "reply",
                title: format!("Note appended to {}", body.task),
                detail: body.status,
            });
        }),
        PendingKind::ConsentDecision => apply(
            session,
            surface,
            data,
            bytes,
            |data, body: wire::intent::ConsentAnswer| {
                data.push(EventRow {
                    class: "reply",
                    title: format!("Consent {:?}", body.answer).to_lowercase(),
                    detail: body.request.to_string(),
                });
            },
        ),
    }
}

fn apply<T: for<'de> Deserialize<'de>>(
    session: &Session,
    surface: &UseStateHandle<SurfaceData>,
    data: &Rc<RefCell<SurfaceData>>,
    bytes: &[u8],
    reply: impl FnOnce(&mut SurfaceData, T),
) -> Result<(), String> {
    let response: Response<T> = session
        .admit_frame(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?
        .into_inner();
    let mut data = data.borrow_mut();
    match response.outcome {
        Outcome::Reply(answer) => reply(&mut data, answer.body),
        Outcome::Refusal(refusal) => data.push(EventRow {
            class: "refusal",
            title: refusal.reason,
            detail: refusal.remedy,
        }),
        Outcome::Fault(fault) => data.push(EventRow {
            class: "fault",
            title: fault.reason,
            detail: fault.detail,
        }),
    }
    surface.set(data.clone());
    Ok(())
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
        .lanes
        .as_ref()
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

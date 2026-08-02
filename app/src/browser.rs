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
    BoardRead, ConsentPolicy, ConsentRead, LaneRead, SteerInbox, TaskAnswer, TaskNote,
};
use wire::{
    Answer, CanonicalUuid, CommandKind, Response, StreamRequest, TaskHandle, WireVersion, Witness,
};
use yew::prelude::*;

use crate::{Axis, CanvasRuntime, ChainEnd, Principal, Session, SteerPane, StreamDirective};

type SenderHandle = Rc<RefCell<Option<UnboundedSender<ClientAction>>>>;

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

#[derive(Clone)]
enum PendingKind {
    Board,
    Lanes,
    Inbox,
    Consent,
    Policy,
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
    let axis = use_state(|| Axis::Flow);
    let note_task = use_state(String::new);
    let note_text = use_state(String::new);
    let sender = use_mut_ref(|| None::<UnboundedSender<ClientAction>>);
    let speech = use_state(|| wire::SpeechPlayback::Disabled {});
    let speech_status = use_state(SpeechStatus::default);
    let speech_controller = use_mut_ref(crate::speech::Controller::default);

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
        speech.clone(),
        speech_status.clone(),
        speech_controller.clone(),
    );

    render_canvas(
        &connection,
        &surface,
        &axis,
        &sender,
        &note_task,
        &note_text,
        submit_note,
        refresh,
        &speech,
        &speech_status,
        toggle_speech,
    )
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

fn render_canvas(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    axis: &UseStateHandle<Axis>,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
    refresh: Callback<MouseEvent>,
    speech: &UseStateHandle<wire::SpeechPlayback>,
    speech_status: &UseStateHandle<SpeechStatus>,
    toggle_speech: Callback<MouseEvent>,
) -> Html {
    let active_axis = **axis;
    let cursor = (**surface)
        .as_ref()
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
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
                speech,
                speech_status,
                toggle_speech,
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
    speech: &wire::SpeechPlayback,
    speech_status: &SpeechStatus,
    toggle_speech: Callback<MouseEvent>,
) -> Html {
    let runtime = (**surface).as_ref();
    let task_count = runtime
        .and_then(CanvasRuntime::board)
        .map_or(0, |board| board.tasks.len());
    let active_tasks = runtime.and_then(CanvasRuntime::board).map_or(0, |board| {
        board
            .tasks
            .iter()
            .filter(|task| task.workflow.body.status != "completed")
            .count()
    });
    let lane_count = runtime
        .and_then(CanvasRuntime::lanes)
        .map_or(0, |lanes| lanes.lanes.len());
    let pending_count = runtime
        .and_then(CanvasRuntime::consent)
        .map_or(0, |consent| consent.pending.len());
    let cursor = runtime
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <div class="workspace">
            <header class="topbar">
                <div class="mobile-brand">{"Spice Canvas"}</div>
                <div class={connection.class()}>
                    <span class="status-dot"></span>
                    <span class="status-copy">{connection.label()}</span>
                </div>
                <div class="topbar-actions">
                    {speech_control(speech, speech_status, toggle_speech)}
                    <button class="refresh" type="button" onclick={refresh}>{"Refresh"}</button>
                </div>
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
                    {metric("Journal", cursor, "admitted stream cursor".to_owned())}
                </section>
                <div class="pane-strip" aria-label="Panes on this axis">
                    {for active_axis.panes().map(|pane| html! {
                        <span class="pane-chip">{title(pane.as_str())}</span>
                    })}
                </div>
                {render_axis(
                    active_axis,
                    surface,
                    connection,
                    sender,
                    note_task,
                    note_text,
                    submit_note,
                )}
            </main>
        </div>
    }
}

fn speech_control(
    speech: &wire::SpeechPlayback,
    status: &SpeechStatus,
    toggle: Callback<MouseEvent>,
) -> Html {
    if matches!(speech, wire::SpeechPlayback::Disabled {}) {
        return Html::default();
    }
    let unavailable = matches!(
        speech,
        wire::SpeechPlayback::External {
            available: false,
            ..
        }
    );
    let label = if unavailable {
        "Speech unavailable"
    } else {
        status.label()
    };
    let title = if unavailable {
        "External speech backend is not configured".to_owned()
    } else {
        status.title()
    };
    html! {
        <>
            <button
                class={classes!("speech-button", status.active().then_some("stop"))}
                type="button"
                aria-label={label}
                title={title}
                data-state={status.state()}
                disabled={unavailable}
                onclick={toggle}
            >
                <span class="speech-icon" aria-hidden="true"></span>
            </button>
            <span class="visually-hidden" aria-live="polite">{status.title()}</span>
        </>
    }
}

fn render_axis(
    axis: Axis,
    surface: &SurfaceData,
    connection: &Connection,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    match axis {
        Axis::Magnitude => render_magnitude(surface),
        Axis::Flow => render_flow(surface, note_task, note_text, submit_note),
        Axis::Topology => render_topology(surface),
        Axis::Chronology => render_chronology(surface, sender, connection),
    }
}

fn render_magnitude(surface: &SurfaceData) -> Html {
    let Some(consent) = surface.as_ref().and_then(CanvasRuntime::consent) else {
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
    let Some(board) = surface.as_ref().and_then(CanvasRuntime::board) else {
        return empty("No board snapshot is available.");
    };
    let can_note = surface
        .as_ref()
        .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote));
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
            if can_note {
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
            }
        </div>
    }
}

fn render_topology(surface: &SurfaceData) -> Html {
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted topology is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No lane snapshot is available.");
    };
    html! {
        <>
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
            {render_causal_thread(runtime)}
        </>
    }
}

fn render_chronology(
    surface: &SurfaceData,
    sender: &SenderHandle,
    connection: &Connection,
) -> Html {
    let runtime = surface.as_ref();
    let can_decide = runtime
        .is_some_and(|runtime| runtime.grants() && runtime.offers(CommandKind::ConsentDecide));
    let pending = surface
        .as_ref()
        .and_then(CanvasRuntime::consent)
        .map(|value| value.pending.as_slice())
        .unwrap_or(&[]);
    let directives = surface
        .as_ref()
        .and_then(CanvasRuntime::inbox)
        .map(|inbox| {
            inbox
                .lanes
                .iter()
                .flat_map(|lane| lane.pending.iter())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let events = surface
        .iter()
        .flat_map(|runtime| runtime.events())
        .collect::<Vec<_>>();
    html! {
        <div class="split">
            <div>
                if can_decide {
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
                }
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
            <div>
                {runtime.map_or_else(Html::default, render_trust)}
                <section class="section">
                    <div class="section-heading"><h2>{"Command outcomes"}</h2></div>
                    if !matches!(connection, Connection::Offline(_)) && events.is_empty() {
                        {empty("No command outcomes have been recorded.")}
                    } else {
                        <div class="event-list">
                            if let Connection::Offline(reason) = connection {
                                <article class="event fault">
                                    <strong>{"Canvas connection reset"}</strong>
                                    <p>{reason}</p>
                                </article>
                            }
                            {for events.iter().map(|event| html! {
                                <article class={classes!("event", event.class())}>
                                    <strong>{event.title()}</strong>
                                    <p>{event.detail()}</p>
                                </article>
                            })}
                        </div>
                    }
                </section>
                {runtime.map_or_else(Html::default, render_fact_chronology)}
            </div>
        </div>
    }
}

fn render_causal_thread(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.thread_pages();
    let walk = runtime.thread_walk();
    let end = walk.as_ref().map(|walk| match walk.end() {
        ChainEnd::Root => "root".to_owned(),
        ChainEnd::Dangling(position) => format!("dangling at {position}"),
        ChainEnd::Cycle(position) => format!("cycle at {position}"),
    });
    let end_label = end.unwrap_or_else(|| "awaiting thread".to_owned());
    html! {
        <section class="section" data-pane="lineage">
            <div class="section-heading">
                <h2>{"Causal thread"}</h2>
                <span class="section-count">{end_label}</span>
            </div>
            if pages.is_empty() {
                {empty("No causal thread has been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "thread",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="table-wrap"><table>
                                <thead><tr>
                                    <th>{"Seq"}</th>
                                    <th>{"Kind"}</th>
                                    <th>{"Origin"}</th>
                                    <th>{"Cause"}</th>
                                </tr></thead>
                                <tbody>{for page.rows().iter().map(|row| {
                                    let cause = row.cause().map_or_else(
                                        || "root".to_owned(),
                                        |cause| cause.to_string(),
                                    );
                                    html! {
                                        <tr>
                                            <td class="mono">{row.seq().to_string()}</td>
                                            <td>{row.kind().to_string()}</td>
                                            <td class="mono">{row.origin().to_string()}</td>
                                            <td class="mono">{cause}</td>
                                        </tr>
                                    }
                                })}</tbody>
                            </table></div>
                        </div>
                    }
                })}
            }
        </section>
    }
}

fn render_trust(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.trust_pages();
    let history = runtime
        .fact_pages()
        .into_iter()
        .flat_map(|page| page.rows().to_vec())
        .filter(|row| {
            matches!(
                row.kind(),
                wire::FactKind::TrustPended
                    | wire::FactKind::TrustGranted
                    | wire::FactKind::TrustRevoked
            )
        })
        .collect::<Vec<_>>();
    if !runtime.grants() {
        return Html::default();
    }
    html! {
        <section class="section" data-pane="trust">
            <div class="section-heading">
                <h2>{"Trust"}</h2>
                <span class="section-count">{format!("{} transitions", history.len())}</span>
            </div>
            if pages.is_empty() {
                {empty("No trust heads have been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "trust",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="table-wrap"><table>
                                <thead><tr>
                                    <th>{"Digest"}</th>
                                    <th>{"Standing"}</th>
                                    <th>{"Position"}</th>
                                </tr></thead>
                                <tbody>{for page.rows().iter().map(|row| {
                                    let standing = format!("{:?}", row.head.state).to_lowercase();
                                    html! {
                                        <tr>
                                            <td class="mono">
                                                {short(&row.head.digest.to_string())}
                                            </td>
                                            <td><span class="tag active">{standing}</span></td>
                                            <td class="mono">{row.position.to_string()}</td>
                                        </tr>
                                    }
                                })}</tbody>
                            </table></div>
                        </div>
                    }
                })}
            }
            if !history.is_empty() {
                <div class="event-list trust-history">{for history.iter().rev().map(|row| html! {
                    <article class="event">
                        <strong>{row.kind().to_string()}</strong>
                        <p>{format!("{} / {} / {}", row.seq(), row.actor(), row.origin())}</p>
                    </article>
                })}</div>
            }
        </section>
    }
}

fn render_fact_chronology(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.fact_pages();
    html! {
        <section class="section" data-pane="commands">
            <div class="section-heading">
                <h2>{"Admitted journal"}</h2>
                <span class="section-count">{format!("{} packed pages", pages.len())}</span>
            </div>
            if pages.is_empty() {
                {empty("No journal page has been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "facts",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="event-list">
                                {for page.rows().iter().rev().map(|row| html! {
                                    <article class="event">
                                        <strong>
                                            {format!("{} / {}", row.seq(), row.kind())}
                                        </strong>
                                        <p>{format!("{} / {}", row.actor(), row.origin())}</p>
                                    </article>
                                })}
                            </div>
                        </div>
                    }
                })}
            }
        </section>
    }
}

fn packed_provenance(
    subject: &'static str,
    page: usize,
    rows: usize,
    overflow: Option<wire::Overflow>,
    horizon: Option<wire::Seq>,
) -> Html {
    let horizon = horizon.map_or_else(|| "-".to_owned(), |value| value.to_string());
    html! {
        <div class="packed-provenance" data-subject={subject}>
            <span>{format!("page {page} / {rows} rows")}</span>
            <span>{format!("horizon {horizon}")}</span>
            if let Some(overflow) = overflow {
                <span class="overflow-marker">
                    {format!("{} omitted / continue {}", overflow.omitted, overflow.continuation)}
                </span>
            }
        </div>
    }
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
    let discovery: wire::CanvasBootstrap = Request::get("/bootstrap")
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
                let directives = handle_incoming(&session, surface, data, &mut pending, &bytes)?;
                for directive in directives {
                    if send_stream_directive(&principal, &mut writer, directive).await? {
                        return Err("resident stream requested reconnect".to_owned());
                    }
                }
            },
            action = action => {
                let action = action.ok_or_else(|| "canvas command channel closed".to_owned())?;
                match action {
                    ClientAction::Refresh => {
                        send_refresh(&principal, &mut writer, &mut pending).await?
                    }
                    ClientAction::Note { task, note } => {
                        let body = TaskNote { task: task.clone(), note };
                        let expected = vec![Witness::Existence { task: task.clone() }];
                        send_typed(
                            &principal,
                            &mut writer,
                            &mut pending,
                            PendingKind::Note(task),
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
            }
        }
    }
    Ok(())
}

async fn request_external_speech(text: &str, content_type: &str) -> Result<Vec<u8>, String> {
    let request = Request::post("/speech")
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

fn bounded_speech_error(error: &str) -> String {
    let mut end = error.len().min(240);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_owned()
}

async fn send_refresh(
    principal: &Principal,
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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

async fn send_subscription(
    principal: &Principal,
    data: &Rc<RefCell<SurfaceData>>,
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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
    writer: &mut futures_util::stream::SplitSink<WebSocket, Message>,
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
) -> Result<Vec<StreamDirective>, String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        let admitted = session
            .admit_stream(bytes)
            .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
        let directives = data
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?
            .apply_stream(&admitted);
        surface.set(data.borrow().clone());
        return Ok(directives);
    }

    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let mut data = data.borrow_mut();
    let runtime = data
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    match kind {
        PendingKind::Board => runtime.apply_board(&admit_response(session, bytes)?),
        PendingKind::Lanes => runtime.apply_lanes(&admit_response(session, bytes)?),
        PendingKind::Inbox => runtime.apply_inbox(&admit_response(session, bytes)?),
        PendingKind::Consent => runtime.apply_consent(&admit_response(session, bytes)?),
        PendingKind::Policy => runtime.apply_policy(&admit_response(session, bytes)?),
        PendingKind::Note(task) => {
            let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            runtime.apply_note(&admitted, &task.to_string());
        }
        PendingKind::ConsentDecision => {
            let admitted: crate::Admitted<Response<wire::intent::ConsentAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_decision(&admitted);
        }
    }
    surface.set(data.clone());
    Ok(Vec::new())
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

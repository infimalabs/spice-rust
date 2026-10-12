//! Working-group catalog and browser-local task-room consoles.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use wasm_bindgen::JsValue;
use web_sys::{HtmlElement, HtmlTextAreaElement};
use wire::intent::WorkingGroupChangeStatus;
use wire::rooms::{RoomAuthorityStamp, RoomPostOutcome, WorkingGroup};
use wire::{CommandKind, TaskStamp};
use yew::prelude::*;

use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::{ClientAction, RoomPostAction};
use crate::room_views::{GroupView, RoomViewAction, RoomViews};
use crate::runtime::{RoomAnchorAuthority, RoomEventKind, RoomProjectionStatus};

const STORAGE: &str = "spice:room-views:v1";

thread_local! {
    static LAST_GROUP_MICROS: Cell<i64> = const { Cell::new(0) };
}

impl Reducible for RoomViews {
    type Action = RoomViewAction;

    fn reduce(self: Rc<Self>, action: RoomViewAction) -> Rc<Self> {
        let mut views = Rc::unwrap_or_clone(self);
        views.apply(action);
        Rc::new(views)
    }
}

#[derive(Clone)]
pub(super) struct Controls {
    views: UseReducerHandle<RoomViews>,
    notice: UseStateHandle<Option<String>>,
    /// Last authoritative name of each group seen in this page's memory, shown
    /// while room authority reloads so a reconnect does not retitle consoles.
    names: Rc<RefCell<BTreeMap<RoomAuthorityStamp, String>>>,
    /// The room an explicit open asked to focus, honored once by its composer.
    requested: Rc<RefCell<Option<(RoomAuthorityStamp, TaskStamp)>>>,
}

#[hook]
pub(super) fn use_controls() -> Controls {
    let views = use_reducer(restore);
    let names = use_mut_ref(BTreeMap::new);
    let requested = use_mut_ref(|| None);
    use_effect_with((*views).clone(), store);
    Controls {
        views,
        notice: use_state(|| None),
        names,
        requested,
    }
}

impl Controls {
    fn update(&self, action: RoomViewAction) {
        self.views.dispatch(action);
    }

    fn open(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        *self.requested.borrow_mut() = Some((group.clone(), room.clone()));
        self.update(RoomViewAction::Open { group, room });
    }

    fn close_room(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        self.update(RoomViewAction::CloseRoom { group, room });
    }

    fn close_group(&self, group: RoomAuthorityStamp) {
        self.update(RoomViewAction::CloseGroup { group });
    }

    fn move_group(&self, group: RoomAuthorityStamp, delta: isize) {
        self.update(RoomViewAction::MoveGroup { group, delta });
    }

    fn move_room(&self, group: RoomAuthorityStamp, room: TaskStamp, delta: isize) {
        self.update(RoomViewAction::MoveRoom { group, room, delta });
    }

    fn draft(&self, group: RoomAuthorityStamp, room: TaskStamp, draft: String) {
        self.update(RoomViewAction::Draft { group, room, draft });
    }

    fn reader(&self, group: RoomAuthorityStamp, room: TaskStamp, offset: i32) {
        self.update(RoomViewAction::Reader {
            group,
            room,
            offset,
        });
    }

    fn focus(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        self.update(RoomViewAction::Focus { group, room });
    }

    fn report(&self, notice: String) {
        self.notice.set(Some(notice));
    }
}

fn restore() -> RoomViews {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(STORAGE).ok().flatten())
        .and_then(|document| RoomViews::restore(&document))
        .unwrap_or_default()
}

fn store(views: &RoomViews) {
    let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    else {
        return;
    };
    if views.groups().is_empty() {
        let _ = storage.remove_item(STORAGE);
    } else if let Some(document) = views.document() {
        let _ = storage.set_item(STORAGE, &document);
    }
}

fn mint_group() -> Option<RoomAuthorityStamp> {
    let now = (js_sys::Date::now() * 1_000.0) as i64;
    LAST_GROUP_MICROS.with(|last| {
        let next = now.max(last.get().saturating_add(1));
        last.set(next);
        RoomAuthorityStamp::for_micros(next).ok()
    })
}

pub(super) fn render_catalog(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
) -> Html {
    let Some(runtime) = runtime else {
        return section(
            "Working groups",
            html! { <p class="empty">{"Unavailable"}</p> },
        );
    };
    let projection = runtime.room_projection();
    let can_put = runtime.offers(CommandKind::WorkingGroupPut);
    let can_remove = runtime.offers(CommandKind::WorkingGroupRemove);
    let board = runtime.board();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="empty" aria-busy="true">{"Loading groups…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="empty">{"Group authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => {
            let create_name = NodeRef::default();
            let create_room = NodeRef::default();
            html! {
                <>
                    if let Some(notice) = &*controls.notice {
                        <p class="room-notice" role="status">{notice.clone()}</p>
                    }
                    {render_new_group(
                        board,
                        can_put,
                        create_name,
                        create_room,
                        controls.clone(),
                        sender.clone(),
                    )}
                    <div class="room-group-catalog">
                        {for projection.groups().values().map(|positioned| {
                            render_catalog_group(
                                runtime,
                                &positioned.value,
                                positioned.position,
                                can_put,
                                can_remove,
                                controls,
                                sender,
                            )
                        })}
                    </div>
                </>
            }
        }
    };
    section("Working groups", body)
}

fn section(title: &'static str, body: Html) -> Html {
    html! {
        <section class="app-spice-section room-group-section" data-room-catalog="">
            <header><h2>{title}</h2><span>{"shared organization"}</span></header>
            {body}
        </section>
    }
}

fn render_new_group(
    board: Option<&crate::runtime::BoardView>,
    enabled: bool,
    name_ref: NodeRef,
    room_ref: NodeRef,
    controls: Controls,
    sender: SenderHandle,
) -> Html {
    let create = new_group_callback(
        name_ref.clone(),
        room_ref.clone(),
        false,
        controls.clone(),
        sender.clone(),
    );
    let create_open =
        new_group_callback(name_ref.clone(), room_ref.clone(), true, controls, sender);
    html! {
        <div class="room-group-create">
            <label>
                <span>{"New working group"}</span>
                <input ref={name_ref} type="text" maxlength="120" placeholder="Group name" disabled={!enabled} />
            </label>
            <label>
                <span>{"Room"}</span>
                <select ref={room_ref} disabled={!enabled}>
                    <option value="">{"No room yet"}</option>
                    {for task_options(board)}
                </select>
            </label>
            <div class="room-inline-actions">
                <button type="button" disabled={!enabled} onclick={create}>{"Create"}</button>
                <button type="button" disabled={!enabled} onclick={create_open}>{"Create & open"}</button>
            </div>
        </div>
    }
}

fn new_group_callback(
    name_ref: NodeRef,
    room_ref: NodeRef,
    open: bool,
    controls: Controls,
    sender: SenderHandle,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let Some(name) = input_value(&name_ref) else {
            return;
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            controls.report("A working-group name is required.".into());
            return;
        }
        let Some(group) = mint_group() else {
            controls.report("A working-group identity could not be created.".into());
            return;
        };
        let room = input_value(&room_ref)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<TaskStamp>().ok());
        let anchors = room.clone().into_iter().collect::<Vec<_>>();
        submit_group(
            &sender,
            &controls,
            WorkingGroup {
                group: group.clone(),
                name,
                anchors,
            },
            None,
        );
        if open && let Some(room) = room {
            controls.open(group, room);
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn render_catalog_group(
    runtime: &AppRuntime,
    group: &WorkingGroup,
    head: wire::Seq,
    can_put: bool,
    can_remove: bool,
    controls: &Controls,
    sender: &SenderHandle,
) -> Html {
    let select = NodeRef::default();
    let add = edit_group_callback(
        select.clone(),
        group.clone(),
        head,
        false,
        controls.clone(),
        sender.clone(),
    );
    let open = edit_group_callback(
        select.clone(),
        group.clone(),
        head,
        true,
        controls.clone(),
        sender.clone(),
    );
    let remove_group = {
        let group = group.group.clone();
        let controls = controls.clone();
        let sender = sender.clone();
        Callback::from(move |_| submit_group_remove(&sender, &controls, group.clone(), head))
    };
    html! {
        <article class="room-group-row" key={group.group.to_string()} data-group={group.group.to_string()}>
            <header>
                <strong>{group.name.clone()}</strong>
                <code>{group.group.to_string()}</code>
            </header>
            <ol>
                {for group.anchors.iter().enumerate().map(|(index, room)| {
                    render_catalog_anchor(group, head, room, index, can_put, controls, sender, runtime)
                })}
            </ol>
            <div class="room-group-add">
                <select ref={select} disabled={!can_put} aria-label={format!("Room for {}", group.name)}>
                    <option value="">{"Choose task"}</option>
                    {for task_options(runtime.board())}
                </select>
                <button type="button" disabled={!can_put} onclick={add}>{"Add to group"}</button>
                <button type="button" disabled={!can_put} onclick={open}>{"Open in group"}</button>
            </div>
            <button class="room-danger" type="button" disabled={!can_remove} onclick={remove_group}>
                {"Remove group"}
            </button>
        </article>
    }
}

#[allow(clippy::too_many_arguments)]
fn render_catalog_anchor(
    group: &WorkingGroup,
    head: wire::Seq,
    room: &TaskStamp,
    index: usize,
    enabled: bool,
    controls: &Controls,
    sender: &SenderHandle,
    runtime: &AppRuntime,
) -> Html {
    let label = anchor_label(runtime, room);
    let open = {
        let controls = controls.clone();
        let group = group.group.clone();
        let room = room.clone();
        Callback::from(move |_| controls.open(group.clone(), room.clone()))
    };
    let move_anchor = |delta: isize| {
        let mut replacement = group.clone();
        let target = index
            .saturating_add_signed(delta)
            .min(replacement.anchors.len() - 1);
        if target != index {
            replacement.anchors.swap(index, target);
        }
        group_action(sender, controls, replacement, head)
    };
    let remove = {
        let mut replacement = group.clone();
        replacement.anchors.remove(index);
        group_action(sender, controls, replacement, head)
    };
    html! {
        <li key={room.to_string()} data-room={room.to_string()}>
            <button type="button" onclick={open}>{label}</button>
            <button type="button" disabled={!enabled || index == 0} onclick={move_anchor(-1)} aria-label="Move room left">{"←"}</button>
            <button type="button" disabled={!enabled || index + 1 == group.anchors.len()} onclick={move_anchor(1)} aria-label="Move room right">{"→"}</button>
            <button type="button" disabled={!enabled} onclick={remove} aria-label="Remove room from group">{"−"}</button>
        </li>
    }
}

fn edit_group_callback(
    select: NodeRef,
    group: WorkingGroup,
    head: wire::Seq,
    open: bool,
    controls: Controls,
    sender: SenderHandle,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let Some(room) = input_value(&select)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<TaskStamp>().ok())
        else {
            controls.report("Choose a task room first.".into());
            return;
        };
        if !group.anchors.contains(&room) {
            let mut replacement = group.clone();
            replacement.anchors.push(room.clone());
            submit_group(&sender, &controls, replacement, Some(head));
        }
        if open {
            controls.open(group.group.clone(), room);
        }
    })
}

fn group_action(
    sender: &SenderHandle,
    controls: &Controls,
    replacement: WorkingGroup,
    head: wire::Seq,
) -> Callback<MouseEvent> {
    let sender = sender.clone();
    let controls = controls.clone();
    Callback::from(move |_| {
        submit_group(&sender, &controls, replacement.clone(), Some(head));
    })
}

fn submit_group(
    sender: &SenderHandle,
    controls: &Controls,
    group: WorkingGroup,
    head: Option<wire::Seq>,
) {
    let notice = controls.notice.clone();
    let respond = Rc::new(
        move |result: Result<wire::intent::WorkingGroupChange, String>| {
            let message = match result {
                Ok(change) => match change.status {
                    WorkingGroupChangeStatus::Stored => "Working group saved.".to_owned(),
                    WorkingGroupChangeStatus::Removed => "Working group removed.".to_owned(),
                },
                Err(error) => error,
            };
            notice.set(Some(message));
        },
    );
    let _ = admit_action(
        sender,
        ClientAction::WorkingGroupPut {
            group,
            head,
            respond,
        },
    );
}

fn submit_group_remove(
    sender: &SenderHandle,
    controls: &Controls,
    group: RoomAuthorityStamp,
    head: wire::Seq,
) {
    let notice = controls.notice.clone();
    let respond = Rc::new(
        move |result: Result<wire::intent::WorkingGroupChange, String>| {
            notice.set(Some(match result {
                Ok(_) => "Working group removed; open local views were preserved.".to_owned(),
                Err(error) => error,
            }));
        },
    );
    let _ = admit_action(
        sender,
        ClientAction::WorkingGroupRemove {
            group,
            head,
            respond,
        },
    );
}

fn task_options(board: Option<&crate::runtime::BoardView>) -> Vec<Html> {
    board
        .into_iter()
        .flat_map(|board| &board.tasks)
        .map(|task| {
            let statement = &task.statement.body;
            html! {
                <option value={statement.born.to_string()}>{statement.handle.to_string()}</option>
            }
        })
        .collect()
}

fn input_value(node: &NodeRef) -> Option<String> {
    let input = node.cast::<HtmlElement>()?;
    js_sys::Reflect::get(input.as_ref(), &JsValue::from_str("value"))
        .ok()?
        .as_string()
}

pub(super) fn render_consoles(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
) -> Html {
    if controls.views.groups().is_empty() {
        return Html::default();
    }
    html! {
        <div class="room-console-fleet" data-room-console-fleet="">
            {for controls.views.groups().iter().enumerate().map(|(index, view)| {
                render_console(runtime, controls, sender, view, index)
            })}
        </div>
    }
}

fn render_console(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    view: &GroupView,
    index: usize,
) -> Html {
    let loaded = runtime.is_some_and(|runtime| {
        runtime.room_projection().status() == RoomProjectionStatus::Available
    });
    let authoritative =
        runtime.and_then(|runtime| runtime.room_projection().groups().get(&view.group));
    // Absence is shown only once a completed snapshot lacks the group; while
    // authority reloads, the console keeps its last name and its geometry.
    let absent = loaded && authoritative.is_none();
    let name = match authoritative {
        Some(group) => {
            controls
                .names
                .borrow_mut()
                .insert(view.group.clone(), group.value.name.clone());
            group.value.name.clone()
        }
        None if absent => "Unavailable working group".to_owned(),
        None => controls
            .names
            .borrow()
            .get(&view.group)
            .cloned()
            .unwrap_or_else(|| "Loading working group…".to_owned()),
    };
    let close = {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |_| controls.close_group(group.clone()))
    };
    let move_group = |delta| {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |_| controls.move_group(group.clone(), delta))
    };
    html! {
        <section class="room-console" key={view.group.to_string()} data-group={view.group.to_string()}>
            <header class="room-console-header">
                <div>
                    <span class="room-console-kicker">{"Working group"}</span>
                    <h2 aria-busy={(!loaded).to_string()}>{name}</h2>
                </div>
                <div class="room-inline-actions">
                    <button type="button" disabled={index == 0} onclick={move_group(-1)} aria-label="Move group left">{"←"}</button>
                    <button type="button" disabled={index + 1 == controls.views.groups().len()} onclick={move_group(1)} aria-label="Move group right">{"→"}</button>
                    <button type="button" onclick={close}>{"Close group"}</button>
                </div>
            </header>
            if absent {
                <p class="room-unavailable">{"Shared group authority is unavailable. This local view remains in place."}</p>
            }
            <div class="room-console-rooms">
                {for view.rooms().iter().enumerate().map(|(room_index, room)| {
                    render_room(runtime, controls, sender, view, room, room_index)
                })}
            </div>
        </section>
    }
}

fn render_room(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    view: &GroupView,
    room: &TaskStamp,
    index: usize,
) -> Html {
    let group = view.group.clone();
    let label = runtime.map_or_else(|| room.to_string(), |runtime| anchor_label(runtime, room));
    let close = {
        let controls = controls.clone();
        let group = group.clone();
        let room = room.clone();
        Callback::from(move |_| controls.close_room(group.clone(), room.clone()))
    };
    let move_room = |delta| {
        let controls = controls.clone();
        let group = group.clone();
        let room = room.clone();
        Callback::from(move |_| controls.move_room(group.clone(), room.clone(), delta))
    };
    let cards = runtime
        .and_then(|runtime| {
            runtime
                .room_projection()
                .room(room)
                .map(|state| (runtime, state))
        })
        .map(|(runtime, room)| {
            room.events()
                .iter()
                .map(|(publication, event)| render_event(runtime, publication, event))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let post = room_post_form(runtime, controls, sender, view, room);
    let grooming = oops_grooming_form(runtime, controls, sender, room);
    let preparation =
        runtime.map_or_else(Html::default, |runtime| render_preparation(runtime, room));
    let reader = {
        let controls = controls.clone();
        let group = group.clone();
        let room = room.clone();
        Callback::from(move |offset| controls.reader(group.clone(), room.clone(), offset))
    };
    let loaded = runtime.is_some_and(|runtime| {
        runtime.room_projection().status() == RoomProjectionStatus::Available
    });
    html! {
        <article class="room-pane" key={room.to_string()} data-room={room.to_string()}>
            <header class="room-pane-header">
                <h3>{label}</h3>
                <div class="room-inline-actions">
                    <button type="button" disabled={index == 0} onclick={move_room(-1)} aria-label="Move room left">{"←"}</button>
                    <button type="button" disabled={index + 1 == view.rooms().len()} onclick={move_room(1)} aria-label="Move room right">{"→"}</button>
                    <button type="button" onclick={close}>{"Close view"}</button>
                </div>
            </header>
            {preparation}
            <RoomReader offset={view.reader(room)} onscroll={reader}>
                if !cards.is_empty() {
                    {for cards}
                } else if loaded {
                    <p class="room-empty">{"No room publications are available."}</p>
                } else {
                    <p class="room-empty" aria-busy="true">{"Loading room…"}</p>
                }
            </RoomReader>
            {grooming}
            {post}
        </article>
    }
}

fn render_preparation(runtime: &AppRuntime, controller: &TaskStamp) -> Html {
    let authority = runtime.room_projection().preparation();
    let mut records = Vec::new();
    for positioned in authority
        .candidates
        .values()
        .filter(|positioned| positioned.value.candidate.controller == *controller)
    {
        records.push((
            positioned.position,
            "Retained candidate",
            serde_json::to_string(&positioned.value).expect("candidate authority encodes"),
        ));
    }
    for positioned in authority
        .critiques
        .values()
        .filter(|positioned| positioned.value.critique.candidate.controller == *controller)
    {
        records.push((
            positioned.position,
            "Critique",
            serde_json::to_string(&positioned.value).expect("critique authority encodes"),
        ));
    }
    for positioned in authority
        .coverage
        .values()
        .filter(|positioned| positioned.value.coverage.candidate.controller == *controller)
    {
        records.push((
            positioned.position,
            "Review coverage",
            serde_json::to_string(&positioned.value).expect("coverage authority encodes"),
        ));
    }
    for positioned in authority
        .decisions
        .values()
        .filter(|positioned| positioned.value.decision.controller == *controller)
    {
        records.push((
            positioned.position,
            "Decision",
            serde_json::to_string(&positioned.value).expect("decision authority encodes"),
        ));
    }
    for positioned in authority
        .authorizations
        .values()
        .filter(|positioned| positioned.value.decision.controller == *controller)
    {
        records.push((
            positioned.position,
            "Execution authorized",
            serde_json::to_string(&positioned.value).expect("authorization authority encodes"),
        ));
    }
    if records.is_empty() {
        return Html::default();
    }
    records.sort_by_key(|record| record.0);
    html! {
        <section class="room-preparation" data-room-preparation="">
            <header>
                <strong>{"Preparation authority"}</strong>
                <span>{records.len()}</span>
            </header>
            {for records.into_iter().map(|(position, kind, body)| html! {
                <article class="room-card room-preparation-card" key={position.to_string()} tabindex="0">
                    <header><strong>{kind}</strong><code>{position.to_string()}</code></header>
                    <pre>{body}</pre>
                </article>
            })}
        </section>
    }
}

fn render_event(
    runtime: &AppRuntime,
    publication: &RoomAuthorityStamp,
    event: &crate::runtime::RoomEvent,
) -> Html {
    let (kind, body) = match &event.kind {
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Authored { text }) => {
            ("Operator", text.clone())
        }
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Linked { link }) => {
            ("Source", format!("Linked source: {link:?}"))
        }
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::OopsGroomed { grooming }) => {
            ("OOPS", grooming_summary(runtime, grooming))
        }
        RoomEventKind::Messaged {
            source, directed, ..
        } => (
            "Agent",
            format!(
                "Transcript source {}:{}–{}{}",
                source.head.source,
                source.text.start,
                source.text.end,
                if directed.is_empty() {
                    ""
                } else {
                    " · directed"
                }
            ),
        ),
        RoomEventKind::Coordination { key, target } => {
            ("Coordination", format!("{key} → {target}"))
        }
        RoomEventKind::PutAside { canceled, stop } => (
            "Put aside",
            if let Some(stop) = stop {
                format!(
                    "Stop requested from {} · {} canceled",
                    stop.target,
                    canceled.len()
                )
            } else {
                format!("{} request(s) canceled", canceled.len())
            },
        ),
    };
    html! {
        <article class="room-card" key={publication.to_string()} data-publication={publication.to_string()} tabindex="0">
            <header><strong>{kind}</strong><code>{publication.to_string()}</code></header>
            <p>{body}</p>
        </article>
    }
}

fn grooming_summary(runtime: &AppRuntime, grooming: &wire::rooms::OopsGrooming) -> String {
    let reports = grooming
        .reports
        .iter()
        .map(|report| {
            let Some(task) = runtime.board().and_then(|board| {
                board
                    .tasks
                    .iter()
                    .find(|task| task.statement.body.born == *report)
            }) else {
                return format!("{report} · unavailable");
            };
            let standing = if wire::is_oops_project(&task.statement.body.project) {
                "parked input"
            } else if task.statement.body.authorization.is_some() {
                "selected authorization"
            } else if task.statement.body.preparation.is_some() {
                "controlled · authorization pending"
            } else {
                "public · uncontrolled"
            };
            format!("{} · {standing}", task.statement.body.handle)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut body = format!(
        "{} · {} · {}",
        grooming.disposition, grooming.cause, reports
    );
    if !grooming.exceptions.is_empty() {
        body.push_str(" · exceptions: ");
        body.push_str(&grooming.exceptions);
    }
    if !grooming.uncertainty.is_empty() {
        body.push_str(" · uncertainty: ");
        body.push_str(&grooming.uncertainty);
    }
    body
}

type OopsReport = (TaskStamp, wire::Seq, wire::TaskHandle);

#[derive(Clone)]
struct OopsInputs {
    report: NodeRef,
    disposition: NodeRef,
    cause: NodeRef,
    exceptions: NodeRef,
    uncertainty: NodeRef,
}

fn oops_grooming_form(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    room: &TaskStamp,
) -> Html {
    let Some(runtime) = runtime else {
        return Html::default();
    };
    let Some(board) = runtime.board() else {
        return Html::default();
    };
    let Some(batch) = board.tasks.iter().find(|task| {
        task.statement.body.born == *room
            && task.statement.body.intent == Some(wire::TaskIntent::Design)
            && task.workflow.body.status == "pending"
            && task.workflow.body.completion.is_none()
    }) else {
        return Html::default();
    };
    let reports = board
        .tasks
        .iter()
        .filter(|task| {
            wire::is_oops_project(&task.statement.body.project)
                && task.workflow.body.completion.is_none()
        })
        .map(|task| {
            (
                task.statement.body.born.clone(),
                task.statement.position,
                task.statement.body.handle.clone(),
            )
        })
        .collect::<Vec<_>>();
    let inputs = OopsInputs {
        report: NodeRef::default(),
        disposition: NodeRef::default(),
        cause: NodeRef::default(),
        exceptions: NodeRef::default(),
        uncertainty: NodeRef::default(),
    };
    let submit = oops_submit(
        controls.clone(),
        sender.clone(),
        reports.clone(),
        batch.statement.body.handle.clone(),
        batch.statement.position,
        inputs.clone(),
    );
    html! {
        <form class="room-grooming" onsubmit={submit}>
            <strong>{"OOPS grooming"}</strong>
            <label>
                <span>{"Parked report"}</span>
                <select ref={inputs.report.clone()} disabled={reports.is_empty()}>
                    <option value="">{"Choose report"}</option>
                    {for reports.iter().map(|(report, _, handle)| html! {
                        <option value={report.to_string()}>{handle.to_string()}</option>
                    })}
                </select>
            </label>
            <label>
                <span>{"Disposition"}</span>
                <select ref={inputs.disposition.clone()}>
                    <option value="duplicate">{"Duplicate"}</option>
                    <option value="no-work">{"No work"}</option>
                    <option value="deferred-for-later">{"Deferred for later"}</option>
                    <option value="covered">{"Covered"}</option>
                    <option value="proposed-work">{"Proposed work"}</option>
                </select>
            </label>
            <label><span>{"Evidenced cause / basis"}</span><input ref={inputs.cause} type="text" /></label>
            <label><span>{"Exceptions"}</span><input ref={inputs.exceptions} type="text" /></label>
            <label><span>{"Uncertainty"}</span><input ref={inputs.uncertainty} type="text" /></label>
            <button type="submit" disabled={reports.is_empty()}>{"Record disposition"}</button>
        </form>
    }
}

fn oops_submit(
    controls: Controls,
    sender: SenderHandle,
    reports: Vec<OopsReport>,
    batch_handle: wire::TaskHandle,
    batch_head: wire::Seq,
    inputs: OopsInputs,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(selected) =
            input_value(&inputs.report).and_then(|value| value.parse::<TaskStamp>().ok())
        else {
            controls.report("Choose one parked OOPS report.".into());
            return;
        };
        let Some((report, report_head, report_handle)) = reports
            .iter()
            .find(|(report, _, _)| *report == selected)
            .cloned()
        else {
            controls.report("The selected OOPS report is no longer parked.".into());
            return;
        };
        let Some(disposition) = input_value(&inputs.disposition)
            .and_then(|value| value.parse::<wire::rooms::OopsDisposition>().ok())
        else {
            controls.report("Choose an OOPS disposition.".into());
            return;
        };
        let cause = input_value(&inputs.cause).unwrap_or_default();
        let cause = cause.trim().to_owned();
        if cause.is_empty() {
            controls.report("An evidenced cause or negative basis is required.".into());
            return;
        }
        let grooming = wire::rooms::OopsGrooming {
            reports: vec![report],
            disposition,
            cause,
            exceptions: input_value(&inputs.exceptions)
                .map_or_else(String::new, |value| value.trim().to_owned()),
            uncertainty: input_value(&inputs.uncertainty)
                .map_or_else(String::new, |value| value.trim().to_owned()),
        };
        let success_controls = controls.clone();
        let batch_for_next = batch_handle.clone();
        let next = (disposition == wire::rooms::OopsDisposition::ProposedWork).then(|| {
            format!(
                " Promote with `spice task wake {report_handle} --preparation {batch_for_next}`; capture remains gated until selected authorization."
            )
        });
        let respond =
            Rc::new(
                move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                    Ok(answer) => success_controls.report(format!(
                        "{}{}",
                        room_post_notice(&answer.outcome),
                        next.as_deref().unwrap_or_default()
                    )),
                    Err(error) => success_controls.report(error),
                },
            );
        let _ = admit_action(
            &sender,
            ClientAction::RoomPost(RoomPostAction {
                room: batch_handle.clone(),
                head: batch_head,
                text: String::new(),
                grooming: Some(grooming),
                report_heads: vec![report_head],
                respond,
            }),
        );
    })
}

#[derive(Properties, PartialEq)]
struct ComposerFocusProps {
    node: NodeRef,
    active: bool,
    disabled: bool,
    /// An explicit open of this room asked for focus.
    requested: bool,
    /// Retires that request once this composer has answered it.
    settle: Callback<()>,
}

/// Focuses a room composer when its explicit open asks, and otherwise
/// restores its saved focus only while nothing else holds focus, however late
/// the composer becomes enabled.
#[function_component]
fn ComposerFocus(props: &ComposerFocusProps) -> Html {
    let node = props.node.clone();
    let settle = props.settle.clone();
    use_effect_with(
        (props.active, props.disabled, props.requested),
        move |(active, disabled, requested)| {
            let focus = !*disabled && (*requested || *active && unfocused_document());
            if focus && let Some(target) = node.cast::<HtmlElement>() {
                let _ = target.focus();
            }
            if *requested {
                settle.emit(());
            }
            || ()
        },
    );
    html! {}
}

/// Whether no element other than the document body holds focus.
fn unfocused_document() -> bool {
    web_sys::window()
        .and_then(|window| window.document())
        .is_some_and(|document| {
            document
                .active_element()
                .is_none_or(|element| element.tag_name().eq_ignore_ascii_case("body"))
        })
}

fn room_post_form(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    view: &GroupView,
    room: &TaskStamp,
) -> Html {
    let authority = runtime.map(|runtime| {
        (
            runtime
                .room_projection()
                .anchor_authority(room, runtime.board()),
            runtime.offers(CommandKind::RoomPost),
        )
    });
    let task = runtime.and_then(|runtime| {
        runtime
            .board()?
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *room)
            .map(|task| (task.statement.body.handle.clone(), task.statement.position))
    });
    let disabled =
        !matches!(authority, Some((RoomAnchorAuthority::Available(_), true))) || task.is_none();
    let textarea = NodeRef::default();
    let requested = controls
        .requested
        .borrow()
        .as_ref()
        .is_some_and(|(group, opened)| *group == view.group && opened == room);
    let settle = {
        let requested = controls.requested.clone();
        Callback::from(move |()| *requested.borrow_mut() = None)
    };
    let oninput = {
        let controls = controls.clone();
        let group = view.group.clone();
        let room = room.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlTextAreaElement = event.target_unchecked_into();
            controls.draft(group.clone(), room.clone(), input.value());
        })
    };
    let onfocus = {
        let controls = controls.clone();
        let group = view.group.clone();
        let room = room.clone();
        Callback::from(move |_| controls.focus(group.clone(), room.clone()))
    };
    let onsubmit = {
        let controls = controls.clone();
        let sender = sender.clone();
        let group = view.group.clone();
        let room_stamp = room.clone();
        let task = task.clone();
        let textarea = textarea.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Some((room, head)) = task.clone() else {
                controls.report("This room source is unavailable.".into());
                return;
            };
            let Some(text) = textarea
                .cast::<HtmlTextAreaElement>()
                .map(|input| input.value())
            else {
                return;
            };
            let text = text.trim().to_owned();
            if text.is_empty() {
                controls.report("A room post cannot be empty.".into());
                return;
            }
            let success_controls = controls.clone();
            let posted = RoomViewAction::Posted {
                group: group.clone(),
                room: room_stamp.clone(),
                text: text.clone(),
            };
            let respond =
                Rc::new(
                    move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                        Ok(answer) => {
                            success_controls.update(posted.clone());
                            success_controls.report(room_post_notice(&answer.outcome));
                        }
                        Err(error) => success_controls.report(error),
                    },
                );
            let _ = admit_action(
                &sender,
                ClientAction::RoomPost(RoomPostAction {
                    room,
                    head,
                    text,
                    grooming: None,
                    report_heads: Vec::new(),
                    respond,
                }),
            );
        })
    };
    html! {
        <form class="room-composer" onsubmit={onsubmit}>
            <textarea
                ref={textarea.clone()}
                value={view.draft(room).to_owned()}
                placeholder="Post to this task room"
                aria-label="Room message"
                disabled={disabled}
                {oninput}
                {onfocus}
            />
            <ComposerFocus node={textarea} active={view.focused(room)} {disabled} {requested} {settle} />
            <button type="submit" disabled={disabled}>{"Post"}</button>
        </form>
    }
}

fn room_post_notice(outcome: &RoomPostOutcome) -> String {
    match outcome {
        RoomPostOutcome::RecordedWithListeners { listeners } => {
            format!(
                "Recorded in room for {} current listener(s).",
                listeners.len()
            )
        }
        RoomPostOutcome::RecordedNoCurrentListener { .. } => {
            "Recorded in room; no current listener. Start or direct a lane explicitly.".into()
        }
    }
}

fn anchor_label(runtime: &AppRuntime, room: &TaskStamp) -> String {
    match runtime
        .room_projection()
        .anchor_authority(room, runtime.board())
    {
        RoomAnchorAuthority::Available(handle) => handle.to_string(),
        RoomAnchorAuthority::Settled { handle, standing } => {
            format!("{handle} · {}", standing.as_str())
        }
        RoomAnchorAuthority::Unavailable => format!("{room} · unavailable"),
        RoomAnchorAuthority::Missing => format!("{room} · missing"),
    }
}

#[derive(Properties, PartialEq)]
struct RoomReaderProps {
    offset: i32,
    onscroll: Callback<i32>,
    children: Children,
}

#[function_component(RoomReader)]
fn room_reader(properties: &RoomReaderProps) -> Html {
    let node = use_node_ref();
    {
        let node = node.clone();
        let offset = properties.offset;
        use_effect_with((), move |()| {
            if let Some(element) = node.cast::<web_sys::HtmlElement>() {
                element.set_scroll_top(offset);
            }
        });
    }
    let onscroll = {
        let node = node.clone();
        let callback = properties.onscroll.clone();
        Callback::from(move |_| {
            if let Some(element) = node.cast::<web_sys::HtmlElement>() {
                callback.emit(element.scroll_top());
            }
        })
    };
    html! {
        <div ref={node} class="room-timeline" {onscroll}>
            {properties.children.clone()}
        </div>
    }
}

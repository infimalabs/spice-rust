//! Working-group catalog and browser-local task-room consoles.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::JsValue;
use web_sys::{HtmlElement, HtmlTextAreaElement};
use wire::intent::WorkingGroupChangeStatus;
use wire::rooms::{RoomAuthorityStamp, RoomPostOutcome, WorkingGroup};
use wire::{CommandKind, TaskStamp};
use yew::prelude::*;

use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::ClientAction;
use crate::room_views::{GroupView, RoomViews};
use crate::runtime::{RoomAnchorAuthority, RoomEventKind, RoomProjectionStatus};

const STORAGE: &str = "spice:room-views:v1";

thread_local! {
    static LAST_GROUP_MICROS: Cell<i64> = const { Cell::new(0) };
}

#[derive(Clone)]
pub(super) struct Controls {
    views: UseStateHandle<RoomViews>,
    notice: UseStateHandle<Option<String>>,
}

#[hook]
pub(super) fn use_controls() -> Controls {
    Controls {
        views: use_state(restore),
        notice: use_state(|| None),
    }
}

impl Controls {
    fn update(&self, apply: impl FnOnce(&mut RoomViews)) {
        let mut views = (*self.views).clone();
        apply(&mut views);
        store(&views);
        self.views.set(views);
    }

    fn open(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        self.update(|views| views.open_room(group, room));
    }

    fn close_room(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        self.update(|views| views.close_room(&group, &room));
    }

    fn close_group(&self, group: RoomAuthorityStamp) {
        self.update(|views| views.close_group(&group));
    }

    fn move_group(&self, group: RoomAuthorityStamp, delta: isize) {
        self.update(|views| views.move_group(&group, delta));
    }

    fn move_room(&self, group: RoomAuthorityStamp, room: TaskStamp, delta: isize) {
        self.update(|views| views.move_room(&group, &room, delta));
    }

    fn draft(&self, group: RoomAuthorityStamp, room: TaskStamp, draft: String) {
        self.update(|views| views.set_draft(&group, &room, draft));
    }

    fn reader(&self, group: RoomAuthorityStamp, room: TaskStamp, offset: i32) {
        self.update(|views| views.set_reader(&group, &room, offset));
    }

    fn focus(&self, group: RoomAuthorityStamp, room: TaskStamp) {
        self.update(|views| views.focus(&group, &room));
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
    let authoritative =
        runtime.and_then(|runtime| runtime.room_projection().groups().get(&view.group));
    let name = authoritative
        .map(|group| group.value.name.clone())
        .unwrap_or_else(|| "Unavailable working group".to_owned());
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
                    <h2>{name}</h2>
                </div>
                <div class="room-inline-actions">
                    <button type="button" disabled={index == 0} onclick={move_group(-1)} aria-label="Move group left">{"←"}</button>
                    <button type="button" disabled={index + 1 == controls.views.groups().len()} onclick={move_group(1)} aria-label="Move group right">{"→"}</button>
                    <button type="button" onclick={close}>{"Close group"}</button>
                </div>
            </header>
            if authoritative.is_none() {
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
        .and_then(|runtime| runtime.room_projection().room(room))
        .map(|room| {
            room.events()
                .iter()
                .map(|(publication, event)| render_event(publication, event))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let post = room_post_form(runtime, controls, sender, view, room);
    let reader = {
        let controls = controls.clone();
        let group = group.clone();
        let room = room.clone();
        Callback::from(move |offset| controls.reader(group.clone(), room.clone(), offset))
    };
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
            <RoomReader offset={view.reader(room)} onscroll={reader}>
                if cards.is_empty() {
                    <p class="room-empty">{"No room publications are available."}</p>
                } else {
                    {for cards}
                }
            </RoomReader>
            {post}
        </article>
    }
}

fn render_event(publication: &RoomAuthorityStamp, event: &crate::runtime::RoomEvent) -> Html {
    let (kind, body) = match &event.kind {
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Authored { text }) => {
            ("Operator", text.clone())
        }
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Linked { link }) => {
            ("Source", format!("Linked source: {link:?}"))
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

#[derive(Properties, PartialEq)]
struct ComposerFocusProps {
    node: NodeRef,
    active: bool,
    disabled: bool,
}

#[function_component]
fn ComposerFocus(props: &ComposerFocusProps) -> Html {
    let node = props.node.clone();
    use_effect_with((props.active, props.disabled), move |(active, disabled)| {
        if *active
            && !*disabled
            && let Some(target) = node.cast::<HtmlElement>()
        {
            let _ = target.focus();
        }
        || ()
    });
    html! {}
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
            let success_group = group.clone();
            let success_room = room_stamp.clone();
            let respond =
                Rc::new(
                    move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                        Ok(answer) => {
                            success_controls.draft(
                                success_group.clone(),
                                success_room.clone(),
                                String::new(),
                            );
                            success_controls.report(room_post_notice(&answer.outcome));
                        }
                        Err(error) => success_controls.report(error),
                    },
                );
            let _ = admit_action(
                &sender,
                ClientAction::RoomPost {
                    room,
                    head,
                    text,
                    respond,
                },
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
            <ComposerFocus node={textarea} active={view.focused(room)} {disabled} />
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

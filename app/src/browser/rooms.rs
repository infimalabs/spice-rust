//! Working-group catalog and browser-local working-group consoles.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlElement;
use wire::intent::WorkingGroupChangeStatus;
use wire::rooms::{RoomAuthorityStamp, RoomPostOutcome, WorkingGroup};
use wire::{CommandKind, TaskStamp};
use yew::prelude::*;

mod conversation;
mod designs;

use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::{ClientAction, RoomPostAction};
use crate::room_views::{GroupView, RoomViewAction, RoomViews};
use crate::runtime::{RoomAnchorAuthority, RoomEventKind, RoomProjectionStatus};

mod console;
pub(super) use console::{GroupRender, render_group_surfaces};

const STORAGE: &str = "spice:room-views:v2";

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
    /// The design composer an explicit open asked to focus, honored once.
    requested: Rc<RefCell<Option<(RoomAuthorityStamp, TaskStamp)>>>,
    /// The text each design composer was drawn with in this page. The live
    /// text belongs to the textarea; redrawing it per keystroke would remount
    /// the composer under the caret.
    drawn: Rc<RefCell<BTreeMap<(RoomAuthorityStamp, TaskStamp), String>>>,
    slots: super::watchband::ComposerSlots<RoomAuthorityStamp>,
}

#[hook]
pub(super) fn use_controls() -> Controls {
    let views = use_reducer(restore);
    let names = use_mut_ref(BTreeMap::new);
    let requested = use_mut_ref(|| None);
    let drawn = use_mut_ref(BTreeMap::new);
    let slots = super::watchband::ComposerSlots(use_mut_ref(std::collections::HashMap::new));
    use_effect_with((*views).clone(), store);
    Controls {
        views,
        notice: use_state(|| None),
        names,
        requested,
        drawn,
        slots,
    }
}

impl Controls {
    fn update(&self, action: RoomViewAction) {
        self.views.dispatch(action);
    }

    /// Mount a group console; naming a design also focuses its composer.
    fn open(&self, group: RoomAuthorityStamp, design: Option<TaskStamp>) {
        *self.requested.borrow_mut() = design.clone().map(|design| (group.clone(), design));
        self.update(RoomViewAction::Open { group, design });
    }

    fn close_group(&self, group: RoomAuthorityStamp) {
        self.update(RoomViewAction::CloseGroup { group });
    }

    fn move_group(&self, group: RoomAuthorityStamp, delta: isize) {
        self.update(RoomViewAction::MoveGroup { group, delta });
    }

    fn draft(&self, group: RoomAuthorityStamp, design: TaskStamp, draft: String) {
        self.update(RoomViewAction::Draft {
            group,
            design,
            draft,
        });
    }

    fn focus(&self, group: RoomAuthorityStamp, design: TaskStamp) {
        self.update(RoomViewAction::Focus { group, design });
    }

    fn report(&self, notice: String) {
        self.notice.set(Some(notice));
    }

    /// The catalog's notice as a callback for surfaces that report there.
    fn reporter(&self) -> Callback<String> {
        let controls = self.clone();
        Callback::from(move |notice| controls.report(notice))
    }

    /// The text a design composer draws with: its saved draft the first time
    /// this page shows it, then whatever a landed post last cleared it to.
    fn drawn(&self, view: &GroupView, design: &TaskStamp) -> String {
        self.drawn
            .borrow_mut()
            .entry((view.group.clone(), design.clone()))
            .or_insert_with(|| view.draft(design).to_owned())
            .clone()
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
    let board = runtime.board();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="empty" aria-busy="true">{"Loading groups…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="empty">{"Group authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => {
            let drops = designs::Drops::new(
                sender.clone(),
                controls.clone(),
                projection
                    .groups()
                    .values()
                    .map(|positioned| (positioned.value.clone(), positioned.position))
                    .collect(),
                can_put,
            );
            html! {
                <>
                    if let Some(notice) = (*controls.notice).clone() {
                        <p class="room-notice" role="status">{notice}</p>
                    }
                    {conversation::render_inbox(runtime, &controls.reporter(), sender)}
                    <div class="room-group-catalog">
                        {for projection.groups().values().map(|positioned| {
                            render_catalog_group(runtime, &positioned.value, &drops, controls)
                        })}
                    </div>
                    {render_new_group(&drops)}
                    {designs::render_designs(board, &drops)}
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

/// The one way a working group begins: a design dropped here, as an agent
/// dropped on New Team begins a team.
fn render_new_group(drops: &designs::Drops) -> Html {
    let designs::DropHandlers { ondragover, ondrop } = designs::new_group_target(drops);
    html! {
        <section
            class="app-sheet-section room-group-create"
            data-design-drop={designs::new_group_value()}
            data-drop-enabled={drops.enabled().to_string()}
            {ondragover}
            {ondrop}
        >
            <header><h3>{"New working group"}</h3><span>{"drop design to create"}</span></header>
            <div class="app-drop-target">{"Drop design here"}</div>
        </section>
    }
}

/// A group's label, drawn from its lead design as a team's is from its host.
pub(super) fn group_label(runtime: &AppRuntime, group: &WorkingGroup) -> String {
    let lead = group.anchors.first();
    let title = lead.and_then(|lead| {
        runtime
            .board()?
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *lead)
            .map(|task| task.presentation_title.clone())
    });
    let host = title.unwrap_or_else(|| {
        lead.map_or_else(
            || group.group.to_string(),
            |lead| anchor_label(runtime, lead),
        )
    });
    format!("{host} group")
}

/// A group's card, read as a team's: its label and size, the console it opens,
/// and its designs as tiles that drag out to wherever they land.
fn render_catalog_group(
    runtime: &AppRuntime,
    group: &WorkingGroup,
    drops: &designs::Drops,
    controls: &Controls,
) -> Html {
    let can_put = drops.enabled();
    let open_group = {
        let group = group.group.clone();
        let controls = controls.clone();
        Callback::from(move |_| controls.open(group.clone(), None))
    };
    let size = match group.anchors.len() {
        1 => "1 design".to_owned(),
        count => format!("{count} designs"),
    };
    let designs::DropHandlers { ondragover, ondrop } = designs::group_target(drops, &group.group);
    html! {
        <article
            class="room-group-row"
            key={group.group.to_string()}
            data-group={group.group.to_string()}
            data-design-drop={group.group.to_string()}
            data-drop-enabled={can_put.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <strong>{group_label(runtime, group)}</strong>
                <span>{size}</span>
                <button type="button" onclick={open_group}>{"Open"}</button>
            </header>
            <ol>
                {for group.anchors.iter().map(|room| html! {
                    <li key={room.to_string()} data-room={room.to_string()}>
                        {designs::render_member(runtime, room, anchor_label(runtime, room), drops)}
                    </li>
                })}
            </ol>
        </article>
    }
}

fn submit_group(
    sender: &SenderHandle,
    controls: &Controls,
    group: WorkingGroup,
    head: Option<wire::Seq>,
) {
    let controls = controls.clone();
    let respond = Rc::new(
        move |result: Result<wire::intent::WorkingGroupChange, String>| {
            let message = match result {
                Ok(change) => match change.status {
                    WorkingGroupChangeStatus::Stored => "Working group saved.".to_owned(),
                    WorkingGroupChangeStatus::Removed => {
                        "Working group ended; its last design left.".to_owned()
                    }
                },
                Err(error) => error,
            };
            controls.report(message);
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

fn input_value(node: &NodeRef) -> Option<String> {
    let input = node.cast::<HtmlElement>()?;
    js_sys::Reflect::get(input.as_ref(), &JsValue::from_str("value"))
        .ok()?
        .as_string()
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
    report: &Callback<String>,
    sender: &SenderHandle,
    publication: &RoomAuthorityStamp,
    event: &crate::runtime::RoomEvent,
) -> Html {
    if let Some(card) = conversation::render_card(runtime, report, sender, publication, event) {
        return card;
    }
    let (kind, body) = match &event.kind {
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Linked { link }) => {
            ("Source", format!("Linked source: {link:?}"))
        }
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::OopsGroomed { grooming }) => {
            ("OOPS", grooming_summary(runtime, grooming))
        }
        // Rendered by the conversation card family whenever its authority
        // is loaded.
        RoomEventKind::Published(wire::rooms::RoomPublicationContent::Authored { .. })
        | RoomEventKind::Messaged { .. }
        | RoomEventKind::Posted => (
            "Conversation",
            "Conversation authority is unavailable.".to_owned(),
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
            } else if task.statement.body.engagement.is_some() {
                "engaged authorization"
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

/// An open, pending Design's OOPS grooming form; its outcomes go to `report`.
fn oops_grooming_form(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
    room: &TaskStamp,
) -> Html {
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
        report.clone(),
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
    notify: Callback<String>,
    sender: SenderHandle,
    reports: Vec<OopsReport>,
    batch_handle: wire::TaskHandle,
    batch_head: wire::Seq,
    inputs: OopsInputs,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let refuse = |notice: &str| notify.emit(notice.to_owned());
        let Some(selected) =
            input_value(&inputs.report).and_then(|value| value.parse::<TaskStamp>().ok())
        else {
            refuse("Choose one parked OOPS report.");
            return;
        };
        let Some((report, report_head, report_handle)) = reports
            .iter()
            .find(|(report, _, _)| *report == selected)
            .cloned()
        else {
            refuse("The selected OOPS report is no longer parked.");
            return;
        };
        let Some(disposition) = input_value(&inputs.disposition)
            .and_then(|value| value.parse::<wire::rooms::OopsDisposition>().ok())
        else {
            refuse("Choose an OOPS disposition.");
            return;
        };
        let cause = input_value(&inputs.cause).unwrap_or_default();
        let cause = cause.trim().to_owned();
        if cause.is_empty() {
            refuse("An evidenced cause or negative basis is required.");
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
        let landed = notify.clone();
        let batch_for_next = batch_handle.clone();
        let next = (disposition == wire::rooms::OopsDisposition::ProposedWork).then(|| {
            format!(
                " Promote with `spice task wake {report_handle} --preparation {batch_for_next}`; capture remains gated until selected authorization."
            )
        });
        let respond =
            Rc::new(
                move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                    Ok(answer) => landed.emit(format!(
                        "{}{}",
                        room_post_notice(&answer.outcome),
                        next.as_deref().unwrap_or_default()
                    )),
                    Err(error) => landed.emit(error),
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
    /// The design composer's textarea.
    id: String,
    active: bool,
    /// An explicit open of this design asked for focus.
    requested: bool,
    /// Retires that request once this composer has answered it.
    settle: Callback<()>,
}

/// Focuses a design composer when its explicit open asks, and otherwise
/// restores its saved focus only while nothing else holds focus.
#[function_component]
fn ComposerFocus(props: &ComposerFocusProps) -> Html {
    let id = props.id.clone();
    let settle = props.settle.clone();
    use_effect_with(
        (props.active, props.requested),
        move |(active, requested)| {
            let focus = *requested || *active && unfocused_document();
            if focus
                && let Some(target) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id(&id))
                    .and_then(|element| element.dyn_into::<HtmlElement>().ok())
            {
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

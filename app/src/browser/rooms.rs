//! Working-group catalog, browser-local working-group consoles, each Design's
//! read view, and the operator's personal inbox.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlElement;
use wire::intent::WorkingGroupChangeStatus;
use wire::rooms::{
    RoomAuthorityStamp, RoomPostOutcome, WorkingGroup, WorkingGroupArrange, WorkingGroupMove,
};
use wire::{CommandKind, TaskStamp};
use yew::prelude::*;

mod card;
mod conversation;

use super::roster::{self, RosterView, Wording};
use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::{ClientAction, GroupChange};
use crate::room_views::{GroupView, RoomViewAction, RoomViews};
use crate::roster::{Change, Destination};
use crate::runtime::{RoomAnchorAuthority, RoomEventKind, RoomProjectionStatus};

mod approval;
mod conclusion;
mod console;
mod contribute;
mod contributions;
mod coordination;
mod invitations;
mod reading;
mod standing;
pub(super) use console::{GroupRender, group_base, render_group_surfaces};
pub(super) use invitations::render_invitations;
pub(super) use reading::render_design_room;

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
    /// The outcome of the last inbox control, shown beside the inbox.
    inbox_notice: UseStateHandle<Option<String>>,
    /// The operator's draft decision for one exact proposal version.
    decision: UseReducerHandle<approval::Composer>,
    /// Proposal versions whose decision awaits the resident's answer.
    deciding: Rc<RefCell<BTreeSet<wire::rooms::CandidateRef>>>,
    /// The read view's Design conclusion preview and last outcome.
    conclusion: UseStateHandle<Option<conclusion::Conclusion>>,
    /// The read view's lane contribution draft.
    composer: UseReducerHandle<contribute::Composer>,
    /// Designs whose contribution awaits the resident's answer.
    contributing: Rc<RefCell<BTreeSet<TaskStamp>>>,
    /// Last authoritative name of each group seen in this page's memory, shown
    /// while room authority reloads so a reconnect does not retitle consoles.
    names: Rc<RefCell<BTreeMap<RoomAuthorityStamp, String>>>,
    /// The design composer an explicit open asked to focus, honored once.
    requested: Rc<RefCell<Option<(RoomAuthorityStamp, TaskStamp)>>>,
    /// The text each design composer was drawn with in this page. The live
    /// text belongs to the textarea; redrawing it per keystroke would remount
    /// the composer under the caret.
    drawn: Rc<RefCell<BTreeMap<(RoomAuthorityStamp, TaskStamp), String>>>,
    /// Design composers whose post awaits the resident's answer.
    posting: Rc<RefCell<BTreeSet<(RoomAuthorityStamp, TaskStamp)>>>,
    /// The group whose arrangement menu is open, if any.
    arranging: UseStateHandle<Option<RoomAuthorityStamp>>,
    slots: super::watchband::ComposerSlots<RoomAuthorityStamp>,
}

#[hook]
pub(super) fn use_controls() -> Controls {
    let views = use_reducer(restore);
    let names = use_mut_ref(BTreeMap::new);
    let requested = use_mut_ref(|| None);
    let drawn = use_mut_ref(BTreeMap::new);
    let posting = use_mut_ref(BTreeSet::new);
    let contributing = use_mut_ref(BTreeSet::new);
    let deciding = use_mut_ref(BTreeSet::new);
    let arranging = use_state_eq(|| None);
    let slots = super::watchband::ComposerSlots(use_mut_ref(std::collections::HashMap::new));
    use_effect_with((*views).clone(), store);
    console::use_arrangement_focus(&arranging);
    Controls {
        views,
        notice: use_state(|| None),
        inbox_notice: use_state(|| None),
        decision: use_reducer(approval::Composer::default),
        deciding,
        conclusion: use_state(|| None),
        composer: use_reducer(contribute::Composer::default),
        contributing,
        names,
        requested,
        drawn,
        posting,
        arranging,
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

    /// The inbox's notice as a callback for its controls.
    fn inbox_reporter(&self) -> Callback<String> {
        let notice = self.inbox_notice.clone();
        Callback::from(move |text| notice.set(Some(text)))
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

const GROUP_WORDING: Wording = Wording {
    kind: "group",
    new_title: "New working group",
    new_hint: "drop design to create",
    new_target: "Drop design here",
    ungrouped_title: "Designs",
    ungrouped_hint: "drop to ungroup",
    full: "Working groups are limited to six designs",
};

pub(super) fn render_catalog(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    read: &Callback<TaskStamp>,
) -> Html {
    let Some(runtime) = runtime else {
        return section(
            "Working groups",
            html! { <p class="empty">{"Unavailable"}</p> },
        );
    };
    let projection = runtime.room_projection();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="empty" aria-busy="true">{"Loading groups…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="empty">{"Group authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => html! {
            <>
                if let Some(notice) = (*controls.notice).clone() {
                    <p class="room-notice" role="status">{notice}</p>
                }
                {render_roster(runtime, controls, sender, read)}
            </>
        },
    };
    section("Working groups", body)
}

/// The operator's personal inbox, beneath their identity. Its heading follows
/// the current identity on every render, so a refresh retitles the same
/// surface without remounting its items.
pub(super) fn render_inbox(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
) -> Html {
    let unresolved = wire::OperatorIdentity::default();
    let operator = runtime
        .and_then(AppRuntime::settings)
        .map_or(&unresolved, |settings| &settings.operator);
    let (count, body) = match runtime {
        None => (
            None,
            html! { <p class="room-unavailable">{"Unavailable"}</p> },
        ),
        Some(runtime) => match runtime.room_projection().status() {
            RoomProjectionStatus::Loading => (
                None,
                html! { <p class="room-empty" aria-busy="true">{"Loading inbox…"}</p> },
            ),
            RoomProjectionStatus::RequiresSnapshot => (
                None,
                html! {
                    <p class="room-unavailable">{"Inbox unavailable until reconnect."}</p>
                },
            ),
            RoomProjectionStatus::Available => {
                let (count, items) =
                    conversation::render_inbox(runtime, &controls.inbox_reporter(), sender);
                (Some(count), items)
            }
        },
    };
    html! {
        <section
            class="room-inbox"
            data-room-inbox=""
            aria-labelledby="spice-operator-inbox-heading"
            tabindex="-1"
        >
            <header>
                <h3 id="spice-operator-inbox-heading">
                    {crate::operator_inbox::heading(operator)}
                </h3>
                if let Some(count) = count {
                    <span>{count}</span>
                }
            </header>
            if let Some(notice) = (*controls.inbox_notice).clone() {
                <p class="room-notice" role="status">{notice}</p>
            }
            {body}
        </section>
    }
}

fn section(title: &'static str, body: Html) -> Html {
    html! {
        <section
            class="app-sheet-section room-group-section"
            aria-labelledby="spice-working-groups-heading"
            data-room-catalog=""
        >
            <div class="section-heading">
                <h2 id="spice-working-groups-heading">{title}</h2>
                <span class="section-count">{"shared organization"}</span>
            </div>
            {body}
        </section>
    }
}

/// The working-group roster: each group a card named for its lead design with
/// the console it opens, then New working group and the ungrouped Designs.
/// Every design tile also opens its read view through `read`.
fn render_roster(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    read: &Callback<TaskStamp>,
) -> Html {
    let partition = runtime.room_projection().partition();
    let tasks = runtime
        .board()
        .map_or(&[][..], |board| board.tasks.as_slice());
    let head = |group: &RoomAuthorityStamp, designs: &[TaskStamp]| {
        let open_group = {
            let group = group.clone();
            let controls = controls.clone();
            Callback::from(move |_| controls.open(group.clone(), None))
        };
        let size = match designs.len() {
            1 => "1 design".to_owned(),
            count => format!("{count} designs"),
        };
        html! {
            <>
                <div>
                    <strong>{lead_label(runtime, group, designs.first())}</strong>
                    <span>{size}</span>
                </div>
                <button type="button" onclick={open_group}>{"Open"}</button>
            </>
        }
    };
    let member = |design: &TaskStamp, _grouped: bool| render_design(runtime, design, read);
    let revision = partition.revision;
    let submit = {
        let (sender, controls) = (sender.clone(), controls.clone());
        Rc::new(move |change| submit_change(&sender, &controls, change, revision))
    };
    roster::render(RosterView {
        wording: &GROUP_WORDING,
        roster: Rc::new(crate::roster::working_groups(partition, tasks)),
        revision,
        enabled: runtime.offers(CommandKind::WorkingGroupMove)
            && runtime.offers(CommandKind::WorkingGroupArrange),
        empty: "No ungrouped designs",
        submit,
        head: &head,
        member: &member,
    })
}

/// One design's tile content: its handle, standing and title while the board
/// carries its task, else its anchor label, and the action reading its room.
fn render_design(runtime: &AppRuntime, design: &TaskStamp, read: &Callback<TaskStamp>) -> Html {
    let task = runtime.board().and_then(|board| board.row(design));
    let label = task.map_or_else(
        || anchor_label(runtime, design),
        |task| task.statement.body.handle.to_string(),
    );
    let standing = runtime
        .room_projection()
        .design_standing(design, runtime.board());
    let onclick = {
        let (read, design) = (read.clone(), design.clone());
        Callback::from(move |_: MouseEvent| read.emit(design.clone()))
    };
    html! {
        <div class="room-design" data-design-standing={standing.key()}>
            <header>
                <strong data-roster-handle="">{label.clone()}</strong>
                <span>{standing.label()}</span>
                <button type="button" aria-label={format!("Read {label}")} {onclick}>{"Read"}</button>
            </header>
            if let Some(task) = task {
                <span class="room-design-title">{task.presentation_title.clone()}</span>
            }
        </div>
    }
}

/// A group's label, drawn from its lead design as a team's is from its host.
pub(super) fn group_label(runtime: &AppRuntime, group: &WorkingGroup) -> String {
    lead_label(runtime, &group.group, group.anchors.first())
}

/// A working group's label from its lead Design, or its identity alone.
pub(in crate::browser) fn lead_label(
    runtime: &AppRuntime,
    group: &RoomAuthorityStamp,
    lead: Option<&TaskStamp>,
) -> String {
    let title = lead.and_then(|lead| {
        runtime
            .board()?
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *lead)
            .map(|task| task.presentation_title.clone())
    });
    let host = title.unwrap_or_else(|| {
        lead.map_or_else(|| group.to_string(), |lead| anchor_label(runtime, lead))
    });
    format!("{host} group")
}

/// Send one working-group roster change at the observed partition revision
/// and report its outcome in the catalog notice. A new group takes a freshly
/// minted identity.
fn submit_change(
    sender: &SenderHandle,
    controls: &Controls,
    change: Change<RoomAuthorityStamp, TaskStamp>,
    revision: u64,
) {
    let change = match change {
        Change::Move { member, to } => {
            let to = match to {
                Destination::New => {
                    let Some(group) = mint_group() else {
                        controls.report("A working-group identity could not be created.".into());
                        return;
                    };
                    Some(group)
                }
                Destination::Group(group) => Some(group),
                Destination::Ungrouped => None,
            };
            GroupChange::Move(WorkingGroupMove { design: member, to })
        }
        Change::Arrange(groups) => GroupChange::Arrange(WorkingGroupArrange {
            groups: groups
                .into_iter()
                .map(|(group, anchors)| WorkingGroup { group, anchors })
                .collect(),
        }),
    };
    let respond = {
        let controls = controls.clone();
        Rc::new(
            move |result: Result<wire::intent::WorkingGroupChange, String>| {
                let message = match result {
                    Ok(change) => match change.status {
                        WorkingGroupChangeStatus::Moved => "Design moved.".to_owned(),
                        WorkingGroupChangeStatus::Ended => {
                            "Design moved; its group ended with its last design.".to_owned()
                        }
                        WorkingGroupChangeStatus::Arranged => "Working groups arranged.".to_owned(),
                        WorkingGroupChangeStatus::Retuned => "Group fast mode set.".to_owned(),
                    },
                    Err(error) => error,
                };
                controls.report(message);
            },
        )
    };
    let _ = admit_action(
        sender,
        ClientAction::WorkingGroupChange {
            change,
            revision,
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

/// A task's statement head, which a command naming the task witnesses.
fn statement_witness(task: &wire::intent::BoardTask) -> wire::Witness {
    wire::Witness::Task {
        task: task.statement.body.born.clone(),
        head: task.statement.position,
    }
}

/// A task's statement, workflow, and claim heads, which a command transitioning
/// or settling it witnesses.
fn transition_witnesses(task: &wire::intent::BoardTask) -> Vec<wire::Witness> {
    let born = &task.statement.body.born;
    vec![
        statement_witness(task),
        wire::Witness::Workflow {
            task: born.clone(),
            head: task.workflow.position,
            phase: task.workflow.body.phase.clone(),
        },
        wire::Witness::Claim {
            task: born.clone(),
            head: task.claim.as_ref().map(|claim| claim.position),
        },
    ]
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
        .withdrawals
        .values()
        .filter(|positioned| positioned.value.withdrawal.candidate.controller == *controller)
    {
        let record = &positioned.value;
        records.push((
            positioned.position,
            "Withdrawal",
            serde_json::json!({
                "withdrawal": record.withdrawal,
                "author": record.author,
                "at": record.at,
            })
            .to_string(),
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

/// A recorded room event outside the conversation and contribution families,
/// as a card face: its kind as the badge and its summary as the words.
fn summary_face(runtime: &AppRuntime, event: &crate::runtime::RoomEvent) -> card::Face {
    let (label, words) = event_summary(runtime, event);
    card::Face {
        speaker: None,
        receivers: String::new(),
        badge: Some(card::Badge::Kind {
            kind: "event",
            label,
        }),
        words,
        tail: Html::default(),
    }
}

/// One event's speaker or kind and the words it carried, as a ticker line
/// names it; a lane packet whose words this browser lacks names only its
/// speaker.
pub(in crate::browser) fn event_headline(
    runtime: &AppRuntime,
    publication: &RoomAuthorityStamp,
    event: &crate::runtime::RoomEvent,
) -> (String, String) {
    match conversation::spoken(runtime, publication, event) {
        Some(spoken) => (spoken.author, spoken.text.unwrap_or_default()),
        None => {
            let (kind, body) = event_summary(runtime, event);
            (kind.to_owned(), body)
        }
    }
}

/// The kind and summary of one event outside the conversation card family.
fn event_summary(
    runtime: &AppRuntime,
    event: &crate::runtime::RoomEvent,
) -> (&'static str, String) {
    match &event.kind {
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
        RoomEventKind::Left { lane, clause } => (
            "Left",
            format!(
                "{} left: {clause}",
                conversation::lane_label(runtime, *lane)
            ),
        ),
        RoomEventKind::Contributed {
            lane,
            contribution,
            text,
            ..
        } => (
            contributions::kind_label(*contribution),
            format!("{}: {text}", conversation::lane_label(runtime, *lane)),
        ),
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
            let standing = if task.statement.body.filing.is_oops() {
                "parked input"
            } else if task.statement.body.authorization.is_some() {
                "selected authorization"
            } else if let Some(preparation) = &task.statement.body.preparation {
                let design = runtime.board().is_some_and(|board| {
                    board.tasks.iter().any(|parent| {
                        parent.statement.body.born == preparation.parent
                            && parent.statement.body.intent == Some(wire::TaskIntent::Design)
                    })
                });
                if design {
                    "controlled · authorization pending"
                } else {
                    "constructed · awaits its Plan"
                }
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

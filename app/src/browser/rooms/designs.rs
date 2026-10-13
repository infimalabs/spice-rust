//! The global menu's Design list and its drop targets. A design moves between
//! working groups exactly as an agent moves between teams: a mouse drags the
//! whole row, a finger or pen drags its handle. Dropped on New working group it
//! begins a group, on a group it joins that group, and on the Design list it
//! leaves its group; a design belongs to at most one group, and a group whose
//! last design leaves ends. Every drop only replaces shared organization
//! through the guarded group put, so the authority's typed refusals surface in
//! the catalog notice.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{DragEvent, PointerEvent};
use wire::TaskStamp;
use wire::intent::BoardTask;
use wire::rooms::{RoomAuthorityStamp, WorkingGroup};
use yew::prelude::*;

use super::{Controls, mint_group, submit_group};
use crate::browser::sender::SenderHandle;
use crate::browser::touch_lift;
use crate::watchband::PointerDragSession;

const DESIGN_MIME: &str = "application/x-spice-design";
/// The attribute naming what a release over an element creates or joins.
const DROP_ATTRIBUTE: &str = "data-design-drop";
/// The drop-target value naming a new working group rather than a saved one.
const NEW_GROUP_DROP: &str = "new";
/// The drop-target value naming no group: a design released there leaves its group.
const UNGROUP_DROP: &str = "none";
/// Design lifts span no roster topology: each drop is guarded by its group's
/// head instead, so one constant revision keeps a lift alive across renders.
const DESIGN_LIFT_REVISION: u64 = 0;

thread_local! {
    /// The one design lift a finger or pen holds; mouse uses HTML5 drag and drop.
    static TOUCH_DRAG: RefCell<PointerDragSession<TaskStamp>> = RefCell::default();
}

/// Everything a design drop needs to replace shared organization, captured
/// from the render that drew the targets.
#[derive(Clone)]
pub(super) struct Drops {
    sender: SenderHandle,
    controls: Controls,
    /// Every saved group beside the head its replacement must witness.
    groups: Rc<Vec<(WorkingGroup, wire::Seq)>>,
    enabled: bool,
}

impl Drops {
    pub(super) fn new(
        sender: SenderHandle,
        controls: Controls,
        groups: Vec<(WorkingGroup, wire::Seq)>,
        enabled: bool,
    ) -> Self {
        Self {
            sender,
            controls,
            groups: Rc::new(groups),
            enabled,
        }
    }

    pub(super) const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The saved group holding `design`, beside its head.
    fn holder(&self, design: &TaskStamp) -> Option<&(WorkingGroup, wire::Seq)> {
        self.groups
            .iter()
            .find(|(saved, _)| saved.anchors.contains(design))
    }

    fn grouped(&self, design: &TaskStamp) -> bool {
        self.holder(design).is_some()
    }

    /// Where a release over `value` lands `design`, absent where the design
    /// already is.
    fn target(&self, value: &str, design: &TaskStamp) -> Option<Target> {
        match value {
            NEW_GROUP_DROP => Some(Target::New),
            UNGROUP_DROP => self.grouped(design).then_some(Target::Ungroup),
            _ => {
                let group = value.parse::<RoomAuthorityStamp>().ok()?;
                let (saved, _) = self.groups.iter().find(|(saved, _)| saved.group == group)?;
                (!saved.anchors.contains(design)).then_some(Target::Group(group))
            }
        }
    }

    /// Submit the replacement a drop asks for; whether a command was sent.
    /// Joining or beginning a group names the design there, and the authority
    /// takes it out of the group that held it.
    fn land(&self, target: Target, design: TaskStamp) -> bool {
        let (replacement, head) = match target {
            Target::New => {
                let Some(group) = mint_group() else {
                    self.controls
                        .report("A working-group identity could not be created.".into());
                    return false;
                };
                (
                    WorkingGroup {
                        group,
                        anchors: vec![design],
                    },
                    None,
                )
            }
            Target::Group(group) => {
                let Some((saved, head)) =
                    self.groups.iter().find(|(saved, _)| saved.group == group)
                else {
                    return false;
                };
                let mut replacement = saved.clone();
                replacement.anchors.push(design);
                (replacement, Some(*head))
            }
            Target::Ungroup => {
                let Some((saved, head)) = self.holder(&design) else {
                    return false;
                };
                let mut replacement = saved.clone();
                replacement.anchors.retain(|held| *held != design);
                (replacement, Some(*head))
            }
        };
        submit_group(&self.sender, &self.controls, replacement, head);
        true
    }
}

enum Target {
    New,
    Group(RoomAuthorityStamp),
    Ungroup,
}

/// The handlers that make an element a design drop target. The element also
/// carries `data-design-drop` and `data-drop-enabled`.
pub(super) struct DropHandlers {
    pub(super) ondragover: Callback<DragEvent>,
    pub(super) ondrop: Callback<DragEvent>,
}

/// A saved group's row accepts designs it does not already hold. A full group
/// still accepts the drop, so the authority's limit refusal reports it.
pub(super) fn group_target(drops: &Drops, group: &RoomAuthorityStamp) -> DropHandlers {
    target_handlers(drops, group.to_string())
}

pub(super) fn new_group_target(drops: &Drops) -> DropHandlers {
    target_handlers(drops, NEW_GROUP_DROP.to_owned())
}

pub(super) fn new_group_value() -> &'static str {
    NEW_GROUP_DROP
}

fn target_handlers(drops: &Drops, value: String) -> DropHandlers {
    let enabled = drops.enabled;
    let drops = drops.clone();
    DropHandlers {
        ondragover: Callback::from(move |event: DragEvent| {
            if enabled {
                event.prevent_default();
                if let Some(transfer) = event.data_transfer() {
                    transfer.set_drop_effect("copy");
                }
            }
        }),
        ondrop: Callback::from(move |event: DragEvent| {
            event.prevent_default();
            let Some(design) = dragged_design(&event) else {
                return;
            };
            if drops.enabled
                && let Some(target) = drops.target(&value, &design)
                && drops.land(target, design)
            {
                event.stop_propagation();
            }
        }),
    }
}

fn dragged_design(event: &DragEvent) -> Option<TaskStamp> {
    event
        .data_transfer()?
        .get_data(DESIGN_MIME)
        .ok()?
        .parse()
        .ok()
}

/// Every Design task a working group could hold, in board order.
pub(super) fn designs(
    board: Option<&crate::runtime::BoardView>,
) -> impl Iterator<Item = &BoardTask> {
    board
        .into_iter()
        .flat_map(|board| &board.tasks)
        .filter(|task| task.statement.body.intent == Some(wire::TaskIntent::Design))
}

/// The Design list, the working-group counterpart of Unassigned Agents: it
/// shows designs no group holds, and a grouped design dropped here leaves its
/// group.
pub(super) fn render_designs(board: Option<&crate::runtime::BoardView>, drops: &Drops) -> Html {
    let designs = designs(board)
        .filter(|task| !drops.grouped(&task.statement.body.born))
        .collect::<Vec<_>>();
    let DropHandlers { ondragover, ondrop } = target_handlers(drops, UNGROUP_DROP.to_owned());
    html! {
        <section
            class="room-designs"
            data-design-list=""
            data-design-drop={UNGROUP_DROP}
            data-drop-enabled={drops.enabled.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <h3>{"Designs"}</h3>
                <span>{"drop to ungroup"}</span>
            </header>
            <div class="room-design-list">
                if designs.is_empty() {
                    <p class="app-empty-state">{"No ungrouped designs"}</p>
                } else {
                    {for designs.into_iter().map(|task| render_design(task, drops))}
                }
            </div>
        </section>
    }
}

fn render_design(task: &BoardTask, drops: &Drops) -> Html {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    render_draggable(
        &statement.born,
        statement.handle.to_string(),
        html! { <span>{format!("{} · {}", workflow.phase, workflow.status)}</span> },
        html! { <span class="room-design-title">{task.presentation_title.clone()}</span> },
        drops,
    )
}

/// A grouped design's tile, drawn as the Design list draws it while the board
/// carries its task, else under its anchor label.
pub(super) fn render_member(
    runtime: &crate::AppRuntime,
    design: &TaskStamp,
    label: String,
    drops: &Drops,
) -> Html {
    match designs(runtime.board()).find(|task| task.statement.body.born == *design) {
        Some(task) => render_design(task, drops),
        None => render_draggable(design, label, Html::default(), Html::default(), drops),
    }
}

/// One design as a drag source, wherever it stands: its label is the finger
/// or pen handle, `aside` shares its header, and `body` follows.
pub(super) fn render_draggable(
    design: &TaskStamp,
    label: String,
    aside: Html,
    body: Html,
    drops: &Drops,
) -> Html {
    let design = design.clone();
    html! {
        <div
            key={design.to_string()}
            class="room-design"
            draggable={drops.enabled.to_string()}
            data-design={design.to_string()}
            ondragstart={begin_drag(design.clone(), drops.enabled)}
            onpointerdown={touch_start(design.clone(), drops.enabled)}
            onpointermove={touch_movement(design.clone(), drops.clone())}
            onpointerup={touch_finish(design.clone(), drops.clone())}
            onpointercancel={touch_cancel(design.clone())}
            onlostpointercapture={touch_cancel(design)}
        >
            <header><strong>{label}</strong>{aside}</header>
            {body}
        </div>
    }
}

fn begin_drag(design: TaskStamp, enabled: bool) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if !enabled {
            event.prevent_default();
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if transfer.set_data(DESIGN_MIME, design.as_str()).is_ok() {
            transfer.set_effect_allowed("copy");
        }
    })
}

/// Begin a finger or pen lift from a design's handle.
fn touch_start(design: TaskStamp, enabled: bool) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        if event.pointer_type() == "mouse" || !event.is_primary() || !on_touch_handle(&event) {
            return;
        }
        // A primary contact going down ends any lift whose release never arrived.
        if TOUCH_DRAG.with_borrow_mut(PointerDragSession::clear) {
            touch_lift::clear();
        }
        TOUCH_DRAG.with_borrow_mut(|session| {
            session.begin(
                enabled,
                design.clone(),
                event.pointer_id(),
                event.client_x(),
                event.client_y(),
                DESIGN_LIFT_REVISION,
            )
        });
    })
}

fn on_touch_handle(event: &PointerEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| {
            element
                .closest(".room-design > header > strong")
                .ok()
                .flatten()
        })
        .is_some()
}

fn touch_movement(design: TaskStamp, drops: Drops) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        if TOUCH_DRAG
            .with_borrow_mut(|session| session.reconcile(drops.enabled, Some(DESIGN_LIFT_REVISION)))
        {
            touch_lift::clear();
            return;
        }
        let Some(lift) = TOUCH_DRAG.with_borrow_mut(|session| {
            let active = session.movement(
                &design,
                event.pointer_id(),
                event.client_x(),
                event.client_y(),
                DESIGN_LIFT_REVISION,
            )?;
            let lift = !active.dragging;
            active.dragging = true;
            Some(lift)
        }) else {
            return;
        };
        let Some(handle) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if lift
            && (handle.set_pointer_capture(event.pointer_id()).is_err()
                || touch_lift::begin(
                    &handle,
                    ".room-design",
                    "data-design",
                    event.client_x(),
                    event.client_y(),
                )
                .is_none())
        {
            TOUCH_DRAG.with_borrow_mut(PointerDragSession::clear);
            touch_lift::clear();
            return;
        }
        event.prevent_default();
        touch_lift::follow(&handle, event.client_x(), event.client_y());
        touch_lift::preview(
            touch_drop_at(&drops, &design, event.client_x(), event.client_y())
                .map(|(_, target)| target)
                .as_ref(),
        );
    })
}

fn touch_finish(design: TaskStamp, drops: Drops) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        let finished = TOUCH_DRAG.with_borrow_mut(|session| {
            if session.reconcile(drops.enabled, Some(DESIGN_LIFT_REVISION)) {
                return None;
            }
            session.finish(&design, event.pointer_id(), DESIGN_LIFT_REVISION)
        });
        if !finished.is_some_and(|active| active.dragging) {
            touch_lift::clear();
            return;
        }
        let target = touch_drop_at(&drops, &design, event.client_x(), event.client_y())
            .map(|(target, _)| target);
        touch_lift::clear();
        event.prevent_default();
        if let Some(target) = target {
            drops.land(target, design.clone());
        }
    })
}

fn touch_cancel(design: TaskStamp) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        if TOUCH_DRAG.with_borrow_mut(|session| session.cancel(&design, event.pointer_id())) {
            touch_lift::clear();
        }
    })
}

/// The group a lifted design would join if released here, beside the target
/// element that states it. Releasing where the design already is states nothing.
fn touch_drop_at(
    drops: &Drops,
    design: &TaskStamp,
    client_x: i32,
    client_y: i32,
) -> Option<(Target, web_sys::Element)> {
    let element = touch_lift::target_at(DROP_ATTRIBUTE, client_x, client_y)?;
    let target = drops.target(&element.get_attribute(DROP_ATTRIBUTE)?, design)?;
    Some((target, element))
}

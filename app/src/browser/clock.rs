//! Deadline-driven relative ages for the browser canvas.
//!
//! Ages used to be arithmetic done during render: a component read
//! `Date::now()`, subtracted an instant, and spelled the difference. That makes
//! the age a property of *when the frame happened* rather than of the thing
//! being aged, so a lane the resident has stopped talking about keeps showing
//! the span it had when its last frame arrived. The canvas looked idle because
//! it was idle.
//!
//! Each [`Age`] schedules only the next instant at which its visible spelling
//! can change. A seconds-old message wakes each second, a minutes-old message
//! wakes once a minute, and a saturated age never wakes. Keeping that timer in
//! the leaf avoids reconciling the complete canvas VDOM for every seconds-old
//! label. The callback writes only a changed text node; any later Yew render
//! computes that same text from the current clock. Dropping the effect cancels
//! the pending browser timer and prevents the callback from rearming itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use yew::prelude::*;

use crate::watchband::{AGE_LABEL_WIDTH, elapsed_label, elapsed_label_deadline_millis};

#[derive(Properties, PartialEq)]
pub(super) struct AgeProps {
    /// Epoch milliseconds selected by the caller's display chronology.
    ///
    /// Provenance remains with the caller; this component only renders elapsed
    /// time and does not classify the instant as authored or observed.
    pub(super) authored: i64,
    /// Element class, so a caller keeps its own placement and colour.
    pub(super) class: Classes,
    /// Hover text, which carries the exact instants the label rounds away.
    #[prop_or_default]
    pub(super) title: AttrValue,
}

/// One relative age, advancing at its next visible deadline.
///
/// The rendered span is a fixed-width slot: `data-age-slot` carries the
/// character budget the label is bounded to, and the stylesheet sizes the box
/// from it once. Because the box never depends on the text, a tick replaces the
/// text node and leaves every neighbouring layout box exactly where it was.
#[function_component(Age)]
pub(super) fn age(props: &AgeProps) -> Html {
    let slot = use_node_ref();
    {
        let slot = slot.clone();
        use_effect_with(props.authored, move |authored| {
            let active = Rc::new(Cell::new(true));
            let pending = Rc::new(RefCell::new(None));
            schedule(slot, *authored, Rc::clone(&active), Rc::clone(&pending));
            move || {
                active.set(false);
                pending.borrow_mut().take();
            }
        });
    }
    let now = now_millis();
    html! {
        <span
            ref={slot}
            class={props.class.clone()}
            title={props.title.clone()}
            data-age-slot={AGE_LABEL_WIDTH.to_string()}
            data-age-authored={props.authored.to_string()}
            data-age-clock="deadline"
        >{elapsed_label(now.saturating_sub(props.authored))}</span>
    }
}

fn schedule(
    slot: NodeRef,
    authored: i64,
    active: Rc<Cell<bool>>,
    pending: Rc<RefCell<Option<Timeout>>>,
) {
    let Some(delay) = elapsed_label_deadline_millis(now_millis().saturating_sub(authored)) else {
        return;
    };
    let next_pending = Rc::clone(&pending);
    let timeout = Timeout::new(delay, move || {
        next_pending.borrow_mut().take();
        if !active.get() {
            return;
        }
        let Some(element) = slot.cast::<web_sys::Element>() else {
            return;
        };
        let label = elapsed_label(now_millis().saturating_sub(authored));
        if element.text_content().as_deref() != Some(&label) {
            element.set_text_content(Some(&label));
        }
        schedule(slot, authored, active, next_pending);
    });
    pending.borrow_mut().replace(timeout);
}

/// The browser's current instant in epoch milliseconds.
fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

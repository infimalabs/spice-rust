//! Deadline-driven relative ages for the browser canvas.
//!
//! Ages used to be arithmetic done during render: a component read
//! `Date::now()`, subtracted an instant, and spelled the difference. That makes
//! the age a property of *when the frame happened* rather than of the thing
//! being aged, so a lane the resident has stopped talking about keeps showing
//! the span it had when its last frame arrived. The canvas looked idle because
//! it was idle.
//!
//! Each relative label schedules only the next instant at which its visible
//! spelling can change. A seconds-old message wakes each second, while a lease
//! countdown wakes at the decreasing boundary for its displayed unit. Keeping
//! that timer in the leaf avoids reconciling the complete canvas VDOM for every
//! relative label. The callback writes only a changed text node; any later Yew
//! render computes that same text from the current clock. Dropping the effect
//! cancels the pending browser timer and prevents the callback from rearming.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use yew::prelude::*;

use crate::watchband::{
    AGE_LABEL_WIDTH, elapsed_label, elapsed_label_deadline_millis, remaining_label,
    remaining_label_deadline_millis,
};

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

#[derive(Properties, PartialEq)]
pub(super) struct CountdownProps {
    /// Authoritative epoch millisecond at which the lease expires.
    pub(super) expiry: i64,
    /// Element class, so a caller keeps its own placement and colour.
    pub(super) class: Classes,
    /// Hover text, which carries exact context the label rounds away.
    #[prop_or_default]
    pub(super) title: AttrValue,
}

#[derive(Clone, Copy)]
enum RelativeClock {
    Elapsed,
    Remaining,
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
            schedule(
                slot,
                *authored,
                RelativeClock::Elapsed,
                Rc::clone(&active),
                Rc::clone(&pending),
            );
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

/// One relative countdown, advancing toward an authoritative expiry deadline.
#[function_component(Countdown)]
pub(super) fn countdown(props: &CountdownProps) -> Html {
    let slot = use_node_ref();
    {
        let slot = slot.clone();
        use_effect_with(props.expiry, move |expiry| {
            let active = Rc::new(Cell::new(true));
            let pending = Rc::new(RefCell::new(None));
            schedule(
                slot,
                *expiry,
                RelativeClock::Remaining,
                Rc::clone(&active),
                Rc::clone(&pending),
            );
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
            data-expiry={props.expiry.to_string()}
            data-age-clock="countdown"
        >{remaining_label(props.expiry.saturating_sub(now))}</span>
    }
}

fn schedule(
    slot: NodeRef,
    instant: i64,
    clock: RelativeClock,
    active: Rc<Cell<bool>>,
    pending: Rc<RefCell<Option<Timeout>>>,
) {
    let relative = relative_millis(clock, instant);
    let deadline = match clock {
        RelativeClock::Elapsed => elapsed_label_deadline_millis,
        RelativeClock::Remaining => remaining_label_deadline_millis,
    };
    let Some(delay) = deadline(relative) else {
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
        let label = relative_label(clock, instant);
        if element.text_content().as_deref() != Some(&label) {
            element.set_text_content(Some(&label));
        }
        schedule(slot, instant, clock, active, next_pending);
    });
    pending.borrow_mut().replace(timeout);
}

fn relative_millis(clock: RelativeClock, instant: i64) -> i64 {
    match clock {
        RelativeClock::Elapsed => now_millis().saturating_sub(instant),
        RelativeClock::Remaining => instant.saturating_sub(now_millis()),
    }
}

fn relative_label(clock: RelativeClock, instant: i64) -> String {
    match clock {
        RelativeClock::Elapsed => elapsed_label(relative_millis(clock, instant)),
        RelativeClock::Remaining => remaining_label(relative_millis(clock, instant)),
    }
}

/// The browser's current instant in epoch milliseconds.
fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

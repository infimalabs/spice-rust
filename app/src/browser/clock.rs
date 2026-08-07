//! One browser clock, shared by every relative age the canvas draws.
//!
//! Ages used to be arithmetic done during render: a component read
//! `Date::now()`, subtracted an instant, and spelled the difference. That makes
//! the age a property of *when the frame happened* rather than of the thing
//! being aged, so a lane the resident has stopped talking about keeps showing
//! the span it had when its last frame arrived. The canvas looked idle because
//! it was idle.
//!
//! The clock is inverted here. [`WallClock`] owns the only interval in the
//! canvas and publishes the current instant as context; [`Age`] consumes it and
//! is the only thing that subtracts. Every surface that shows an age holds an
//! absolute authored instant and renders an `<Age>`, so they all advance
//! together off one timer, and none of them needs a frame from the resident.
//!
//! The shape matters as much as the sharing. `Age` is a leaf, so a tick
//! re-renders the label and nothing above it. That is what keeps a ticking age
//! away from the Mosaic: the plane's props never mention time, so a second
//! passing cannot re-measure a card or move a placement.

use gloo_timers::callback::Interval;
use yew::prelude::*;

use crate::watchband::{AGE_LABEL_WIDTH, elapsed_label};

/// How often the shared clock republishes the current instant.
///
/// One second because that is the finest distinction `elapsed_label` draws: a
/// faster tick would republish an instant that spells the same label, and a
/// slower one would let the seconds visibly skip.
const TICK_MILLIS: u32 = 1_000;

/// The current instant, in epoch milliseconds, as the canvas last observed it.
///
/// A newtype rather than a bare `i64` so the context lookup is unambiguous, and
/// `PartialEq` so a republished-but-identical instant costs consumers nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Clock(pub(super) i64);

#[derive(Properties, PartialEq)]
pub(super) struct WallClockProps {
    pub(super) children: Html,
}

/// Publish a once-a-second instant to everything rendered beneath.
///
/// The interval is started by the effect that runs on mount and dropped by its
/// teardown, so it is tied to the canvas being on screen and to nothing else.
/// In particular it does not wait on the resident: an operator who opens the
/// canvas while discovery is failing still watches the ages move, which is the
/// only way the screen can distinguish "nothing is happening" from "I am no
/// longer being told what is happening".
#[function_component(WallClock)]
pub(super) fn wall_clock(props: &WallClockProps) -> Html {
    let now = use_state_eq(now_millis);
    {
        let now = now.clone();
        use_effect_with((), move |()| {
            let interval = Interval::new(TICK_MILLIS, move || now.set(now_millis()));
            move || drop(interval)
        });
    }
    html! {
        <ContextProvider<Clock> context={Clock(*now)}>
            {props.children.clone()}
        </ContextProvider<Clock>>
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct AgeProps {
    /// Epoch milliseconds the aged thing was authored, claimed or renewed.
    pub(super) authored: i64,
    /// Element class, so a caller keeps its own placement and colour.
    pub(super) class: Classes,
    /// Hover text, which carries the exact instants the label rounds away.
    #[prop_or_default]
    pub(super) title: AttrValue,
}

/// One relative age, advancing on the shared clock.
///
/// The rendered span is a fixed-width slot: `data-age-slot` carries the
/// character budget the label is bounded to, and the stylesheet sizes the box
/// from it once. Because the box never depends on the text, a tick replaces the
/// text node and leaves every neighbouring layout box exactly where it was.
#[function_component(Age)]
pub(super) fn age(props: &AgeProps) -> Html {
    // An `Age` outside the provider still reads correctly on first paint and
    // then freezes -- which is the exact defect this module exists to remove,
    // arriving silently. Nothing renders one today, and nothing should: the
    // provider wraps the whole canvas. The fallback is here so a future caller
    // placed outside it is wrong about liveness rather than blank, and the
    // absent `data-age-clock` attribute is what names it as unprovided.
    let clock = use_context::<Clock>();
    let now = clock.map_or_else(now_millis, |clock| clock.0);
    html! {
        <span
            class={props.class.clone()}
            title={props.title.clone()}
            data-age-slot={AGE_LABEL_WIDTH.to_string()}
            data-age-authored={props.authored.to_string()}
            data-age-clock={clock.map(|_| "shared")}
        >{elapsed_label(now - props.authored)}</span>
    }
}

/// The browser's current instant in epoch milliseconds.
fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

//! Deadline-driven relative ages for the browser app.
//!
//! Ages used to be arithmetic done during render: a component read
//! `Date::now()`, subtracted an instant, and spelled the difference. That makes
//! the age a property of *when the frame happened* rather than of the thing
//! being aged, so a lane the resident has stopped talking about keeps showing
//! the span it had when its last frame arrived. The app looked idle because
//! it was idle.
//!
//! Each relative label schedules only the next instant at which its visible
//! spelling can change. A seconds-old message wakes each second, while a lease
//! countdown wakes at the decreasing boundary for its displayed unit. Keeping
//! that timer in the leaf avoids reconciling the complete app VDOM for every
//! relative label. The callback writes only a changed text node; any later Yew
//! render computes that same text from the current clock. Dropping the effect
//! cancels the pending browser timer and prevents the callback from rearming.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use js_sys::Intl::DateTimeFormat;
use js_sys::{Array, Date, Object, Reflect};
use wasm_bindgen::JsValue;
use yew::prelude::*;

use crate::watchband::{
    AGE_SLOT_WIDTH, elapsed_label, elapsed_label_deadline_millis, remaining_label,
    remaining_label_deadline_millis, task_elapsed_label, task_elapsed_label_deadline_millis,
    task_remaining_label, task_remaining_label_deadline_millis,
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
    /// Canonical card permalink. Absent ages remain plain text.
    #[prop_or_default]
    pub(super) href: Option<AttrValue>,
    /// Accessible action label for a linked age.
    #[prop_or_default]
    pub(super) label: AttrValue,
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

/// Density-aware task timestamp presentation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskTimePresentation {
    DetailedElapsed,
    DetailedRemaining,
}

impl TaskTimePresentation {
    const fn clock(self) -> RelativeClock {
        match self {
            Self::DetailedElapsed => RelativeClock::Elapsed,
            Self::DetailedRemaining => RelativeClock::Remaining,
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::DetailedElapsed => "detailed-elapsed",
            Self::DetailedRemaining => "detailed-remaining",
        }
    }
}

#[derive(Properties, PartialEq)]
struct TaskTimestampProps {
    instant: i64,
    canonical: AttrValue,
    presentation: TaskTimePresentation,
}

#[derive(Clone)]
enum ClockLabels {
    Relative,
    Task {
        compact: AttrValue,
        full: AttrValue,
        detailed: bool,
    },
}

impl ClockLabels {
    fn render(&self, clock: RelativeClock, relative: i64) -> String {
        match self {
            Self::Relative => relative_label_from_millis(clock, relative),
            Self::Task {
                compact,
                full,
                detailed,
            } => match task_relative_label(clock, relative) {
                Some(relative) if *detailed => format!("{relative} · {full}"),
                Some(relative) => relative,
                None if *detailed => full.to_string(),
                None => compact.to_string(),
            },
        }
    }

    fn deadline(&self, clock: RelativeClock, relative: i64) -> Option<u32> {
        match (self, clock) {
            (Self::Relative, RelativeClock::Elapsed) => elapsed_label_deadline_millis(relative),
            (Self::Relative, RelativeClock::Remaining) => remaining_label_deadline_millis(relative),
            (Self::Task { .. }, RelativeClock::Elapsed) => {
                task_elapsed_label_deadline_millis(relative)
            }
            (Self::Task { .. }, RelativeClock::Remaining) => {
                task_remaining_label_deadline_millis(relative)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RelativeClock {
    Elapsed,
    Remaining,
}

/// One relative age, advancing at its next visible deadline.
///
/// The rendered span is an exact fixed-width slot: `data-age-slot` carries that
/// width and the stylesheet owns its geometry. A tick replaces only the text
/// node and leaves every neighboring layout box where it was.
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
                ClockLabels::Relative,
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
    let text = elapsed_label(now.saturating_sub(props.authored));
    if let Some(href) = &props.href {
        html! {
            <a
                ref={slot}
                class={props.class.clone()}
                title={props.title.clone()}
                href={href.clone()}
                aria-label={props.label.clone()}
                data-age-slot={AGE_SLOT_WIDTH.to_string()}
                data-age-authored={props.authored.to_string()}
                data-age-clock="deadline"
            >{text}</a>
        }
    } else {
        html! {
            <span
                ref={slot}
                class={props.class.clone()}
                title={props.title.clone()}
                data-age-slot={AGE_SLOT_WIDTH.to_string()}
                data-age-authored={props.authored.to_string()}
                data-age-clock="deadline"
            >{text}</span>
        }
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
                ClockLabels::Relative,
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
            data-age-slot={AGE_SLOT_WIDTH.to_string()}
            data-expiry={props.expiry.to_string()}
            data-age-clock="countdown"
        >{remaining_label(props.expiry.saturating_sub(now))}</span>
    }
}

/// One task timestamp with compact/card and detailed/fact presentations sharing
/// the same 24-hour policy and browser-local absolute formatters.
#[function_component(TaskTimestamp)]
fn task_timestamp(props: &TaskTimestampProps) -> Html {
    let compact = local_timestamp(props.instant, &props.canonical, LocalStyle::Compact);
    let full = local_timestamp(props.instant, &props.canonical, LocalStyle::Full);
    let labels = ClockLabels::Task {
        compact,
        full: full.clone(),
        detailed: true,
    };
    let clock = props.presentation.clock();
    let slot = use_node_ref();
    {
        let slot = slot.clone();
        let labels = labels.clone();
        let effect = (props.instant, props.presentation);
        use_effect_with(effect, move |(instant, _)| {
            let active = Rc::new(Cell::new(true));
            let pending = Rc::new(RefCell::new(None));
            schedule(
                slot,
                *instant,
                clock,
                labels,
                Rc::clone(&active),
                Rc::clone(&pending),
            );
            move || {
                active.set(false);
                pending.borrow_mut().take();
            }
        });
    }
    let relative = relative_millis(clock, props.instant);
    html! {
        <time
            ref={slot}
            class="task-time"
            datetime={props.canonical.clone()}
            title={full.clone()}
            aria-label={full}
            data-task-time={props.presentation.key()}
            data-task-time-instant={props.instant.to_string()}
        >{labels.render(clock, relative)}</time>
    }
}

pub(super) fn task_time(instant: &wire::Instant, presentation: TaskTimePresentation) -> Html {
    html! {
        <TaskTimestamp
            instant={instant.get().as_millisecond()}
            canonical={instant.to_string()}
            {presentation}
        />
    }
}

fn schedule(
    slot: NodeRef,
    instant: i64,
    clock: RelativeClock,
    labels: ClockLabels,
    active: Rc<Cell<bool>>,
    pending: Rc<RefCell<Option<Timeout>>>,
) {
    let relative = relative_millis(clock, instant);
    let Some(delay) = labels.deadline(clock, relative) else {
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
        let label = labels.render(clock, relative_millis(clock, instant));
        if element.text_content().as_deref() != Some(&label) {
            element.set_text_content(Some(&label));
        }
        schedule(slot, instant, clock, labels, active, next_pending);
    });
    pending.borrow_mut().replace(timeout);
}

fn relative_millis(clock: RelativeClock, instant: i64) -> i64 {
    match clock {
        RelativeClock::Elapsed => now_millis().saturating_sub(instant),
        RelativeClock::Remaining => instant.saturating_sub(now_millis()),
    }
}

fn relative_label_from_millis(clock: RelativeClock, relative: i64) -> String {
    match clock {
        RelativeClock::Elapsed => elapsed_label(relative),
        RelativeClock::Remaining => remaining_label(relative),
    }
}

fn task_relative_label(clock: RelativeClock, relative: i64) -> Option<String> {
    match clock {
        RelativeClock::Elapsed => task_elapsed_label(relative),
        RelativeClock::Remaining => task_remaining_label(relative),
    }
}

#[derive(Clone, Copy)]
enum LocalStyle {
    Compact,
    Full,
    Date,
}

thread_local! {
    static COMPACT_TIME_FORMATTER: DateTimeFormat = local_formatter("short", Some("short"));
    static FULL_TIME_FORMATTER: DateTimeFormat = local_formatter("medium", Some("long"));
    static DATE_FORMATTER: DateTimeFormat = local_formatter("long", None);
}

fn local_formatter(date_style: &str, time_style: Option<&str>) -> DateTimeFormat {
    let options = Object::new();
    Reflect::set(
        options.as_ref(),
        &JsValue::from_str("dateStyle"),
        &JsValue::from_str(date_style),
    )
    .expect("the date formatter accepts dateStyle");
    if let Some(time_style) = time_style {
        Reflect::set(
            options.as_ref(),
            &JsValue::from_str("timeStyle"),
            &JsValue::from_str(time_style),
        )
        .expect("the date formatter accepts timeStyle");
    }
    DateTimeFormat::new(&Array::new(), &options)
}

fn local_timestamp(instant: i64, canonical: &str, style: LocalStyle) -> AttrValue {
    let date = Date::new(&JsValue::from_f64(instant as f64));
    let rendered = match style {
        LocalStyle::Compact => COMPACT_TIME_FORMATTER.with(|formatter| {
            formatter
                .format()
                .call1(&JsValue::UNDEFINED, date.as_ref())
                .ok()
                .and_then(|value| value.as_string())
        }),
        LocalStyle::Full => FULL_TIME_FORMATTER.with(|formatter| {
            formatter
                .format()
                .call1(&JsValue::UNDEFINED, date.as_ref())
                .ok()
                .and_then(|value| value.as_string())
        }),
        LocalStyle::Date => DATE_FORMATTER.with(|formatter| {
            formatter
                .format()
                .call1(&JsValue::UNDEFINED, date.as_ref())
                .ok()
                .and_then(|value| value.as_string())
        }),
    };
    rendered
        .filter(|value| !value.is_empty() && value != "Invalid Date")
        .unwrap_or_else(|| canonical.to_owned())
        .into()
}

pub(super) fn local_date_millis(instant: i64, canonical: &str) -> String {
    let fallback = canonical
        .split_once('T')
        .map_or(canonical, |(date, _)| date);
    local_timestamp(instant, fallback, LocalStyle::Date).to_string()
}

/// The browser's current instant in epoch milliseconds.
pub(super) fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

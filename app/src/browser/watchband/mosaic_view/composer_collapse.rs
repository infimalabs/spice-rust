//! Passive scroll-input projection onto one team's composer exposure.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Element, HtmlElement, KeyboardEvent, ResizeObserver, TouchEvent, WheelEvent};
use yew::prelude::*;

const COLLAPSE_EPSILON_PX: f64 = 0.25;
const WHEEL_LINE_PX: f64 = 16.0;

#[derive(Default)]
struct CollapseState {
    offset: f64,
    maximum: f64,
    touch_y: Option<i32>,
}

type SharedCollapse = Rc<RefCell<CollapseState>>;

#[hook]
pub(super) fn use_composer_collapse(host: NodeRef) {
    let state = use_mut_ref(CollapseState::default);
    use_effect_with(host, move |host| {
        let registration = host
            .cast::<Element>()
            .and_then(|scroller| Registration::new(scroller, state));
        move || drop(registration)
    });
}

struct Registration {
    scroller: Element,
    observer: ResizeObserver,
    _resize: Closure<dyn FnMut(js_sys::Array, ResizeObserver)>,
    wheel: Closure<dyn FnMut(WheelEvent)>,
    key: Closure<dyn FnMut(KeyboardEvent)>,
    touch_start: Closure<dyn FnMut(TouchEvent)>,
    touch_move: Closure<dyn FnMut(TouchEvent)>,
    touch_end: Closure<dyn FnMut(TouchEvent)>,
}

fn composer_elements(scroller: &Element) -> Option<(HtmlElement, HtmlElement)> {
    let surface = scroller
        .closest(".team-compose")
        .ok()
        .flatten()?
        .dyn_into::<HtmlElement>()
        .ok()?;
    let composer = surface
        .query_selector(".team-composer")
        .ok()
        .flatten()?
        .dyn_into::<HtmlElement>()
        .ok()?;
    Some((surface, composer))
}

impl Registration {
    fn new(scroller: Element, state: SharedCollapse) -> Option<Self> {
        let (surface, composer) = composer_elements(&scroller)?;
        sync_metrics(&surface, &composer, &state);

        let resized_surface = surface.clone();
        let resized_composer = composer.clone();
        let resized_state = Rc::clone(&state);
        let resize = Closure::<dyn FnMut(js_sys::Array, ResizeObserver)>::new(move |_, _| {
            sync_metrics(&resized_surface, &resized_composer, &resized_state);
        });
        let observer = ResizeObserver::new(resize.as_ref().unchecked_ref()).ok()?;
        observer.observe(&composer);

        let wheel_scroller = scroller.clone();
        let wheel_surface = surface.clone();
        let wheel_state = Rc::clone(&state);
        let wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |event| {
            apply_input_delta(
                &wheel_surface,
                &wheel_state,
                wheel_delta(&wheel_scroller, &event),
            );
        });
        scroller
            .add_event_listener_with_callback("wheel", wheel.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its wheel listener");

        let key_scroller = scroller.clone();
        let key_surface = surface.clone();
        let key_state = Rc::clone(&state);
        let key = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event| {
            let Some(requested) = keyboard_offset(&key_scroller, &key_state, &event) else {
                return;
            };
            set_collapse(&key_surface, &key_state, requested);
        });
        scroller
            .add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its keyboard listener");

        let start_state = Rc::clone(&state);
        let touch_start = Closure::<dyn FnMut(TouchEvent)>::new(move |event| {
            start_state.borrow_mut().touch_y = first_touch_y(&event);
        });
        scroller
            .add_event_listener_with_callback("touchstart", touch_start.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its touch-start listener");

        let move_surface = surface;
        let move_state = Rc::clone(&state);
        let touch_move = Closure::<dyn FnMut(TouchEvent)>::new(move |event| {
            let Some(y) = first_touch_y(&event) else {
                return;
            };
            let delta = {
                let mut state = move_state.borrow_mut();
                let Some(previous) = state.touch_y.replace(y) else {
                    return;
                };
                f64::from(previous - y)
            };
            apply_input_delta(&move_surface, &move_state, delta);
        });
        scroller
            .add_event_listener_with_callback("touchmove", touch_move.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its touch-move listener");

        let end_state = state;
        let touch_end = Closure::<dyn FnMut(TouchEvent)>::new(move |_| {
            end_state.borrow_mut().touch_y = None;
        });
        let touch_end_callback = touch_end.as_ref().unchecked_ref();
        scroller
            .add_event_listener_with_callback("touchend", touch_end_callback)
            .expect("the owned activity plane accepts its touch-end listener");
        scroller
            .add_event_listener_with_callback("touchcancel", touch_end_callback)
            .expect("the owned activity plane accepts its touch-cancel listener");

        Some(Self {
            scroller,
            observer,
            _resize: resize,
            wheel,
            key,
            touch_start,
            touch_move,
            touch_end,
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.observer.disconnect();
        let _ = self
            .scroller
            .remove_event_listener_with_callback("wheel", self.wheel.as_ref().unchecked_ref());
        let _ = self
            .scroller
            .remove_event_listener_with_callback("keydown", self.key.as_ref().unchecked_ref());
        let _ = self.scroller.remove_event_listener_with_callback(
            "touchstart",
            self.touch_start.as_ref().unchecked_ref(),
        );
        let _ = self.scroller.remove_event_listener_with_callback(
            "touchmove",
            self.touch_move.as_ref().unchecked_ref(),
        );
        let callback = self.touch_end.as_ref().unchecked_ref();
        let _ = self
            .scroller
            .remove_event_listener_with_callback("touchend", callback);
        let _ = self
            .scroller
            .remove_event_listener_with_callback("touchcancel", callback);
    }
}

fn first_touch_y(event: &TouchEvent) -> Option<i32> {
    event.touches().item(0).map(|touch| touch.client_y())
}

fn wheel_delta(scroller: &Element, event: &WheelEvent) -> f64 {
    match event.delta_mode() {
        WheelEvent::DOM_DELTA_LINE => event.delta_y() * WHEEL_LINE_PX,
        WheelEvent::DOM_DELTA_PAGE => event.delta_y() * f64::from(scroller.client_height()),
        _ => event.delta_y(),
    }
}

fn keyboard_offset(
    scroller: &Element,
    state: &SharedCollapse,
    event: &KeyboardEvent,
) -> Option<f64> {
    let state = state.borrow();
    match event.key().as_str() {
        "Home" => Some(0.0),
        "End" => Some(state.maximum),
        "ArrowDown" => Some(state.offset + WHEEL_LINE_PX),
        "ArrowUp" => Some(state.offset - WHEEL_LINE_PX),
        "PageDown" => Some(state.offset + f64::from(scroller.client_height())),
        "PageUp" => Some(state.offset - f64::from(scroller.client_height())),
        " " if event.shift_key() => Some(state.offset - f64::from(scroller.client_height())),
        " " => Some(state.offset + f64::from(scroller.client_height())),
        _ => None,
    }
}

fn apply_input_delta(surface: &HtmlElement, state: &SharedCollapse, delta: f64) {
    if !delta.is_finite() || delta.abs() <= COLLAPSE_EPSILON_PX {
        return;
    }
    let requested = {
        let state = state.borrow();
        crate::mosaic::composer_collapse_offset(state.offset, state.maximum, delta)
    };
    set_collapse(surface, state, requested);
}

fn sync_metrics(surface: &HtmlElement, composer: &HtmlElement, state: &SharedCollapse) {
    let maximum = f64::from(composer.offset_height());
    let requested = {
        let mut state = state.borrow_mut();
        let was_fully_collapsed = state.maximum > COLLAPSE_EPSILON_PX
            && state.offset >= state.maximum - COLLAPSE_EPSILON_PX;
        state.maximum = maximum;
        if was_fully_collapsed {
            maximum
        } else {
            state.offset.min(maximum)
        }
    };
    set_collapse(surface, state, requested);
}

fn set_collapse(surface: &HtmlElement, state: &SharedCollapse, requested: f64) {
    let (previous, next, maximum) = {
        let mut state = state.borrow_mut();
        let previous = state.offset;
        let next = requested.clamp(0.0, state.maximum);
        state.offset = next;
        (previous, next, state.maximum)
    };
    if (next - previous).abs() <= f64::EPSILON && surface.has_attribute("data-composer-collapse") {
        return;
    }
    surface
        .style()
        .set_property("--team-composer-collapse", &format!("{next}px"))
        .expect("the composer collapse CSS property is valid");
    surface
        .set_attribute("data-composer-collapse", &next.to_string())
        .expect("the composer collapse diagnostic attribute is valid");
    surface
        .set_attribute(
            "data-composer-fully-collapsed",
            if maximum > COLLAPSE_EPSILON_PX && next >= maximum - COLLAPSE_EPSILON_PX {
                "true"
            } else {
                "false"
            },
        )
        .expect("the composer collapse state attribute is valid");
}

//! One team's composer exposure, fused to the Chat plane's scroll motion.
//!
//! The composer overlays the top of the plane, whose scroll content starts
//! below an equal spacer. Every change to the plane's scroll position moves the
//! composer by the same distance until it is fully revealed or hidden, so it
//! reads as part of the stream. Only Mosaic's own anchor correction, which keeps
//! cards visually still, is excluded.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, Event, HtmlElement, KeyboardEvent, ResizeObserver};
use yew::prelude::*;

const COLLAPSE_EPSILON_PX: f64 = 0.25;

struct CollapseState {
    offset: f64,
    maximum: f64,
    /// The plane scroll position the offset last followed.
    scroll_top: f64,
    /// `scrollTop` read through JavaScript keeps subpixel positions that the
    /// integer `web_sys` getter truncates, so the composer never drifts off the
    /// cards on high-density displays.
    scroll_top_key: JsValue,
}

impl CollapseState {
    fn new() -> Self {
        Self {
            offset: 0.0,
            maximum: 0.0,
            scroll_top: 0.0,
            scroll_top_key: JsValue::from_str("scrollTop"),
        }
    }

    fn plane_scroll_top(&self, scroller: &Element) -> f64 {
        js_sys::Reflect::get(scroller, &self.scroll_top_key)
            .ok()
            .and_then(|top| top.as_f64())
            .unwrap_or_else(|| f64::from(scroller.scroll_top()))
    }
}

type SharedCollapse = Rc<RefCell<CollapseState>>;

/// The composer exposure a plane's scroll owner consults before correcting it.
#[derive(Clone)]
pub(super) struct ComposerCollapse(SharedCollapse);

impl ComposerCollapse {
    /// Write a programmatic scroll position without moving the composer.
    pub(super) fn correct_scroll(&self, scroller: &Element, top: i32) {
        let before = self.0.borrow().plane_scroll_top(scroller);
        scroller.set_scroll_top(top);
        let mut state = self.0.borrow_mut();
        let applied = state.plane_scroll_top(scroller) - before;
        state.scroll_top += applied;
    }
}

#[hook]
pub(super) fn use_composer_collapse(host: NodeRef) -> ComposerCollapse {
    let state = use_mut_ref(CollapseState::new);
    {
        let state = Rc::clone(&state);
        use_effect_with(host, move |host| {
            let registration = host
                .cast::<Element>()
                .and_then(|scroller| Registration::new(scroller, state));
            move || drop(registration)
        });
    }
    ComposerCollapse(state)
}

struct Registration {
    scroller: Element,
    observer: ResizeObserver,
    _resize: Closure<dyn FnMut(js_sys::Array, ResizeObserver)>,
    scroll: Closure<dyn FnMut(Event)>,
    key: Closure<dyn FnMut(KeyboardEvent)>,
    _paint: Closure<dyn FnMut(f64)>,
    frame: FrameScheduler,
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

/// One direct compositor write per presentation frame. Keeping the changing
/// values on their owning elements avoids invalidating every card through an
/// inherited custom property while the plane scrolls.
#[derive(Clone)]
struct FrameScheduler {
    window: web_sys::Window,
    callback: js_sys::Function,
    pending: Rc<Cell<Option<i32>>>,
}

impl FrameScheduler {
    fn schedule(&self) {
        if self.pending.get().is_some() {
            return;
        }
        let frame = self
            .window
            .request_animation_frame(&self.callback)
            .expect("Canvas window accepts a composer presentation frame");
        self.pending.set(Some(frame));
    }

    fn cancel(&self) {
        if let Some(frame) = self.pending.take() {
            let _ = self.window.cancel_animation_frame(frame);
        }
    }
}

impl Registration {
    fn new(scroller: Element, state: SharedCollapse) -> Option<Self> {
        let scroll_element = scroller.clone().dyn_into::<HtmlElement>().ok()?;
        let (surface, composer) = composer_elements(&scroller)?;
        {
            let mut state = state.borrow_mut();
            state.scroll_top = state.plane_scroll_top(&scroller);
        }
        sync_metrics(&surface, &composer, &scroll_element, &state);

        let pending = Rc::new(Cell::new(None));
        let painted_pending = Rc::clone(&pending);
        let painted_surface = surface.clone();
        let painted_composer = composer.clone();
        let painted_scroller = scroll_element.clone();
        let painted_state = Rc::clone(&state);
        let paint = Closure::<dyn FnMut(f64)>::new(move |_| {
            painted_pending.set(None);
            paint_collapse(
                &painted_surface,
                &painted_composer,
                &painted_scroller,
                &painted_state,
            );
        });
        let frame = FrameScheduler {
            window: web_sys::window()?,
            callback: paint.as_ref().unchecked_ref::<js_sys::Function>().clone(),
            pending,
        };

        let resized_surface = surface.clone();
        let resized_composer = composer.clone();
        let resized_scroller = scroll_element.clone();
        let resized_state = Rc::clone(&state);
        let resize = Closure::<dyn FnMut(js_sys::Array, ResizeObserver)>::new(move |_, _| {
            sync_metrics(
                &resized_surface,
                &resized_composer,
                &resized_scroller,
                &resized_state,
            );
        });
        let observer = ResizeObserver::new(resize.as_ref().unchecked_ref()).ok()?;
        observer.observe(&composer);

        let scroll_scroller = scroller.clone();
        let scroll_surface = surface.clone();
        let scroll_state = Rc::clone(&state);
        let scroll_frame = frame.clone();
        let scroll = Closure::<dyn FnMut(Event)>::new(move |_| {
            follow_scroll(
                &scroll_surface,
                &scroll_scroller,
                &scroll_state,
                &scroll_frame,
            );
        });
        scroller
            .add_event_listener_with_callback("scroll", scroll.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its scroll listener");

        let key_surface = surface.clone();
        let key_composer = composer.clone();
        let key_scroller = scroll_element.clone();
        let key_state = Rc::clone(&state);
        let key = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            let requested = match event.key().as_str() {
                "Home" => 0.0,
                "End" => key_state.borrow().maximum,
                _ => return,
            };
            set_collapse(
                &key_surface,
                &key_composer,
                &key_scroller,
                &key_state,
                requested,
            );
        });
        scroller
            .add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref())
            .expect("the owned activity plane accepts its keyboard listener");

        Some(Self {
            scroller,
            observer,
            _resize: resize,
            scroll,
            key,
            _paint: paint,
            frame,
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.frame.cancel();
        self.observer.disconnect();
        let _ = self
            .scroller
            .remove_event_listener_with_callback("scroll", self.scroll.as_ref().unchecked_ref());
        let _ = self
            .scroller
            .remove_event_listener_with_callback("keydown", self.key.as_ref().unchecked_ref());
    }
}

fn follow_scroll(
    surface: &HtmlElement,
    scroller: &Element,
    state: &SharedCollapse,
    frame: &FrameScheduler,
) {
    let requested = {
        let mut state = state.borrow_mut();
        let top = state.plane_scroll_top(scroller);
        let scrolled = top - state.scroll_top;
        state.scroll_top = top;
        crate::mosaic::composer_collapse_offset(state.offset, state.maximum, scrolled, top)
    };
    if update_collapse(state, requested) || !surface.has_attribute("data-composer-collapse") {
        frame.schedule();
    }
}

fn sync_metrics(
    surface: &HtmlElement,
    composer: &HtmlElement,
    scroller: &HtmlElement,
    state: &SharedCollapse,
) {
    let maximum = f64::from(composer.offset_height());
    surface
        .style()
        .set_property("--team-composer-block", &format!("{maximum}px"))
        .expect("the composer block CSS property is valid");
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
    set_collapse(surface, composer, scroller, state, requested);
}

fn set_collapse(
    surface: &HtmlElement,
    composer: &HtmlElement,
    scroller: &HtmlElement,
    state: &SharedCollapse,
    requested: f64,
) {
    if update_collapse(state, requested) || !surface.has_attribute("data-composer-collapse") {
        paint_collapse(surface, composer, scroller, state);
    }
}

fn update_collapse(state: &SharedCollapse, requested: f64) -> bool {
    let mut state = state.borrow_mut();
    let next = requested.clamp(0.0, state.maximum);
    if (next - state.offset).abs() <= f64::EPSILON {
        return false;
    }
    state.offset = next;
    true
}

fn paint_collapse(
    surface: &HtmlElement,
    composer: &HtmlElement,
    scroller: &HtmlElement,
    state: &SharedCollapse,
) {
    let (offset, maximum) = {
        let state = state.borrow();
        (state.offset, state.maximum)
    };
    composer
        .style()
        .set_property("transform", &format!("translate3d(0, {}px, 0)", -offset))
        .expect("the composer transform is valid");
    scroller
        .style()
        .set_property(
            "scroll-padding-top",
            &format!("{}px", (maximum - offset).max(0.0)),
        )
        .expect("the plane scroll padding is valid");
    surface
        .set_attribute("data-composer-collapse", &offset.to_string())
        .expect("the composer collapse diagnostic attribute is valid");
    let fully_collapsed =
        if maximum > COLLAPSE_EPSILON_PX && offset >= maximum - COLLAPSE_EPSILON_PX {
            "true"
        } else {
            "false"
        };
    if surface
        .get_attribute("data-composer-fully-collapsed")
        .as_deref()
        != Some(fully_collapsed)
    {
        surface
            .set_attribute("data-composer-fully-collapsed", fully_collapsed)
            .expect("the composer collapse state attribute is valid");
    }
}

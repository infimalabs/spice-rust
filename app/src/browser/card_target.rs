use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use yew::prelude::*;

use crate::CanvasRuntime;

use super::SurfaceData;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Status {
    pub(super) target: String,
    pub(super) available: bool,
}

/// History-state key naming where a reader left through a quote link.
const RETURN_STATE: &str = "canvasReturn";

/// The card holding a followed quote link and its offset inside its lane
/// plane at that moment. Chat scrolls inside each lane plane rather than the
/// document, so the browser cannot restore this position itself.
struct ReturnAnchor {
    card: String,
    offset: f64,
    /// Distinguishes one recorded departure from a later one on the same entry.
    nonce: f64,
}

impl ReturnAnchor {
    fn read(history: &web_sys::History) -> Option<Self> {
        let anchor = js_sys::Reflect::get(&history.state().ok()?, &RETURN_STATE.into()).ok()?;
        let field = |name: &str| js_sys::Reflect::get(&anchor, &name.into()).ok();
        Some(Self {
            card: field("card")?.as_string()?,
            offset: field("offset")?.as_f64()?,
            nonce: field("nonce")?.as_f64()?,
        })
    }

    /// Scroll the anchor card back to its recorded offset; false while absent.
    fn restore(&self, document: &web_sys::Document) -> bool {
        let Some(card) = document.get_element_by_id(&self.card) else {
            return false;
        };
        let Some((plane, offset)) = plane_offset(&card) else {
            return false;
        };
        plane.scroll_by_with_x_and_y(0.0, offset - self.offset);
        if let Ok(card) = card.dyn_into::<web_sys::HtmlElement>() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = card.focus_with_options(&options);
        }
        true
    }
}

/// A card's scrolling lane plane and the card's top edge relative to it.
fn plane_offset(card: &web_sys::Element) -> Option<(web_sys::Element, f64)> {
    let plane = card.closest(".lane-mosaic-plane").ok()??;
    let offset = card.get_bounding_client_rect().top() - plane.get_bounding_client_rect().top();
    Some((plane, offset))
}

/// Record, in the history entry a plain quote-link click is about to leave,
/// the card holding that link and where it sits in its lane plane, so Back
/// returns the reader there instead of leaving them at the quoted message.
pub(super) fn remember_quote_return(event: &web_sys::MouseEvent) {
    if event.default_prevented()
        || event.button() != 0
        || event.meta_key()
        || event.ctrl_key()
        || event.shift_key()
        || event.alt_key()
    {
        return;
    }
    let Some(link) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest("a.message-quote-age[href]").ok().flatten())
    else {
        return;
    };
    let Some(card) = link.closest(".mosaic-message-card").ok().flatten() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    // A link to the current entry navigates nowhere, so nothing is left behind.
    if card.id().is_empty() || link.get_attribute("href") == window.location().hash().ok() {
        return;
    }
    let (Ok(history), Some((_, offset))) = (window.history(), plane_offset(&card)) else {
        return;
    };
    let state = js_sys::Object::new();
    // Keep the history library's own entry identity beside the anchor.
    if let Ok(current) = history.state()
        && let Some(current) = current.dyn_ref::<js_sys::Object>()
    {
        js_sys::Object::assign(&state, current);
    }
    let anchor = js_sys::Object::new();
    for (name, value) in [
        ("card", JsValue::from(card.id())),
        ("offset", JsValue::from(offset)),
        ("nonce", JsValue::from(js_sys::Date::now())),
    ] {
        let _ = js_sys::Reflect::set(&anchor, &name.into(), &value);
    }
    let _ = js_sys::Reflect::set(&state, &RETURN_STATE.into(), &anchor);
    let _ = history.replace_state(&state, "");
}

/// Restore a returned-to entry's anchor once per arrival at that entry.
fn returned_to(
    restored: &Rc<RefCell<Option<f64>>>,
    anchor: &ReturnAnchor,
    document: &web_sys::Document,
) -> bool {
    if *restored.borrow() == Some(anchor.nonce) {
        return true;
    }
    let returned = anchor.restore(document);
    if returned {
        *restored.borrow_mut() = Some(anchor.nonce);
    }
    returned
}

/// Keeps a routed card where its route brought it while staged history grows
/// its lane plane around it. The plane disables browser scroll anchoring, so
/// without this a cold permalink load drifts off its card as older cards land
/// above it. The first scroll the pin did not make belongs to the reader and
/// releases it.
struct Pin {
    observer: web_sys::ResizeObserver,
    _aligned: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
}

impl Pin {
    fn hold(card: &web_sys::Element) -> Option<Self> {
        let (plane, offset) = plane_offset(card)?;
        let track = card.closest(".lane-mosaic-track").ok()??;
        let expected = Cell::new(plane.scroll_top());
        let card = card.clone();
        let aligned = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
            move |_, observer: web_sys::ResizeObserver| {
                if plane.scroll_top() != expected.get() || !card.is_connected() {
                    observer.disconnect();
                    return;
                }
                if let Some((_, now)) = plane_offset(&card) {
                    plane.scroll_by_with_x_and_y(0.0, now - offset);
                }
                expected.set(plane.scroll_top());
            },
        );
        let observer = web_sys::ResizeObserver::new(aligned.as_ref().unchecked_ref()).ok()?;
        observer.observe(&track);
        Some(Self {
            observer,
            _aligned: aligned,
        })
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

/// Bring a route's element into view and focus, report it reached, and pin it.
fn reach(
    element: &web_sys::Element,
    target: &str,
    status: &UseStateHandle<Option<Status>>,
) -> Option<Pin> {
    element.scroll_into_view();
    if let Some(element) = element.dyn_ref::<web_sys::HtmlElement>() {
        let _ = element.focus();
    }
    status.set(Some(Status {
        target: target.to_owned(),
        available: true,
    }));
    Pin::hold(element)
}

/// Layout phase attribute; a `measuring` card is hidden until its lattice commits.
const LAYOUT_PHASE: &str = "data-layout-phase";

/// The rendered element a route names. A card counts only once its layout
/// commits: while measuring it is hidden, so it can neither be scrolled to
/// meaningfully nor take focus, and its node may be replaced as the layout commits.
/// Route targets are ASCII alphanumerics and `-`, so they quote verbatim.
fn routed_element(document: &web_sys::Document, target: &str) -> Option<web_sys::Element> {
    let nodes = document
        .query_selector_all(&format!("[id=\"{target}\"]"))
        .ok()?;
    (0..nodes.length())
        .filter_map(|index| nodes.item(index)?.dyn_into::<web_sys::Element>().ok())
        .find(|element| element.get_attribute(LAYOUT_PHASE).as_deref() != Some("measuring"))
}

/// Watches the document for a routed element that has not rendered yet.
/// Staged hydration renders cards without advancing the stream cursor, so a
/// cold permalink otherwise waits on an unrelated live event to find its card.
struct Seek {
    observer: web_sys::MutationObserver,
    _found: Closure<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
    /// The card this seek marked selected, outside the route effect's care.
    marked: Rc<RefCell<Option<web_sys::Element>>>,
}

impl Seek {
    fn start(
        document: &web_sys::Document,
        target: &str,
        status: &UseStateHandle<Option<Status>>,
        pin: &Rc<RefCell<Option<Pin>>>,
    ) -> Option<Self> {
        let body = document.body()?;
        let marked = Rc::new(RefCell::new(None));
        let (document, target, status, pin, mark) = (
            document.clone(),
            target.to_owned(),
            status.clone(),
            Rc::clone(pin),
            Rc::clone(&marked),
        );
        let found = Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(
            move |_, observer: web_sys::MutationObserver| {
                let Some(element) = routed_element(&document, &target) else {
                    return;
                };
                observer.disconnect();
                if element.matches(".mosaic-message-card").unwrap_or(false) {
                    let _ = element.set_attribute("data-card-route-selected", "true");
                    *mark.borrow_mut() = Some(element.clone());
                }
                *pin.borrow_mut() = reach(&element, &target, &status);
            },
        );
        let observer = web_sys::MutationObserver::new(found.as_ref().unchecked_ref()).ok()?;
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        // A rendered card becomes reachable when its layout commits.
        options.set_attribute_filter(&js_sys::Array::of1(&LAYOUT_PHASE.into()));
        observer.observe_with_options(&body, &options).ok()?;
        Some(Self {
            observer,
            _found: found,
            marked,
        })
    }
}

impl Drop for Seek {
    fn drop(&mut self) {
        self.observer.disconnect();
        if let Some(element) = self.marked.borrow_mut().take() {
            let _ = element.remove_attribute("data-card-route-selected");
        }
    }
}

/// One route target being brought into view: sought until it renders, then
/// pinned until the reader scrolls. Dropped when the route moves on.
struct Hold {
    target: String,
    _seek: Option<Seek>,
    _pin: Rc<RefCell<Option<Pin>>>,
}

/// Focus a card named by the current route once retained Chat state is ready,
/// unless history returned to an entry a quote link left: then the card that
/// held the link returns to where the reader saw it.
///
/// A target not yet rendered is sought as staged hydration renders it, and a
/// reached card stays pinned while that hydration grows its plane, until the
/// reader scrolls it.
#[hook]
pub(super) fn use_target(
    target: Option<String>,
    surface: &SurfaceData,
    status: UseStateHandle<Option<Status>>,
    visible: bool,
) {
    let restored = use_mut_ref(|| None::<f64>);
    let held = use_mut_ref(|| None::<Hold>);
    let (ready, revision) = {
        let surface = surface.read();
        let runtime = surface.as_ref();
        (
            runtime.and_then(CanvasRuntime::lanes).is_some(),
            runtime
                .and_then(CanvasRuntime::resume)
                .map(|cursor| cursor.seq.to_string()),
        )
    };
    use_effect_with(
        (target, visible, ready, revision),
        move |(target, visible, ready, _)| {
            let mut selected = None;
            let window = web_sys::window();
            let document = window.as_ref().and_then(web_sys::Window::document);
            let anchor = window
                .as_ref()
                .and_then(|window| window.history().ok())
                .and_then(|history| ReturnAnchor::read(&history))
                .filter(|_| *visible && *ready);
            let returned = match (&anchor, &document) {
                (Some(anchor), Some(document)) => returned_to(&restored, anchor, document),
                _ => {
                    // Leaving the entry lets a later Back restore it again.
                    restored.borrow_mut().take();
                    false
                }
            };
            if returned
                || held.borrow().as_ref().is_some_and(|hold| {
                    Some(hold.target.as_str()) != target.as_deref() || !*visible
                })
            {
                held.borrow_mut().take();
            }
            if let Some(target) = target.as_deref().filter(|_| *visible) {
                if *ready {
                    let settled = returned
                        || status
                            .as_ref()
                            .is_some_and(|settled| settled.target == target && settled.available);
                    let element = document
                        .as_ref()
                        .and_then(|document| routed_element(document, target));
                    if let Some(element) = element
                        .as_ref()
                        .filter(|element| element.matches(".mosaic-message-card").unwrap_or(false))
                    {
                        let _ = element.set_attribute("data-card-route-selected", "true");
                        selected = Some(element.clone());
                    }
                    if !settled {
                        if let Some(element) = element {
                            let pin = Rc::new(RefCell::new(reach(&element, target, &status)));
                            *held.borrow_mut() = Some(Hold {
                                target: target.to_owned(),
                                _seek: None,
                                _pin: pin,
                            });
                        } else {
                            status.set(Some(Status {
                                target: target.to_owned(),
                                available: false,
                            }));
                            if held.borrow().is_none()
                                && let Some(document) = &document
                            {
                                let pin = Rc::new(RefCell::new(None));
                                let seek = Seek::start(document, target, &status, &pin);
                                *held.borrow_mut() = Some(Hold {
                                    target: target.to_owned(),
                                    _seek: seek,
                                    _pin: pin,
                                });
                            }
                        }
                    } else if returned && status.as_ref().is_none_or(|known| known.target != target)
                    {
                        // The route's own card counts as reached, so a later
                        // stream advance does not yank the reader to it.
                        status.set(Some(Status {
                            target: target.to_owned(),
                            available: element.is_some(),
                        }));
                    }
                }
            } else if status.is_some() {
                status.set(None);
            }
            move || {
                if let Some(element) = selected {
                    let _ = element.remove_attribute("data-card-route-selected");
                }
            }
        },
    );
}

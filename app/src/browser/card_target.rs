use std::cell::RefCell;
use std::rc::Rc;

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

/// Focus a card named by the current route once retained Chat state is ready,
/// unless history returned to an entry a quote link left: then the card that
/// held the link returns to where the reader saw it.
///
/// Missing cards are retried when the stream cursor advances because the
/// target may arrive during staged hydration.
#[hook]
pub(super) fn use_target(
    target: Option<String>,
    surface: &SurfaceData,
    status: UseStateHandle<Option<Status>>,
    visible: bool,
) {
    let restored = use_mut_ref(|| None::<f64>);
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
            if let Some(target) = target.as_deref().filter(|_| *visible) {
                if *ready {
                    let settled = returned
                        || status
                            .as_ref()
                            .is_some_and(|settled| settled.target == target && settled.available);
                    let element = document
                        .as_ref()
                        .and_then(|document| document.get_element_by_id(target));
                    if let Some(element) = element
                        .as_ref()
                        .filter(|element| element.matches(".mosaic-message-card").unwrap_or(false))
                    {
                        let _ = element.set_attribute("data-card-route-selected", "true");
                        selected = Some(element.clone());
                    }
                    if !settled {
                        let available = element.is_some();
                        if let Some(element) = element {
                            element.scroll_into_view();
                            if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
                                let _ = element.focus();
                            }
                        }
                        status.set(Some(Status {
                            target: target.to_owned(),
                            available,
                        }));
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

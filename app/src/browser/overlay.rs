//! One Escape owner at a time for Canvas modals and disclosures.
//!
//! Each dismissible surface registers while it is active. One document
//! listener hands each Escape press to the most recently activated owner and
//! consumes it, so repeated presses walk the visible stack one owner at a time.
//! A native modal dialog is always above every registered owner: while one is
//! open, its own cancel handles Escape.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;
use yew::prelude::*;

type Dismissal = Rc<RefCell<Callback<()>>>;

/// The document's Escape owners, most recently activated last.
struct EscapeStack {
    next: u64,
    active: Vec<(u64, Dismissal)>,
    _listener: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
}

thread_local! {
    static ESCAPE: RefCell<EscapeStack> = RefCell::new(EscapeStack::install());
}

impl EscapeStack {
    fn install() -> Self {
        let listener = web_sys::window()
            .and_then(|window| window.document())
            .map(|document| {
                let observed = document.clone();
                let key = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                    move |event: web_sys::KeyboardEvent| {
                        if event.key() != "Escape"
                            || event.default_prevented()
                            || event.is_composing()
                            || event.repeat()
                            || native_modal_open(&observed)
                        {
                            return;
                        }
                        if dismiss_topmost() {
                            event.prevent_default();
                        }
                    },
                );
                let _ = document
                    .add_event_listener_with_callback("keydown", key.as_ref().unchecked_ref());
                key
            });
        Self {
            next: 0,
            active: Vec::new(),
            _listener: listener,
        }
    }
}

/// Remove the topmost owner before it runs, so its dismissal is consumed even
/// when the resulting render lands after the next key press.
fn dismiss_topmost() -> bool {
    let Some((_, dismissal)) = ESCAPE.with_borrow_mut(|stack| stack.active.pop()) else {
        return false;
    };
    let dismissal = dismissal.borrow().clone();
    dismissal.emit(());
    true
}

/// Own Escape while `active`; the latest `dismiss` runs on the owner's turn.
#[hook]
pub(super) fn use_escape_owner(active: bool, dismiss: Callback<()>) {
    let current = use_mut_ref(|| dismiss.clone());
    *current.borrow_mut() = dismiss;
    use_effect_with(active, move |active| {
        let owner = active.then(|| {
            ESCAPE.with_borrow_mut(|stack| {
                let owner = stack.next;
                stack.next += 1;
                stack.active.push((owner, current));
                owner
            })
        });
        move || {
            if let Some(owner) = owner {
                ESCAPE.with_borrow_mut(|stack| stack.active.retain(|(id, _)| *id != owner));
            }
        }
    });
}

fn native_modal_open(document: &web_sys::Document) -> bool {
    document
        .query_selector("dialog:modal")
        .ok()
        .flatten()
        .is_some()
}

/// Whether focus may return to `element`: it is still in the document,
/// rendered, and outside every inert subtree.
pub(super) fn returnable(element: &web_sys::Element) -> bool {
    element.is_connected()
        && element
            .dyn_ref::<web_sys::HtmlElement>()
            .is_some_and(|element| element.offset_width() > 0 || element.offset_height() > 0)
        && element.closest("[inert]").ok().flatten().is_none()
}

/// Whether the operator asked for reduced motion. It removes slides, never
/// the ownership or completion of the layer that would have slid.
pub(super) fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_none_or(|query| query.matches())
}

const CLOSE_TRANSITION_EVENTS: [&str; 2] = ["transitionend", "transitioncancel"];

/// Finish a closing layer when its own `transform` slide ends or is
/// canceled, or at once when no rendered layer is left to run the slide.
/// Completion always has an actual boundary; nothing waits on a clock.
#[hook]
pub(super) fn use_close_transition(
    closing: bool,
    rendered: bool,
    layer: NodeRef,
    complete: Callback<()>,
) {
    use_effect_with(
        (closing, rendered, layer, complete),
        move |(closing, rendered, layer, complete)| {
            let element = (*closing && *rendered)
                .then(|| layer.cast::<web_sys::Element>())
                .flatten()
                .filter(|element| element.is_connected());
            if *closing && element.is_none() {
                complete.emit(());
            }
            let registration = element.and_then(|element| {
                let target = element.clone();
                let complete = complete.clone();
                let callback =
                    Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                        let property = js_sys::Reflect::get(
                            event.as_ref(),
                            &JsValue::from_str("propertyName"),
                        )
                        .ok()
                        .and_then(|value| value.as_string());
                        let from_layer = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                            .is_some_and(|element| element == target);
                        if from_layer && property.as_deref() == Some("transform") {
                            complete.emit(());
                        }
                    });
                for kind in CLOSE_TRANSITION_EVENTS {
                    element
                        .add_event_listener_with_callback(kind, callback.as_ref().unchecked_ref())
                        .ok()?;
                }
                Some((element, callback))
            });
            move || {
                if let Some((element, callback)) = registration {
                    for kind in CLOSE_TRANSITION_EVENTS {
                        let _ = element.remove_event_listener_with_callback(
                            kind,
                            callback.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        },
    );
}

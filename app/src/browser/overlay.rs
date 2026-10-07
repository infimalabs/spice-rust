//! Shared ownership rules for app overlays.
//!
//! Escape dispatch walks active dismissible surfaces from newest to oldest.
//! Native modal dialogs retain their own Escape handling. Shared returnability
//! checks keep focus restoration on connected, visible, non-inert controls.

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
/// Return focus without moving the page, but only while the original control
/// remains a rendered, non-inert member of this document.
pub(super) fn return_focus(element: &web_sys::Element) {
    if returnable(element)
        && let Some(element) = element.dyn_ref::<web_sys::HtmlElement>()
    {
        let options = web_sys::FocusOptions::new();
        options.set_prevent_scroll(true);
        let _ = element.focus_with_options(&options);
    }
}

/// Close the innermost open disclosure containing this click. Confirmations
/// use this after acting; focus follows the click rather than returning to the
/// disclosure button.
pub(super) fn dismiss_disclosure_from_click(event: &MouseEvent) {
    if let Some(disclosure) = disclosure(event.target()) {
        dismiss_disclosure(&disclosure, false);
    }
}

/// Give nested disclosure surfaces one Escape order: confirmation, lane menu,
/// then the document-level overlay stack.
pub(super) fn dismiss_disclosure_on_escape(event: KeyboardEvent) {
    if event.key() != "Escape"
        || event.default_prevented()
        || event.is_composing()
        || event.repeat()
    {
        return;
    }
    let Some(disclosure) = disclosure(event.target()) else {
        return;
    };
    event.prevent_default();
    event.stop_propagation();
    dismiss_disclosure(&disclosure, true);
}

fn disclosure(target: Option<web_sys::EventTarget>) -> Option<web_sys::Element> {
    target
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| {
            target
                .closest("[data-app-overlay-disclosure][open]")
                .ok()
                .flatten()
        })
}

fn dismiss_disclosure(disclosure: &web_sys::Element, restore_focus: bool) {
    let _ = disclosure.remove_attribute("open");
    if restore_focus
        && let Some(summary) = disclosure.query_selector(":scope > summary").ok().flatten()
    {
        return_focus(&summary);
    }
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
fn transform_transition_running(element: &web_sys::Element) -> bool {
    let Ok(get_animations) =
        js_sys::Reflect::get(element.as_ref(), &JsValue::from_str("getAnimations"))
    else {
        return false;
    };
    let Ok(get_animations) = get_animations.dyn_into::<js_sys::Function>() else {
        return false;
    };
    let Ok(animations) = get_animations.call0(element.as_ref()) else {
        return false;
    };
    js_sys::Array::from(&animations).iter().any(|animation| {
        js_sys::Reflect::get(&animation, &JsValue::from_str("transitionProperty"))
            .ok()
            .and_then(|property| property.as_string())
            .as_deref()
            == Some("transform")
    })
}

/// Finish a closing layer when its own `transform` slide ends or is canceled,
/// or at once when no transform transition is running. Completion always has
/// an actual boundary; nothing waits on a clock.
#[hook]
pub(super) fn use_close_transition(
    closing: bool,
    rendered: bool,
    layer: NodeRef,
    complete: Callback<()>,
) {
    let closing_now = use_mut_ref(|| closing);
    *closing_now.borrow_mut() = closing;
    let finish_without_transition = complete.clone();
    let transition_layer = layer.clone();
    use_effect_with((closing, rendered), move |(closing, rendered)| {
        let transition_running = *rendered
            && transition_layer
                .cast::<web_sys::Element>()
                .is_some_and(|element| transform_transition_running(&element));
        if *closing && !transition_running {
            finish_without_transition.emit(());
        }
    });
    use_effect_with(
        (rendered, layer, complete),
        move |(rendered, layer, complete)| {
            // Listen throughout presentation. Registering only after `closing`
            // renders can miss a zero-duration or already-canceled transition.
            let registration = rendered
                .then(|| layer.cast::<web_sys::Element>())
                .flatten()
                .filter(|element| element.is_connected())
                .and_then(|element| {
                    let target = element.clone();
                    let complete = complete.clone();
                    let closing_now = closing_now.clone();
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
                            if *closing_now.borrow()
                                && from_layer
                                && property.as_deref() == Some("transform")
                            {
                                complete.emit(());
                            }
                        });
                    for kind in CLOSE_TRANSITION_EVENTS {
                        element
                            .add_event_listener_with_callback(
                                kind,
                                callback.as_ref().unchecked_ref(),
                            )
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

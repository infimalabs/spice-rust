//! The Canvas menu disclosure: Escape, outside pointer, and outside focus
//! close it while it is open, and choosing a destination in it ends it.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;
use yew::prelude::*;

use super::overlay;

/// End the menu from inside it and hand focus back to its trigger, so focus
/// never stays on a control the closed menu no longer shows.
pub(super) fn close(open: &UseStateHandle<bool>) {
    open.set(false);
    if let Some(trigger) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector(".app-menu > summary")
                .ok()
                .flatten()
        })
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = trigger.focus();
    }
}

#[hook]
pub(super) fn use_dismissal(open: UseStateHandle<bool>) {
    let dismiss = {
        let open = open.clone();
        Callback::from(move |()| close(&open))
    };
    overlay::use_escape_owner(*open, dismiss);
    use_effect_with(*open, move |is_open| {
        let registration = (*is_open)
            .then(|| {
                let document = web_sys::window()?.document()?;
                if let Some(first_destination) = document
                    .query_selector(".app-navigation .team-mode")
                    .ok()
                    .flatten()
                    .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = first_destination.focus();
                }
                let pointer_open = open.clone();
                let pointer = Closure::<dyn FnMut(web_sys::Event)>::new(move |event| {
                    if !contains(&event) {
                        pointer_open.set(false);
                    }
                });
                let focus_open = open.clone();
                let focus = Closure::<dyn FnMut(web_sys::Event)>::new(move |event| {
                    if !contains(&event) {
                        focus_open.set(false);
                    }
                });
                let _ = document.add_event_listener_with_callback(
                    "pointerdown",
                    pointer.as_ref().unchecked_ref(),
                );
                let _ = document
                    .add_event_listener_with_callback("focusin", focus.as_ref().unchecked_ref());
                Some((document, pointer, focus))
            })
            .flatten();
        move || {
            if let Some((document, pointer, focus)) = registration {
                let _ = document.remove_event_listener_with_callback(
                    "pointerdown",
                    pointer.as_ref().unchecked_ref(),
                );
                let _ = document
                    .remove_event_listener_with_callback("focusin", focus.as_ref().unchecked_ref());
            }
        }
    });
}

fn contains(event: &web_sys::Event) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|target| target.closest(".app-menu").ok().flatten().is_some())
}

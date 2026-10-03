//! The team controls frame, which alone follows typed drafts.
//!
//! Typed text lives in its composer element until sent, so drafting is heard
//! where it happens: every composer change raises an `input` event that
//! bubbles to the team form, including values the app writes itself. The
//! frame keeps whether any composer holds text and re-renders only itself,
//! and only when that answer flips, so a keystroke never renders the app.

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

use super::CanonicalUuid;

#[derive(Properties, PartialEq)]
pub(in crate::browser::watchband) struct FrameProps {
    /// No agent runs and no quote or image is drafted.
    pub dormant: bool,
    /// Send is admissible apart from whether anything is drafted.
    pub ready: bool,
    /// The team's members; a member joining or leaving changes its composers.
    pub lanes: Vec<CanonicalUuid>,
    pub speech: &'static str,
    pub lifetime: &'static str,
    pub lifetime_pending: bool,
    pub send_class: Classes,
    pub send_title: String,
    pub send_label: &'static str,
    pub children: Html,
}

#[function_component(ControlsFrame)]
pub(in crate::browser::watchband) fn controls_frame(props: &FrameProps) -> Html {
    let root = use_node_ref();
    let typed = use_state_eq(|| false);
    {
        let root = root.clone();
        let typed = typed.clone();
        use_effect_with((), move |()| {
            let form = form_of(&root);
            let listener = form.as_ref().map(|form| {
                let scanned = form.clone();
                let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
                    let Some(textarea) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                    else {
                        return;
                    };
                    // Text in the changed composer settles it; only one that
                    // turned blank asks whether any other still holds text.
                    typed.set(holds_text(&textarea) || any_holds_text(&scanned));
                });
                form.add_event_listener_with_callback("input", callback.as_ref().unchecked_ref())
                    .expect("team form accepts its drafting listener");
                callback
            });
            move || {
                if let (Some(form), Some(callback)) = (form, listener) {
                    let _ = form.remove_event_listener_with_callback(
                        "input",
                        callback.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }
    {
        // A member joining or leaving, or quotes and images coming or going,
        // adds or removes composers without any of them raising input.
        let root = root.clone();
        let typed = typed.clone();
        use_effect_with((props.lanes.clone(), props.dormant), move |_| {
            typed.set(form_of(&root).is_some_and(|form| any_holds_text(&form)));
        });
    }
    let idle = props.dormant && !*typed;
    html! {
        <aside
            ref={root}
            class="team-composer-controls"
            data-speech-mode={props.speech}
            data-lifetime={props.lifetime}
            data-lifetime-pending={props.lifetime_pending.to_string()}
            data-idle={idle.to_string()}
        >
            {props.children.clone()}
            <button
                class={props.send_class.clone()}
                type="submit"
                disabled={idle || !props.ready}
                title={props.send_title.clone()}
            >{props.send_label}</button>
        </aside>
    }
}

fn form_of(root: &NodeRef) -> Option<web_sys::Element> {
    root.cast::<web_sys::Element>()?
        .closest("form")
        .ok()
        .flatten()
}

fn holds_text(textarea: &web_sys::HtmlTextAreaElement) -> bool {
    !textarea.value().trim().is_empty()
}

fn any_holds_text(form: &web_sys::Element) -> bool {
    let Ok(textareas) = form.query_selector_all("textarea") else {
        return false;
    };
    (0..textareas.length()).any(|index| {
        textareas
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            .is_some_and(|textarea| holds_text(&textarea))
    })
}

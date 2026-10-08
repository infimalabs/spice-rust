//! The team controls frame, which alone follows typed drafts.
//!
//! Typed text lives in its composer element until sent, so drafting is heard
//! where it happens: every composer change raises an `input` event that
//! bubbles to the team form, including values the app writes itself. The
//! frame keeps which members a press would send to and re-renders only itself,
//! and only when that answer changes, so a keystroke never renders the app.

use std::fmt::Write as _;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

/// One member as the send button sees it, in member order.
#[derive(Clone, PartialEq)]
pub(in crate::browser::watchband) struct SendSlot {
    /// The member's lane, which names its composer section.
    pub lane: String,
    pub accent: usize,
    /// A press can deliver to this member, waking it if it must.
    pub reachable: bool,
    /// A quote or image is drafted, which sends without any text.
    pub attached: bool,
}

#[derive(Properties, PartialEq)]
pub(in crate::browser::watchband) struct FrameProps {
    /// No agent runs and no quote or image is drafted.
    pub dormant: bool,
    /// Send is admissible apart from whether anything is drafted.
    pub ready: bool,
    /// A topology change may add or remove this team's composers.
    pub topology_revision: u64,
    pub slots: Vec<SendSlot>,
    pub speech: &'static str,
    pub lifetime: &'static str,
    pub lifetime_pending: bool,
    /// The round button row beneath send.
    pub turbo: Html,
    pub children: Html,
}

#[derive(Clone, Default, PartialEq)]
struct Drafting {
    /// Some composer holds typed text.
    typed: bool,
    /// Accents of the members a press would send to, in member order.
    lit: Vec<usize>,
}

#[function_component(ControlsFrame)]
pub(in crate::browser::watchband) fn controls_frame(props: &FrameProps) -> Html {
    let root = use_node_ref();
    let send = use_node_ref();
    let drafting = use_state_eq(Drafting::default);
    {
        // A topology change, or quotes and images coming or going, may add or
        // remove composers without any of them raising input, so the answer
        // is read again whenever the slots change as well as on every edit.
        let root = root.clone();
        let drafting = drafting.clone();
        use_effect_with(
            (props.slots.clone(), props.topology_revision),
            move |(slots, _)| {
                let form = form_of(&root);
                if let Some(form) = &form {
                    drafting.set(read_drafting(form, slots));
                }
                let listener = form.map(|form| {
                    let scanned = form.clone();
                    let slots = slots.clone();
                    let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
                        let edited = event.target().is_some_and(|target| {
                            target.has_type::<web_sys::HtmlTextAreaElement>()
                        });
                        if edited {
                            drafting.set(read_drafting(&scanned, &slots));
                        }
                    });
                    form.add_event_listener_with_callback(
                        "input",
                        callback.as_ref().unchecked_ref(),
                    )
                    .expect("team form accepts its drafting listener");
                    (form, callback)
                });
                move || {
                    if let Some((form, callback)) = listener {
                        let _ = form.remove_event_listener_with_callback(
                            "input",
                            callback.as_ref().unchecked_ref(),
                        );
                    }
                }
            },
        );
    }
    let idle = props.dormant && !drafting.typed;
    // A send that is refused outright reaches no one, so it lights nothing.
    let lit = if props.ready {
        drafting.lit.clone()
    } else {
        Vec::new()
    };
    {
        // The rings are a gradient over the lit accents; the app CSP refuses
        // style attributes, so the value reaches the button through CSSOM.
        let send = send.clone();
        use_effect_with(lit.clone(), move |lit| {
            if let Some(button) = send.cast::<web_sys::HtmlElement>() {
                let style = button.style();
                let _ = if lit.is_empty() {
                    style.remove_property("--team-send-rings").map(drop)
                } else {
                    style.set_property("--team-send-rings", &rings(lit))
                };
            }
        });
    }
    let title = match lit.len() {
        0 => "Send".to_owned(),
        1 => "Send to 1 agent".to_owned(),
        count => format!("Send to {count} agents"),
    };
    html! {
        <aside
            ref={root}
            class="app-glass-control-surface team-composer-controls"
            data-speech-mode={props.speech}
            data-lifetime={props.lifetime}
            data-lifetime-pending={props.lifetime_pending.to_string()}
            data-idle={idle.to_string()}
        >
            <button
                ref={send}
                class="team-send"
                type="submit"
                data-lit={(!lit.is_empty()).to_string()}
                data-rings={lit.len().to_string()}
                disabled={idle || !props.ready}
                {title}
            >{"Send"}</button>
            {props.turbo.clone()}
            {props.children.clone()}
        </aside>
    }
}

/// Concentric bands filling the button, member zero at the core. Each band is
/// lit at its inner edge and shaded toward its outer edge, so neighbours read
/// as separate rings even when two members share an accent.
fn rings(accents: &[usize]) -> String {
    let count = accents.len() as f64;
    let mut gradient = String::from("radial-gradient(circle closest-side");
    for (index, accent) in accents.iter().enumerate() {
        let inner = index as f64 / count * 100.0;
        let outer = (index + 1) as f64 / count * 100.0;
        let crest = inner + (outer - inner) * 0.45;
        let _ = write!(
            gradient,
            ", color-mix(in srgb, var(--accent-{accent}) 78%, var(--app-highlight-source)) {inner:.2}%\
             , var(--accent-{accent}) {crest:.2}%\
             , color-mix(in srgb, var(--accent-{accent}) 72%, var(--app-shadow-source)) {outer:.2}%"
        );
    }
    gradient.push(')');
    gradient
}

fn form_of(root: &NodeRef) -> Option<web_sys::Element> {
    root.cast::<web_sys::Element>()?
        .closest("form")
        .ok()
        .flatten()
}

/// What a press would send: a reachable member is lit when its composer holds
/// text or a drafted quote or image, the same content a submit prepares.
fn read_drafting(form: &web_sys::Element, slots: &[SendSlot]) -> Drafting {
    let mut drafting = Drafting::default();
    for slot in slots {
        let typed = form
            .query_selector(&format!(".team-agent[data-agent-lane=\"{}\"]", slot.lane))
            .ok()
            .flatten()
            .is_some_and(|section| holds_text(&section));
        drafting.typed |= typed;
        if slot.reachable && (typed || slot.attached) {
            drafting.lit.push(slot.accent);
        }
    }
    drafting
}

fn holds_text(section: &web_sys::Element) -> bool {
    let Ok(textareas) = section.query_selector_all("textarea") else {
        return false;
    };
    (0..textareas.length()).any(|index| {
        textareas
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            .is_some_and(|textarea| !textarea.value().trim().is_empty())
    })
}

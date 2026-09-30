//! The one transient result toast: a frosted status pill that shows the
//! latest action result, holds a pending one until its outcome replaces it,
//! and fades a settled one after a fixed dwell.
//!
//! The Canvas mounts one instance at its root and hands its [`Toaster`] to
//! every component through context. That instance is a manual popover raised
//! with each result, so it shows above an open modal dialog such as task
//! detail. The lightbox renders its own instance inside its dialog, beside the
//! image its actions report on.

use gloo_timers::callback::Timeout;
use yew::prelude::*;

/// How long a settled result stays up before the toast fades.
const DISMISS_MILLIS: u32 = 4_000;

/// Replaces the result a toast shows.
#[derive(Clone, PartialEq)]
pub(super) struct Toaster(Callback<(AttrValue, bool)>);

impl Toaster {
    /// Show a result that stays until another replaces it.
    pub(super) fn pending(&self, message: impl Into<AttrValue>) {
        self.0.emit((message.into(), false));
    }

    /// Show a final result, which fades on its own.
    pub(super) fn settled(&self, message: impl Into<AttrValue>) {
        self.0.emit((message.into(), true));
    }

    /// Remove the current result without announcing a replacement.
    pub(super) fn clear(&self) {
        self.0.emit((AttrValue::default(), false));
    }
}

/// The latest result, whether it is still showing, and the handle that
/// replaces it. A settled result arms its own dismissal; replacing it cancels
/// any earlier result's.
#[hook]
pub(super) fn use_toast() -> (UseStateHandle<AttrValue>, UseStateHandle<bool>, Toaster) {
    let text = use_state_eq(AttrValue::default);
    let shown = use_state_eq(|| false);
    let dismissal = use_mut_ref(|| None::<Timeout>);
    let notify = use_callback(
        (text.clone(), shown.clone()),
        move |(message, settled): (AttrValue, bool), (text, shown)| {
            text.set(message);
            shown.set(true);
            let shown = shown.clone();
            *dismissal.borrow_mut() =
                settled.then(|| Timeout::new(DISMISS_MILLIS, move || shown.set(false)));
        },
    );
    (text, shown, Toaster(notify))
}

/// The root toast's handle.
#[hook]
pub(super) fn use_toaster() -> Toaster {
    use_context::<Toaster>().expect("the Canvas root mounts the toast")
}

/// The toast pill. Hidden text stays in place so the pill fades out rather
/// than collapsing first. A `raised` toast is a manual popover in the top
/// layer; the one inside a dialog is already there.
pub(super) fn render_toast(message: AttrValue, shown: bool, raised: Option<NodeRef>) -> Html {
    let popover = raised.is_some().then_some("manual");
    html! {
        <p
            ref={raised.unwrap_or_default()} class="toast" role="status" {popover}
            data-shown={shown.to_string()}
        >{message}</p>
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct ToastProviderProps {
    pub children: Html,
}

/// Mount the root toast and give its handle to everything below.
///
/// A modal dialog (task detail, the lightbox) lives in the top layer, above
/// any ordinary element. The root toast is a manual popover, so it lives
/// there too, and each result re-shows it so it stacks above whichever
/// dialog is open at that moment.
#[function_component(ToastProvider)]
pub(super) fn toast_provider(props: &ToastProviderProps) -> Html {
    let (text, shown, toaster) = use_toast();
    let node = use_node_ref();
    let raising = use_memo((toaster, node.clone()), |(toaster, node)| {
        let (inner, node) = (toaster.0.clone(), node.clone());
        Toaster(Callback::from(move |result| {
            if let Some(element) = node.cast::<web_sys::Element>() {
                raise(&element);
            }
            inner.emit(result);
        }))
    });
    html! {
        <ContextProvider<Toaster> context={(*raising).clone()}>
            {props.children.clone()}
            {render_toast((*text).clone(), *shown && !text.is_empty(), Some(node))}
        </ContextProvider<Toaster>>
    }
}

/// Move a manual popover to the top of the top layer. The Popover API is
/// reached reflectively, as the clipboard is, so the build needs no unstable
/// `web-sys` flag.
fn raise(element: &web_sys::Element) {
    let call = |method: &str| {
        js_sys::Reflect::get(element, &wasm_bindgen::JsValue::from_str(method))
            .ok()
            .and_then(|function| wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(function).ok())
            .map(|function| function.call0(element))
    };
    if element.matches(":popover-open").unwrap_or(false) {
        call("hidePopover");
    }
    call("showPopover");
}

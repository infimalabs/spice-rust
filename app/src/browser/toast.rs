//! Transient result toasts: frosted status pills that show the latest owned
//! result, hold a pending one until its outcome replaces it, and fade a
//! settled one after a fixed dwell.
//!
//! The app mounts a shared root instance and hands its [`Toaster`] to
//! components through context. A workflow that must retain independent
//! pending state can mount its own raised instance. Manual popovers show above
//! an open modal dialog; the lightbox instead renders its instance inside its
//! dialog beside the image its actions report on.

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

#[derive(Clone, PartialEq)]
struct TaskToaster(Toaster);

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

/// An independently owned toast raised above modal dialogs.
#[hook]
pub(super) fn use_raised_toast() -> (
    UseStateHandle<AttrValue>,
    UseStateHandle<bool>,
    Toaster,
    NodeRef,
) {
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
    (text, shown, (*raising).clone(), node)
}

/// The root toast's handle.
#[hook]
pub(super) fn use_toaster() -> Toaster {
    use_context::<Toaster>().expect("the app root mounts the toast")
}

/// The task lifecycle toast's independently owned handle.
#[hook]
pub(super) fn use_task_toaster() -> Toaster {
    use_context::<TaskToaster>()
        .expect("the app root mounts the task toast")
        .0
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

/// A raised toast with an additional placement class.
pub(super) fn render_raised_toast(
    message: AttrValue,
    shown: bool,
    node: NodeRef,
    class: &'static str,
) -> Html {
    html! {
        <p
            ref={node} class={classes!("toast", class)} role="status" popover="manual"
            data-shown={shown.to_string()}
        >{message}</p>
    }
}

#[derive(Properties, PartialEq)]
pub(super) struct ToastProviderProps {
    pub children: Html,
}

/// Mount the independent shared-result and task-lifecycle toasts.
///
/// Both are manual popovers in the top layer, above modal task detail. Each
/// result re-shows only its owning toast, so one lifecycle cannot dismiss the
/// other's message.
#[function_component(ToastProvider)]
pub(super) fn toast_provider(props: &ToastProviderProps) -> Html {
    let (text, shown, toaster, node) = use_raised_toast();
    let (task_text, task_shown, task_toaster, task_node) = use_raised_toast();
    html! {
        <ContextProvider<Toaster> context={toaster}>
            <ContextProvider<TaskToaster> context={TaskToaster(task_toaster)}>
                {props.children.clone()}
                {render_toast((*text).clone(), *shown && !text.is_empty(), Some(node))}
                {render_raised_toast(
                    (*task_text).clone(),
                    *task_shown && !task_text.is_empty(),
                    task_node,
                    "toast--task-feedback",
                )}
            </ContextProvider<TaskToaster>>
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

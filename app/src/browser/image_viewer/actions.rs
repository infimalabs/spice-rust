use super::*;
use gloo_timers::callback::Timeout;
use wasm_bindgen_futures::{JsFuture, spawn_local};

mod export;

#[derive(Clone, Copy)]
enum Action {
    Copy,
    Download,
}

#[derive(Properties, PartialEq)]
pub(super) struct ActionsProps {
    pub source: AttrValue,
    pub name: AttrValue,
    pub onclose: Callback<()>,
    pub image_status: AttrValue,
}

/// How long a settled action result stays up before its toast fades.
const NOTICE_DISMISS_MILLIS: u32 = 4_000;

/// The latest action result, whether it is still showing, and a callback that
/// replaces it. A settled result arms its own dismissal; a pending one
/// ("Copying image") stays until its outcome replaces it.
#[hook]
fn use_notice() -> (
    UseStateHandle<AttrValue>,
    UseStateHandle<bool>,
    Callback<(AttrValue, bool)>,
) {
    let text = use_state_eq(AttrValue::default);
    let shown = use_state_eq(|| false);
    let dismissal = use_mut_ref(|| None::<Timeout>);
    let notify = use_callback(
        (text.clone(), shown.clone()),
        move |(message, settled): (AttrValue, bool), (text, shown)| {
            text.set(message);
            shown.set(true);
            let shown = shown.clone();
            // Replacing the handle cancels any earlier result's dismissal.
            *dismissal.borrow_mut() =
                settled.then(|| Timeout::new(NOTICE_DISMISS_MILLIS, move || shown.set(false)));
        },
    );
    (text, shown, notify)
}

#[function_component(Actions)]
pub(super) fn actions(props: &ActionsProps) -> Html {
    let busy = use_state_eq(|| false);
    let (status, shown, notify) = use_notice();
    let copy_refusal = use_memo((), |()| export::copy_refusal());
    let exportable = use_memo(props.source.clone(), |source| export::readable(source));
    let perform = use_callback(
        (
            props.source.clone(),
            props.name.clone(),
            busy.clone(),
            notify,
            copy_refusal.clone(),
            exportable.clone(),
        ),
        |action: Action, (source, name, busy, notify, copy_refusal, exportable)| {
            // An unavailable action stays activatable so the operator learns
            // why only when asking, rather than from a standing notice.
            let refusal = if !**exportable {
                Some("This image's origin does not allow copy or download.".to_owned())
            } else if matches!(action, Action::Copy) {
                (**copy_refusal).clone()
            } else {
                None
            };
            if let Some(refusal) = refusal {
                notify.emit((refusal.into(), true));
                return;
            }
            busy.set(true);
            notify.emit((
                match action {
                    Action::Copy => "Copying image",
                    Action::Download => "Preparing download",
                }
                .into(),
                false,
            ));
            // Clipboard.write must start inside the trusted click, before awaiting conversion.
            let operation = match action {
                Action::Copy => export::copy(source.clone()),
                Action::Download => Ok(export::download(source.clone(), name.clone())),
            };
            let busy = busy.clone();
            let notify = notify.clone();
            spawn_local(async move {
                let result = match operation {
                    Ok(promise) => JsFuture::from(promise).await,
                    Err(error) => Err(error),
                };
                busy.set(false);
                let message = match result {
                    Ok(_) => match action {
                        Action::Copy => "Image copied as PNG".into(),
                        Action::Download => "Download started".into(),
                    },
                    Err(error) => {
                        let message = js_sys::Reflect::get(&error, &JsValue::from_str("message"))
                            .ok()
                            .and_then(|value| value.as_string())
                            .or_else(|| error.as_string())
                            .unwrap_or_else(|| "Browser refused the image action".to_owned());
                        format!(
                            "{} failed: {message}",
                            match action {
                                Action::Copy => "Copy",
                                Action::Download => "Download",
                            }
                        )
                        .into()
                    }
                };
                notify.emit((message, true));
            });
        },
    );
    let copy = use_callback(perform.clone(), |(), perform| perform.emit(Action::Copy));
    let download = use_callback(perform, |(), perform| perform.emit(Action::Download));
    let copy_button = use_button(copy);
    let download_button = use_button(download);
    let close_button = use_button(props.onclose.clone());
    // A showing action result outranks the image's own load state; once it
    // fades, its text stays so the toast leaves without collapsing first.
    let action_shown = *shown && !status.is_empty();
    let (message, visible) = if action_shown || props.image_status.is_empty() {
        ((*status).clone(), action_shown)
    } else {
        (props.image_status.clone(), true)
    };
    let copy_unavailable = !*exportable || copy_refusal.is_some();
    let download_unavailable = !*exportable;
    html! {
        <>
            <div class="image-lightbox-actions">
                <button ref={copy_button} type="button" disabled={*busy} aria-disabled={copy_unavailable.to_string()} aria-label="Copy image" title="Copy image as PNG">{"Copy"}</button>
                <button ref={download_button} type="button" disabled={*busy} aria-disabled={download_unavailable.to_string()} aria-label="Download image" title="Download original image">{"Download"}</button>
                <button ref={close_button} type="button" autofocus=true aria-label="Close full-size image">{"Close"}</button>
            </div>
            <p class="image-lightbox-status" role="status" data-shown={visible.to_string()}>{message}</p>
        </>
    }
}

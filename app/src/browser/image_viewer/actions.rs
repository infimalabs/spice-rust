use super::*;
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

#[function_component(Actions)]
pub(super) fn actions(props: &ActionsProps) -> Html {
    let busy = use_state_eq(|| false);
    let status = use_state_eq(AttrValue::default);
    let copy_supported = use_memo((), |()| export::clipboard_available());
    let exportable = use_memo(props.source.clone(), |source| export::readable(source));
    let perform = use_callback(
        (
            props.source.clone(),
            props.name.clone(),
            busy.clone(),
            status.clone(),
        ),
        |action: Action, (source, name, busy, status)| {
            busy.set(true);
            status.set(
                match action {
                    Action::Copy => "Copying image",
                    Action::Download => "Preparing download",
                }
                .into(),
            );
            // Clipboard.write must start inside the trusted click, before awaiting conversion.
            let operation = match action {
                Action::Copy => export::copy(source.clone()),
                Action::Download => Ok(export::download(source.clone(), name.clone())),
            };
            let busy = busy.clone();
            let status = status.clone();
            spawn_local(async move {
                let result = match operation {
                    Ok(promise) => JsFuture::from(promise).await,
                    Err(error) => Err(error),
                };
                busy.set(false);
                status.set(match result {
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
                });
            });
        },
    );
    let copy = use_callback(perform.clone(), |(), perform| perform.emit(Action::Copy));
    let download = use_callback(perform, |(), perform| perform.emit(Action::Download));
    let copy_button = use_button(copy);
    let download_button = use_button(download);
    let close_button = use_button(props.onclose.clone());
    let message = if !status.is_empty() {
        (*status).clone()
    } else if !*exportable {
        "Copy and download are unavailable for this image's origin.".into()
    } else if !*copy_supported {
        "Image copy is unavailable in this browser; download is still available.".into()
    } else {
        props.image_status.clone()
    };
    html! {
        <>
            <div class="image-lightbox-actions">
                <button ref={copy_button} type="button" disabled={*busy || !*exportable || !*copy_supported} aria-label="Copy image" title="Copy image as PNG">{"Copy"}</button>
                <button ref={download_button} type="button" disabled={*busy || !*exportable} aria-label="Download image" title="Download original image">{"Download"}</button>
                <button ref={close_button} type="button" autofocus=true aria-label="Close full-size image">{"Close"}</button>
            </div>
            <p class="image-lightbox-status" role="status">{message}</p>
        </>
    }
}

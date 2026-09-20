//! One image-only top-layer viewer for Markdown, diagrams, and attachments.

use wasm_bindgen::prelude::*;
use yew::prelude::*;

mod actions;

#[derive(Clone, PartialEq)]
struct Selection {
    source: AttrValue,
    alt: AttrValue,
    name: AttrValue,
    opener: web_sys::Element,
}

#[derive(Properties, PartialEq)]
pub(super) struct ProviderProps {
    pub children: Children,
}

#[function_component(Provider)]
pub(super) fn provider(props: &ProviderProps) -> Html {
    let selected = use_state_eq(|| None::<Selection>);
    use_effect_with((), {
        let selected = selected.clone();
        move |()| {
            let document = web_sys::window()
                .expect("Canvas window")
                .document()
                .expect("Canvas document");
            let opened = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                let Some(opener) = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .and_then(|target| target.closest("button[data-image-preview]").ok().flatten())
                else {
                    return;
                };
                let Some(image) = opener
                    .query_selector("img")
                    .ok()
                    .flatten()
                    .and_then(|image| image.dyn_into::<web_sys::HtmlImageElement>().ok())
                else {
                    return;
                };
                event.prevent_default();
                selected.set(Some(Selection {
                    source: image.src().into(),
                    alt: image.alt().into(),
                    name: opener
                        .get_attribute("data-image-name")
                        .unwrap_or_default()
                        .into(),
                    opener,
                }));
            });
            document
                .add_event_listener_with_callback("click", opened.as_ref().unchecked_ref())
                .expect("the Canvas document accepts image activation");
            move || {
                let _ = document
                    .remove_event_listener_with_callback("click", opened.as_ref().unchecked_ref());
            }
        }
    });
    let close = use_callback(selected.clone(), |(), selected| selected.set(None));
    html! {
        <>
            {props.children.clone()}
            if let Some(selection) = selected.as_ref() {
                <Viewer selection={selection.clone()} onclose={close} />
            }
        </>
    }
}

/// Raw-Markdown portals need listeners on physical nodes rather than delegated Yew events.
#[hook]
pub(super) fn use_button(action: Callback<()>) -> NodeRef {
    let button = use_node_ref();
    use_effect_with((button.clone(), action), move |(button, action)| {
        let registration = button.cast::<web_sys::HtmlElement>().map(|button| {
            let action = action.clone();
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| action.emit(()));
            button
                .add_event_listener_with_callback("click", callback.as_ref().unchecked_ref())
                .expect("the owned button accepts its click listener");
            (button, callback)
        });
        move || {
            if let Some((button, callback)) = registration {
                let _ = button.remove_event_listener_with_callback(
                    "click",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
    button
}

#[derive(Properties, PartialEq)]
struct ViewerProps {
    selection: Selection,
    onclose: Callback<()>,
}

#[function_component(Viewer)]
fn viewer(props: &ViewerProps) -> Html {
    let host = use_node_ref();
    let image_status = use_state_eq(|| AttrValue::from("Loading image"));
    let loaded = use_callback(image_status.clone(), |_: Event, status| {
        status.set(AttrValue::default())
    });
    let failed = use_callback(image_status.clone(), |_: Event, status| {
        status.set("Image unavailable".into())
    });
    let close = use_callback(host.clone(), |(), host| {
        if let Some(dialog) = host.cast::<web_sys::HtmlDialogElement>() {
            dialog.close();
        }
    });
    use_effect_with(
        (
            host.clone(),
            props.onclose.clone(),
            props.selection.opener.clone(),
        ),
        |(host, onclose, opener)| {
            let dialog = host
                .cast::<web_sys::HtmlDialogElement>()
                .expect("the mounted viewer owns its dialog");
            let onclose = onclose.clone();
            let closed = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| onclose.emit(()));
            let backdrop =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                    if let Some(dialog) = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlDialogElement>().ok())
                    {
                        dialog.close();
                    }
                });
            dialog
                .add_event_listener_with_callback("close", closed.as_ref().unchecked_ref())
                .expect("dialog close listener");
            dialog
                .add_event_listener_with_callback("click", backdrop.as_ref().unchecked_ref())
                .expect("dialog backdrop listener");
            let owner = opener.clone();
            let owned_dialog = dialog.clone();
            let detached =
                Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |_, _| {
                    if !owner.is_connected() {
                        owned_dialog.close();
                    }
                });
            let observer = web_sys::MutationObserver::new(detached.as_ref().unchecked_ref())
                .expect("viewer ownership observer");
            let options = web_sys::MutationObserverInit::new();
            options.set_child_list(true);
            options.set_subtree(true);
            observer
                .observe_with_options(
                    &opener.owner_document().expect("preview document"),
                    &options,
                )
                .expect("observe preview lifetime");
            dialog
                .show_modal()
                .expect("the connected viewer opens in the top layer");
            move || {
                observer.disconnect();
                drop(detached);
                let _ = dialog
                    .remove_event_listener_with_callback("close", closed.as_ref().unchecked_ref());
                let _ = dialog.remove_event_listener_with_callback(
                    "click",
                    backdrop.as_ref().unchecked_ref(),
                );
                dialog.close();
            }
        },
    );
    html! {
        <dialog ref={host} class="image-lightbox" aria-label="Full-size image">
            <actions::Actions source={props.selection.source.clone()} name={props.selection.name.clone()} onclose={close} image_status={(*image_status).clone()} />
            <img class="image-lightbox-image" src={props.selection.source.clone()} alt={props.selection.alt.clone()} onload={loaded} onerror={failed} />
        </dialog>
    }
}

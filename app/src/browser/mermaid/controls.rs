//! Controls in raw-Markdown portals bind to their physical DOM nodes.

use super::*;

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

#[hook]
pub(super) fn use_viewer(image: Option<AttrValue>) -> (NodeRef, Html) {
    let inspected = use_state_eq(|| None::<AttrValue>);
    let inspect = use_callback((inspected.clone(), image), |(), (inspected, image)| {
        inspected.set(image.clone())
    });
    let button = use_button(inspect);
    let close = use_callback(inspected.clone(), |(), inspected| inspected.set(None));
    let viewer = html! {
        if let Some(image) = inspected.as_ref() {
            <Viewer image={image.clone()} onclose={close} />
        }
    };
    (button, viewer)
}

#[derive(Properties, PartialEq)]
struct ViewerProps {
    image: AttrValue,
    onclose: Callback<()>,
}

/// The top layer escapes card clipping without changing the floor's layout.
/// SVG stays an inert image; never navigate to it as an active SVG document.
#[function_component(Viewer)]
fn viewer(props: &ViewerProps) -> Html {
    let host = use_node_ref();
    let close = use_callback(host.clone(), |(), host| {
        if let Some(dialog) = host.cast::<web_sys::HtmlDialogElement>() {
            dialog.close();
        }
    });
    let close_button = use_button(close);
    use_effect_with((host.clone(), props.onclose.clone()), |(host, onclose)| {
        let dialog = host
            .cast::<web_sys::HtmlDialogElement>()
            .expect("the mounted viewer owns its dialog");
        let onclose = onclose.clone();
        let closed = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| onclose.emit(()));
        let backdrop = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            if let Some(dialog) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlDialogElement>().ok())
            {
                dialog.close();
            }
        });
        dialog
            .add_event_listener_with_callback("close", closed.as_ref().unchecked_ref())
            .expect("the owned dialog accepts its close listener");
        dialog
            .add_event_listener_with_callback("click", backdrop.as_ref().unchecked_ref())
            .expect("the owned dialog accepts its backdrop listener");
        dialog
            .show_modal()
            .expect("the connected viewer opens in the top layer");
        move || {
            let _ = dialog
                .remove_event_listener_with_callback("close", closed.as_ref().unchecked_ref());
            let _ = dialog
                .remove_event_listener_with_callback("click", backdrop.as_ref().unchecked_ref());
            dialog.close();
        }
    });
    html! {
        <dialog ref={host} class="mermaid-lightbox" aria-label="Full-size Mermaid diagram">
            <button ref={close_button} type="button" aria-label="Close full-size diagram">{"Close"}</button>
            <img src={props.image.clone()} alt="Full-size Mermaid diagram" data-mosaic-image-state="loaded" />
        </dialog>
    }
}

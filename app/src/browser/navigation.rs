//! Browser-history ownership for the three global Canvas destinations.
//!
//! Only the global `axis`/`mosaic` projection reaches this module. Team tabs,
//! work scopes, detail drawers, and other transient state therefore remain in
//! the current history entry by construction.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;
use web_sys::{Url, Window};
use yew::prelude::*;

use crate::{Axis, CanvasDestination};

const PAGE_PARAMETER: &str = "page";

#[hook]
pub(super) fn use_global_destination(
    canvas_menu_open: UseStateHandle<bool>,
) -> (UseStateHandle<Axis>, UseStateHandle<bool>) {
    let initial_destination = initial_destination();
    let axis = use_state(move || initial_destination.axis().unwrap_or(Axis::Topology));
    let mosaic = use_state(move || initial_destination == CanvasDestination::Chat);
    use_browser_history(axis.clone(), mosaic.clone(), canvas_menu_open);
    (axis, mosaic)
}

fn initial_destination() -> CanvasDestination {
    web_sys::window()
        .as_ref()
        .map_or(CanvasDestination::DEFAULT, location_destination)
}

#[hook]
fn use_browser_history(
    axis: UseStateHandle<Axis>,
    mosaic: UseStateHandle<bool>,
    canvas_menu_open: UseStateHandle<bool>,
) {
    let surface = (*axis, *mosaic);
    use_effect_with(surface, move |&(axis, mosaic)| {
        let destination = CanvasDestination::from_surface(axis, mosaic);
        if let Some(window) = web_sys::window()
            && location_destination(&window) != destination
        {
            let _ = push_destination(&window, destination);
        }
    });

    use_effect_with((), move |()| {
        let registration = web_sys::window().and_then(|window| {
            let listening_window = window.clone();
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event| {
                let destination = location_destination(&listening_window);
                if let Some(axis_value) = destination.axis() {
                    axis.set(axis_value);
                }
                mosaic.set(destination == CanvasDestination::Chat);
                canvas_menu_open.set(false);
            });
            window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref())
                .ok()
                .map(|()| (window, callback))
        });
        move || {
            if let Some((window, callback)) = registration {
                let _ = window.remove_event_listener_with_callback(
                    "popstate",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

fn location_destination(window: &Window) -> CanvasDestination {
    let route_value = window
        .location()
        .href()
        .ok()
        .and_then(|href| Url::new(&href).ok())
        .and_then(|url| url.search_params().get(PAGE_PARAMETER));
    CanvasDestination::from_route_value(route_value.as_deref())
}

fn push_destination(window: &Window, destination: CanvasDestination) -> Result<(), JsValue> {
    let url = Url::new(&window.location().href()?)?;
    url.search_params()
        .set(PAGE_PARAMETER, destination.route_value());
    window
        .history()?
        .push_state_with_url(&JsValue::NULL, "", Some(&url.href()))
}

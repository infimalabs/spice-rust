//! Hash-history ownership for the three global Canvas destinations.
//!
//! Only the global destination reaches browser history. Team tabs, work
//! scopes, detail drawers, and other transient state therefore remain in the
//! current history entry by construction.

use gloo_history::{HashHistory, History};
use yew::prelude::*;

use crate::{Axis, CanvasDestination};

#[hook]
pub(super) fn use_destination() -> (Option<CanvasDestination>, Callback<CanvasDestination>) {
    let history = use_state(HashHistory::new);
    let destination = {
        let history = history.clone();
        use_state(move || location_destination(&history))
    };

    {
        let history = (*history).clone();
        let destination = destination.clone();
        use_effect_with((), move |()| {
            let observed_history = history.clone();
            let listener = history.listen(move || {
                destination.set(location_destination(&observed_history));
            });
            move || drop(listener)
        });
    }

    let history = (*history).clone();
    let navigate = Callback::from(move |destination: CanvasDestination| {
        history.push(destination.path());
    });
    (*destination, navigate)
}

fn location_destination(history: &HashHistory) -> Option<CanvasDestination> {
    let location = history.location();
    let address_query_free = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .is_some_and(|query| query.is_empty());
    if !address_query_free || !location.query_str().is_empty() {
        return None;
    }
    CanvasDestination::from_path(location.path())
}

#[hook]
pub(super) fn use_global_destination(
    destination: CanvasDestination,
) -> (UseStateHandle<Axis>, UseStateHandle<bool>) {
    let axis = use_state(move || destination.axis().unwrap_or(Axis::Topology));
    let mosaic = use_state(move || destination == CanvasDestination::Chat);

    {
        let axis = axis.clone();
        let mosaic = mosaic.clone();
        use_effect_with(destination, move |destination| {
            if let Some(destination_axis) = destination.axis()
                && *axis != destination_axis
            {
                axis.set(destination_axis);
            }
            let destination_mosaic = *destination == CanvasDestination::Chat;
            if *mosaic != destination_mosaic {
                mosaic.set(destination_mosaic);
            }
        });
    }

    (axis, mosaic)
}

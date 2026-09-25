//! Hash-history ownership for the three global Canvas destinations.
//!
//! Only the global destination reaches browser history. Team tabs, work
//! scopes, detail drawers, and other transient state therefore remain in the
//! current history entry by construction.

use gloo_history::{HashHistory, History};
use yew::prelude::*;

use crate::CanvasDestination;
use crate::work::WorkScope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    Info,
    Work(WorkScope),
    SelectedWork,
    Chat,
}

pub(super) fn intent_callback(
    push: Callback<CanvasDestination>,
    work_scope: UseStateHandle<WorkScope>,
    work_scope_notice: UseStateHandle<Option<String>>,
) -> Callback<Intent> {
    Callback::from(move |intent| {
        let destination = match intent {
            Intent::Info => CanvasDestination::Info,
            Intent::Work(scope) => {
                work_scope.set(scope);
                work_scope_notice.set(None);
                CanvasDestination::Work
            }
            Intent::SelectedWork => CanvasDestination::Work,
            Intent::Chat => CanvasDestination::Chat,
        };
        push.emit(destination);
    })
}

#[hook]
pub(super) fn use_destination() -> (Option<CanvasDestination>, Callback<CanvasDestination>) {
    let history = use_state(HashHistory::new);
    let destination = use_state(location_destination);

    {
        let history = (*history).clone();
        let destination = destination.clone();
        use_effect_with((), move |()| {
            let listener = history.listen(move || {
                destination.set(location_destination());
            });
            move || drop(listener)
        });
    }

    let history = (*history).clone();
    let navigate = Callback::from(move |destination: CanvasDestination| {
        if location_destination() != Some(destination) {
            history.push(destination.path());
        }
    });
    (*destination, navigate)
}

fn location_destination() -> Option<CanvasDestination> {
    let location = web_sys::window()?.location();
    if !location.search().ok()?.is_empty() {
        return None;
    }
    let hash = location.hash().ok()?;
    CanvasDestination::from_path(hash.strip_prefix('#')?)
}

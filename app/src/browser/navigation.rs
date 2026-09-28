//! Hash-history ownership for the three global Canvas destinations.
//!
//! Only the global destination reaches browser history. Team tabs, work
//! scopes, detail drawers, and other transient state therefore remain in the
//! current history entry by construction.

use gloo_history::{HashHistory, History};
use wire::TaskStamp;
use yew::prelude::*;

use super::SurfaceData;
use crate::view::CanvasRoute;
use crate::work::{WorkProjection, WorkScope};
use crate::{CanvasDestination, CanvasRuntime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    Dash,
    Work(WorkScope),
    /// Open this exact durable task over the current global destination.
    Task(TaskStamp),
    Chat,
}

/// Every state a destination names changes in the one callback that pushes
/// it, so the first destination render already agrees with the history entry.
pub(super) fn intent_callback(
    destination: CanvasDestination,
    push: Callback<CanvasDestination>,
    work_scope: UseStateHandle<WorkScope>,
    work_scope_notice: UseStateHandle<Option<String>>,
    select_task: Callback<TaskStamp>,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<Intent> {
    Callback::from(move |intent| match intent {
        Intent::Dash => push.emit(CanvasDestination::Dash),
        Intent::Work(scope) => {
            work_scope.set(scope);
            work_scope_notice.set(None);
            push.emit(CanvasDestination::Work);
        }
        Intent::Task(task) => {
            if destination == CanvasDestination::Work {
                let scope = surface
                    .read()
                    .as_ref()
                    .and_then(CanvasRuntime::board)
                    .map_or(WorkScope::AllOpen, |board| {
                        WorkProjection::from_board(board).scope_showing(&task, &work_scope)
                    });
                work_scope.set(scope);
                work_scope_notice.set(None);
            }
            select_task.emit(task);
        }
        Intent::Chat => push.emit(CanvasDestination::Chat),
    })
}

#[hook]
pub(super) fn use_destination() -> (Option<CanvasRoute>, Callback<CanvasDestination>) {
    let history = use_state(HashHistory::new);
    let route = use_state(location_route);

    {
        let history = (*history).clone();
        let route = route.clone();
        use_effect_with((), move |()| {
            let listener = history.listen(move || {
                route.set(location_route());
            });
            move || drop(listener)
        });
    }

    let history = (*history).clone();
    let navigate = Callback::from(move |destination: CanvasDestination| {
        if location_route() != Some(CanvasRoute::destination(destination)) {
            history.push(destination.path());
        }
    });
    ((*route).clone(), navigate)
}

fn location_route() -> Option<CanvasRoute> {
    let location = web_sys::window()?.location();
    if !location.search().ok()?.is_empty() {
        return None;
    }
    let hash = location.hash().ok()?;
    CanvasRoute::from_path(hash.strip_prefix('#')?)
}

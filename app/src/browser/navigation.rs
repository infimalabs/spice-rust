//! Hash-history ownership for the three global Canvas destinations.
//!
//! Only the global destination reaches browser history. Team tabs, work
//! scopes, detail drawers, and other transient state therefore remain in the
//! current history entry by construction.

use gloo_history::{HashHistory, History};
use wire::TaskStamp;
use yew::prelude::*;

use super::SurfaceData;
use crate::work::{WorkProjection, WorkScope};
use crate::{CanvasDestination, CanvasRuntime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    Info,
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
        Intent::Info => push.emit(CanvasDestination::Info),
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

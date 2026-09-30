//! Hash-history ownership for the three global Canvas destinations.
//!
//! Only the global destination reaches browser history. Team tabs, work
//! scopes, detail drawers, and other transient state therefore remain in the
//! current history entry by construction.

use gloo_history::{HashHistory, History};
use wire::TaskStamp;
use yew::prelude::*;

use super::SurfaceData;
use crate::view::{CanvasRoute, NormalizedCanvasLocation, normalize_canvas_location};
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
pub(super) fn use_destination() -> (CanvasRoute, Option<String>, Callback<CanvasDestination>) {
    let history = use_state(HashHistory::new);
    let location = use_state(location_route);

    {
        let history = (*history).clone();
        let location = location.clone();
        use_effect_with((), move |()| {
            let listener = history.listen(move || {
                location.set(location_route());
            });
            move || drop(listener)
        });
    }
    {
        use_effect_with((*location).clone(), move |location| {
            if location.needs_replace() {
                replace_location(location.route());
            }
        });
    }

    let history = (*history).clone();
    let navigate = Callback::from(move |destination: CanvasDestination| {
        if location_route().route() != &CanvasRoute::destination(destination) {
            history.push(destination.path());
        }
    });
    (
        location.route().clone(),
        location.notice().map(str::to_owned),
        navigate,
    )
}

fn location_route() -> NormalizedCanvasLocation {
    let window = web_sys::window().expect("Canvas navigation requires a browser window");
    let location = window.location();
    let search = location
        .search()
        .expect("Canvas navigation requires the current query");
    let hash = location
        .hash()
        .expect("Canvas navigation requires the current fragment");
    normalize_canvas_location(&search, &hash)
}

fn replace_location(route: &CanvasRoute) {
    let window = web_sys::window().expect("Canvas navigation requires a browser window");
    let pathname = window
        .location()
        .pathname()
        .expect("Canvas navigation requires the current path");
    let history = window
        .history()
        .expect("Canvas navigation requires browser history");
    let state = history
        .state()
        .expect("Canvas navigation requires the current history state");
    let url = format!("{pathname}#{}", route.path());
    history
        .replace_state_with_url(&state, "", Some(&url))
        .expect("Canvas navigation requires replaceState");
}

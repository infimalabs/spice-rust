//! Hash-history ownership for the addressable Canvas view.
//!
//! The URL is the sole durable selection: a global destination, a Work scope
//! and its task detail, or a Chat card. Per-team modes, menus, arrangement,
//! and a task detail opened over Dash or Chat stay browser-local, so they
//! never reach history. Browser Back, Forward, and fragment links only
//! rehydrate the route; every history entry Canvas creates is one push.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::Closure;
use wire::TaskStamp;
use yew::prelude::*;

use super::SurfaceData;
use crate::view::{AppRoute, NormalizedAppLocation, RouteStep, normalize_app_location};
use crate::work::{WorkProjection, WorkScope};
use crate::{AppDestination, AppRuntime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    Dash,
    Work(WorkScope),
    /// Open this exact durable task: through its route on Work, or as a
    /// browser-local detail over Dash or Chat.
    Task(TaskStamp),
    /// Close the open task detail.
    CloseTask,
    Chat,
}

/// One route change the router applies to browser history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Navigation {
    Step(RouteStep),
    /// Replace a route whose settled data refused a segment, naming it.
    Refuse(AppRoute, String),
    /// Close the routed task detail. Back consumes the entry this page pushed
    /// to open it; any other task entry is replaced by its containing scope.
    CloseTask,
}

/// A notice naming one refused route. The identity distinguishes repeated
/// refusals of the same text so each is announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RouteNotice {
    id: u64,
    pub(super) text: String,
}

/// Translate one reader intent against the route it was made on.
pub(super) fn intent_callback(
    route: AppRoute,
    navigate: Callback<Navigation>,
    local_task: UseStateHandle<Option<TaskStamp>>,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<Intent> {
    Callback::from(move |intent| {
        let step = match intent {
            Intent::Dash => route.show(AppDestination::Dash),
            Intent::Chat => route.show(AppDestination::Chat),
            Intent::Work(scope) => route.show_work(scope),
            Intent::Task(task) => {
                let Some(scope) = route.work_scope() else {
                    local_task.set(Some(task));
                    return;
                };
                let scope = surface
                    .read()
                    .as_ref()
                    .and_then(AppRuntime::board)
                    .map_or(WorkScope::AllOpen, |board| {
                        WorkProjection::from_board(board).scope_showing(&task, scope)
                    });
                route.open_task(task, scope)
            }
            Intent::CloseTask => {
                if route.task().is_some() {
                    navigate.emit(Navigation::CloseTask);
                } else {
                    local_task.set(None);
                }
                return;
            }
        };
        if let Some(step) = step {
            navigate.emit(Navigation::Step(step));
        }
    })
}

#[hook]
pub(super) fn use_route() -> (AppRoute, Option<RouteNotice>, Callback<Navigation>) {
    let location = use_state(location_route);
    let notice = use_state(|| None::<RouteNotice>);
    let announced = use_mut_ref(|| 0_u64);
    // The task entry this page pushed, so closing it can consume it.
    let owned = use_mut_ref(|| None::<String>);
    let announce = {
        let notice = notice.clone();
        move |text: String| {
            let id = {
                let mut announced = announced.borrow_mut();
                *announced += 1;
                *announced
            };
            notice.set(Some(RouteNotice { id, text }));
        }
    };

    {
        let location = location.clone();
        use_effect_with((), move |()| {
            let rehydrate = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                location.set(location_route());
            });
            let window = web_sys::window().expect("Canvas navigation requires a browser window");
            window
                .add_event_listener_with_callback("popstate", rehydrate.as_ref().unchecked_ref())
                .expect("Canvas navigation requires popstate");
            move || {
                let _ = window.remove_event_listener_with_callback(
                    "popstate",
                    rehydrate.as_ref().unchecked_ref(),
                );
            }
        });
    }
    {
        let announce = announce.clone();
        use_effect_with((*location).clone(), move |location| {
            if location.needs_replace() {
                write_location(&location.route().path(), Entry::Replace);
            }
            if let Some(text) = location.notice() {
                announce(text.to_owned());
            }
        });
    }

    let navigate = {
        let location = location.clone();
        Callback::from(move |navigation: Navigation| {
            let current = location_route().route().path();
            match navigation {
                Navigation::Step(RouteStep::Push(route)) => {
                    let path = route.path();
                    if route.task().is_some() {
                        *owned.borrow_mut() = Some(path.clone());
                    }
                    write_location(&path, Entry::Push);
                }
                Navigation::Step(RouteStep::Replace(route)) => {
                    replace_owned(&owned, &current, &route);
                }
                Navigation::Refuse(route, text) => {
                    replace_owned(&owned, &current, &route);
                    announce(text);
                }
                Navigation::CloseTask => {
                    let Some(open) = location_route().route().without_task() else {
                        return;
                    };
                    if owned.borrow().as_deref() == Some(current.as_str()) {
                        let history = web_sys::window()
                            .and_then(|window| window.history().ok())
                            .expect("Canvas navigation requires browser history");
                        // The popstate this raises rehydrates the route.
                        history
                            .back()
                            .expect("Canvas navigation requires history.back");
                        return;
                    }
                    write_location(&open.path(), Entry::Replace);
                }
            }
            location.set(location_route());
        })
    };
    (location.route().clone(), (*notice).clone(), navigate)
}

/// Replace the current entry, carrying this page's ownership of a task entry
/// to the task that replaces it.
fn replace_owned(owned: &Rc<RefCell<Option<String>>>, current: &str, route: &AppRoute) {
    let path = route.path();
    let mut owned = owned.borrow_mut();
    if owned.as_deref() == Some(current) {
        *owned = route.task().is_some().then(|| path.clone());
    }
    write_location(&path, Entry::Replace);
}

fn location_route() -> NormalizedAppLocation {
    let window = web_sys::window().expect("Canvas navigation requires a browser window");
    let location = window.location();
    let search = location
        .search()
        .expect("Canvas navigation requires the current query");
    let hash = location
        .hash()
        .expect("Canvas navigation requires the current fragment");
    normalize_app_location(&search, &hash)
}

/// Replace the current entry with one route before routing starts.
pub(super) fn replace_location(route: &AppRoute) {
    write_location(&route.path(), Entry::Replace);
}

#[derive(Clone, Copy)]
enum Entry {
    Push,
    Replace,
}

fn write_location(path: &str, entry: Entry) {
    let window = web_sys::window().expect("Canvas navigation requires a browser window");
    let pathname = window
        .location()
        .pathname()
        .expect("Canvas navigation requires the current path");
    let history = window
        .history()
        .expect("Canvas navigation requires browser history");
    let url = format!("{pathname}#{path}");
    match entry {
        Entry::Push => history
            .push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
            .expect("Canvas navigation requires pushState"),
        Entry::Replace => {
            let state = history
                .state()
                .expect("Canvas navigation requires the current history state");
            history
                .replace_state_with_url(&state, "", Some(&url))
                .expect("Canvas navigation requires replaceState");
        }
    }
}

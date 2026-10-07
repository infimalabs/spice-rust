//! Hash-history ownership for durable Dash and Chat locations.
//!
//! Task overviews are browser-local overlays. Their complete selection never
//! enters the URL or creates a history entry; browser Back and Forward own only
//! durable destinations and Chat card anchors.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::Closure;
use yew::prelude::*;

use super::SurfaceData;
use crate::AppDestination;
use crate::view::{AppRoute, NormalizedAppLocation, RouteStep, normalize_app_location};
use crate::work::{TaskOverviewSelection, WorkProjection, WorkScope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
    Dash,
    /// Open every task in one current board scope without changing location.
    Work(WorkScope),
    /// Open every task in one exact project without changing location.
    Project(String),
    /// Open this exact durable task without changing location.
    Task(wire::TaskStamp),
    /// Open a caller-derived complete selection, such as a relationship lead.
    Overview(TaskOverviewSelection),
    /// Close the browser-local task overview.
    CloseTask,
}

/// One route change the router applies to browser history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Navigation {
    Step(RouteStep),
    /// Replace a route whose settled data refused a segment, naming it.
    Refuse(AppRoute, String),
}

/// A notice naming one refused route. The identity distinguishes repeated
/// refusals of the same text so each is announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RouteNotice {
    id: u64,
    pub(super) text: String,
}

/// Translate one reader intent against the durable route and current board.
pub(super) fn intent_callback(
    route: AppRoute,
    navigate: Callback<Navigation>,
    local_selection: UseStateHandle<Option<TaskOverviewSelection>>,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<Intent> {
    Callback::from(move |intent| {
        let step = match intent {
            Intent::Dash => route.show(AppDestination::Dash),
            Intent::Work(scope) => {
                local_selection.set(Some(
                    surface
                        .read()
                        .as_ref()
                        .and_then(|runtime| {
                            runtime.board().map(|board| {
                                WorkProjection::from_board_and_lanes(board, runtime.lanes())
                                    .overview_for_scope(scope.clone())
                            })
                        })
                        .unwrap_or_else(|| {
                            TaskOverviewSelection::new(
                                [],
                                crate::work::TaskOverviewContext::Scope(scope),
                            )
                        }),
                ));
                return;
            }
            Intent::Project(project) => {
                local_selection.set(Some(
                    surface
                        .read()
                        .as_ref()
                        .and_then(|runtime| {
                            runtime.board().map(|board| {
                                WorkProjection::from_board_and_lanes(board, runtime.lanes())
                                    .overview_for_project(project.clone())
                            })
                        })
                        .unwrap_or_else(|| {
                            TaskOverviewSelection::new(
                                [],
                                crate::work::TaskOverviewContext::Project(project),
                            )
                        }),
                ));
                return;
            }
            Intent::Task(task) => {
                local_selection.set(Some(TaskOverviewSelection::task(task)));
                return;
            }
            Intent::Overview(selection) => {
                local_selection.set(Some(selection));
                return;
            }
            Intent::CloseTask => {
                local_selection.set(None);
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
            let window = web_sys::window().expect("app navigation requires a browser window");
            window
                .add_event_listener_with_callback("popstate", rehydrate.as_ref().unchecked_ref())
                .expect("app navigation requires popstate");
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
            match navigation {
                Navigation::Step(RouteStep::Push(route)) => {
                    write_location(&route.path(), Entry::Push);
                }
                Navigation::Refuse(route, text) => {
                    write_location(&route.path(), Entry::Replace);
                    announce(text);
                }
            }
            location.set(location_route());
        })
    };
    (location.route().clone(), (*notice).clone(), navigate)
}

fn location_route() -> NormalizedAppLocation {
    let window = web_sys::window().expect("app navigation requires a browser window");
    let location = window.location();
    let search = location
        .search()
        .expect("app navigation requires the current query");
    let hash = location
        .hash()
        .expect("app navigation requires the current fragment");
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
    let window = web_sys::window().expect("app navigation requires a browser window");
    let pathname = window
        .location()
        .pathname()
        .expect("app navigation requires the current path");
    let history = window
        .history()
        .expect("app navigation requires browser history");
    let url = format!("{pathname}#{path}");
    match entry {
        Entry::Push => history
            .push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url))
            .expect("app navigation requires pushState"),
        Entry::Replace => {
            let state = history
                .state()
                .expect("app navigation requires the current history state");
            history
                .replace_state_with_url(&state, "", Some(&url))
                .expect("app navigation requires replaceState");
        }
    }
}

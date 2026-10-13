//! Hash-history ownership for durable Chat locations.
//!
//! Task overviews are browser-local overlays. Their complete selection never
//! enters the URL or creates a history entry; browser Back and Forward own
//! plain Chat and retained-card anchors. A Design permalink is an arrival, not
//! a location: it opens that Design's read view and leaves the location Chat.

use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::Closure;
use yew::prelude::*;

use super::SurfaceData;
use crate::view::{AppRoute, NormalizedAppLocation, normalize_app_location};
use crate::work::{TaskOverviewSelection, WorkProjection, WorkScope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Intent {
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

/// One settled route refusal that replaces the unusable card location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RouteRefusal {
    pub(super) route: AppRoute,
    pub(super) text: String,
}

/// A notice naming one refused route. The identity distinguishes repeated
/// refusals of the same text so each is announced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RouteNotice {
    id: u64,
    pub(super) text: String,
}

/// One arrival at a Design permalink. The identity distinguishes repeated
/// arrivals at the same Design so each opens its read view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DesignLink {
    id: u64,
    pub(super) design: wire::TaskStamp,
}

/// Translate one reader intent against the current board.
pub(super) fn intent_callback(
    local_selection: UseStateHandle<Option<TaskOverviewSelection>>,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<Intent> {
    Callback::from(move |intent| match intent {
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
        }
        Intent::Task(task) => {
            local_selection.set(Some(TaskOverviewSelection::task(task)));
        }
        Intent::Overview(selection) => {
            local_selection.set(Some(selection));
        }
        Intent::CloseTask => {
            local_selection.set(None);
        }
    })
}

#[hook]
pub(super) fn use_route() -> (
    AppRoute,
    Option<RouteNotice>,
    Callback<RouteRefusal>,
    Option<DesignLink>,
) {
    let location = use_state(location_route);
    let notice = use_state(|| None::<RouteNotice>);
    let link = use_state(|| None::<DesignLink>);
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
    let arrivals = use_mut_ref(|| 0_u64);
    let arrive = {
        let link = link.clone();
        move |location: &NormalizedAppLocation| {
            if let Some(design) = location.design() {
                let id = {
                    let mut arrivals = arrivals.borrow_mut();
                    *arrivals += 1;
                    *arrivals
                };
                link.set(Some(DesignLink {
                    id,
                    design: design.clone(),
                }));
            }
        }
    };

    {
        let location = location.clone();
        use_effect_with((), move |()| {
            arrive(&location);
            let rehydrate = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                let next = location_route();
                arrive(&next);
                location.set(next);
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
                write_location(&location.route().path());
            }
            if let Some(text) = location.notice() {
                announce(text.to_owned());
            }
        });
    }

    let navigate = {
        let location = location.clone();
        Callback::from(move |refusal: RouteRefusal| {
            write_location(&refusal.route.path());
            announce(refusal.text);
            location.set(location_route());
        })
    };
    (
        location.route().clone(),
        (*notice).clone(),
        navigate,
        (*link).clone(),
    )
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
    write_location(&route.path());
}

fn write_location(path: &str) {
    let window = web_sys::window().expect("app navigation requires a browser window");
    let pathname = window
        .location()
        .pathname()
        .expect("app navigation requires the current path");
    let history = window
        .history()
        .expect("app navigation requires browser history");
    let state = history
        .state()
        .expect("app navigation requires the current history state");
    let url = format!("{pathname}#{path}");
    history
        .replace_state_with_url(&state, "", Some(&url))
        .expect("app navigation requires replaceState");
}

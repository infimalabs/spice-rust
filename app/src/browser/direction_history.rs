//! Demand-driven global Work Directions controls.

use std::collections::BTreeSet;

use super::*;

#[derive(Clone)]
pub(super) struct Controls {
    pub validation: UseStateHandle<Option<String>>,
    pub selected_agents: UseStateHandle<BTreeSet<CanonicalUuid>>,
    pub selected_teams: UseStateHandle<BTreeSet<CanonicalUuid>>,
    pub selected_states: UseStateHandle<BTreeSet<wire::Disposition>>,
    pub apply: Callback<MouseEvent>,
    pub clear: Callback<MouseEvent>,
    pub toggle_agent: Callback<CanonicalUuid>,
    pub toggle_team: Callback<CanonicalUuid>,
    pub toggle_state: Callback<wire::Disposition>,
    pub newer: Callback<MouseEvent>,
    pub older: Callback<MouseEvent>,
    pub retry: Callback<MouseEvent>,
}

fn input_value(id: &str) -> String {
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default()
}

fn clear_input(id: &str) {
    if let Some(input) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.set_value("");
    }
}

fn queue(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    reserve: impl FnOnce(&mut AppRuntime) -> Option<crate::runtime::DirectionHistoryRequest>,
) {
    let request = {
        let mut data = surface.write();
        let Some(runtime) = data.as_mut() else {
            return;
        };
        if !runtime.offers(CommandKind::SteerHistory) {
            return;
        }
        reserve(runtime)
    };
    let Some(request) = request else {
        return;
    };
    surface.set(surface.publication());
    if !admit_action(
        sender,
        ClientAction::DirectionHistory {
            request: request.clone(),
        },
    ) {
        let mut data = surface.write();
        if let Some(runtime) = data.as_mut() {
            runtime.refuse_direction_history_request(
                &request,
                "Direction history request could not be queued.".to_owned(),
            );
        }
        drop(data);
        surface.set(surface.publication());
    }
}

/// Replace a focused Directions route once the reader's own filters take
/// over, so the URL never names a narrowing the page no longer shows.
fn release_route(navigate: &Callback<navigation::Navigation>, focused: bool) {
    if focused {
        navigate.emit(navigation::Navigation::Step(
            crate::view::RouteStep::Replace(AppRoute::Directions { focus: None }),
        ));
    }
}

fn team_lanes(
    surface: &UseStateHandle<SurfaceData>,
    teams: &BTreeSet<CanonicalUuid>,
) -> Vec<CanonicalUuid> {
    surface
        .read()
        .as_ref()
        .and_then(AppRuntime::lanes)
        .map(|topology| {
            topology
                .lanes
                .iter()
                .filter(|team| teams.contains(&team.lane))
                .flat_map(|team| team.members.iter().map(|member| member.lane))
                .collect()
        })
        .unwrap_or_default()
}

fn apply_filters(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    selections: &Selections,
) -> bool {
    let query = input_value("direction-history-query");
    let cutoff = input_value("direction-history-cutoff");
    let authored_before = match crate::runtime::direction_history_authored_cutoff(&cutoff) {
        Ok(cutoff) => cutoff,
        Err(issue) => {
            selections.validation.set(Some(issue));
            return false;
        }
    };
    selections.validation.set(None);
    let mut lanes = selections.agents.iter().copied().collect::<Vec<_>>();
    lanes.extend(team_lanes(surface, &selections.teams));
    let dispositions = selections.states.iter().copied().collect();
    queue(sender, surface, move |runtime| {
        runtime.apply_direction_history_filters(query, lanes, dispositions, authored_before)
    });
    true
}

/// Local filter selections a routed read replaces.
#[derive(Clone)]
struct Selections {
    teams: UseStateHandle<BTreeSet<CanonicalUuid>>,
    agents: UseStateHandle<BTreeSet<CanonicalUuid>>,
    states: UseStateHandle<BTreeSet<wire::Disposition>>,
    validation: UseStateHandle<Option<String>>,
}

impl Selections {
    fn reset(&self, teams: BTreeSet<CanonicalUuid>) {
        clear_input("direction-history-query");
        clear_input("direction-history-cutoff");
        self.teams.set(teams);
        self.agents.set(BTreeSet::new());
        self.states.set(BTreeSet::new());
        self.validation.set(None);
    }
}

/// A click that queues one direction-history read.
fn read_on_click(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    reserve: fn(&mut AppRuntime) -> Option<crate::runtime::DirectionHistoryRequest>,
) -> Callback<MouseEvent> {
    let sender = sender.clone();
    let surface = surface.clone();
    Callback::from(move |_: MouseEvent| queue(&sender, &surface, reserve))
}

/// Start the read a Directions route names, once per arrival at it: the
/// page anchored on its direction, or the history of its team's members.
/// Returns whether the route is now served.
fn serve_route(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    focus: &crate::view::DirectionsFocus,
    selections: &Selections,
) -> bool {
    match focus {
        crate::view::DirectionsFocus::Direction(target) => {
            let served = |surface: &UseStateHandle<SurfaceData>| {
                surface.read().as_ref().is_some_and(|runtime| {
                    let history = runtime.direction_history();
                    history.locate(target) == Some(true) || history.anchoring(target)
                })
            };
            if served(surface) {
                return true;
            }
            selections.reset(BTreeSet::new());
            queue(sender, surface, |runtime| {
                runtime.anchor_direction_history(target)
            });
            served(surface)
        }
        crate::view::DirectionsFocus::Team(team) => {
            if surface
                .read()
                .as_ref()
                .and_then(AppRuntime::lanes)
                .is_none()
            {
                return false;
            }
            let teams = BTreeSet::from([*team]);
            let lanes = team_lanes(surface, &teams);
            selections.reset(teams);
            queue(sender, surface, move |runtime| {
                runtime.apply_direction_history_filters(String::new(), lanes, Vec::new(), None)
            });
            true
        }
    }
}

fn toggle_set<T>(selected: &UseStateHandle<BTreeSet<T>>) -> Callback<T>
where
    T: Copy + Ord + 'static,
{
    let selected = selected.clone();
    Callback::from(move |value: T| {
        let mut next = (*selected).clone();
        if !next.insert(value) {
            next.remove(&value);
        }
        selected.set(next);
    })
}

#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    active: bool,
    focus: Option<crate::view::DirectionsFocus>,
    navigate: Callback<navigation::Navigation>,
) -> Controls {
    let selections = Selections {
        teams: use_state(BTreeSet::new),
        agents: use_state(BTreeSet::new),
        states: use_state(BTreeSet::new),
        validation: use_state(|| None),
    };
    let served = use_mut_ref(|| None::<crate::view::DirectionsFocus>);
    let (revision, surface_ready, roster_ready) =
        surface
            .read()
            .as_ref()
            .map_or((0, false, false), |runtime| {
                (
                    runtime.direction_history().revision(),
                    true,
                    runtime.lanes().is_some(),
                )
            });
    {
        let sender = sender.clone();
        let surface = surface.clone();
        let selections = selections.clone();
        use_effect_with(
            (active, focus.clone(), revision, surface_ready, roster_ready),
            move |(active, focus, _, surface_ready, _)| {
                let Some(focus) = focus.as_ref().filter(|_| *active && *surface_ready) else {
                    served.borrow_mut().take();
                    return;
                };
                if served.borrow().as_ref() != Some(focus)
                    && serve_route(&sender, &surface, focus, &selections)
                {
                    *served.borrow_mut() = Some(focus.clone());
                }
            },
        );
    }
    {
        let sender = sender.clone();
        let surface = surface.clone();
        use_effect_with(
            (active, revision, surface_ready),
            move |(active, _, surface_ready)| {
                if *active && *surface_ready {
                    queue(&sender, &surface, AppRuntime::begin_direction_history);
                }
            },
        );
    }
    let focused = focus.is_some();
    let apply = {
        let sender = sender.clone();
        let surface = surface.clone();
        let selections = selections.clone();
        let navigate = navigate.clone();
        Callback::from(move |_: MouseEvent| {
            if apply_filters(&sender, &surface, &selections) {
                release_route(&navigate, focused);
            }
        })
    };
    let clear = {
        let sender = sender.clone();
        let surface = surface.clone();
        let selections = selections.clone();
        Callback::from(move |_: MouseEvent| {
            selections.reset(BTreeSet::new());
            queue(&sender, &surface, |runtime| {
                runtime.apply_direction_history_filters(String::new(), Vec::new(), Vec::new(), None)
            });
            release_route(&navigate, focused);
        })
    };
    let Selections {
        teams: selected_teams,
        agents: selected_agents,
        states: selected_states,
        validation,
    } = selections;
    let toggle_agent = toggle_set(&selected_agents);
    let toggle_team = toggle_set(&selected_teams);
    let toggle_state = toggle_set(&selected_states);
    let newer = read_on_click(&sender, &surface, AppRuntime::begin_newer_direction_history);
    let older = read_on_click(&sender, &surface, AppRuntime::begin_older_direction_history);
    let retry = read_on_click(&sender, &surface, AppRuntime::retry_direction_history);
    Controls {
        validation,
        selected_agents,
        selected_teams,
        selected_states,
        apply,
        clear,
        toggle_agent,
        toggle_team,
        toggle_state,
        newer,
        older,
        retry,
    }
}

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
    reserve: impl FnOnce(&mut CanvasRuntime) -> Option<crate::runtime::DirectionHistoryRequest>,
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

fn apply_filters(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    selected_teams: &BTreeSet<CanonicalUuid>,
    selected_agents: &BTreeSet<CanonicalUuid>,
    selected_states: &BTreeSet<wire::Disposition>,
    validation: &UseStateHandle<Option<String>>,
) {
    let query = input_value("direction-history-query");
    let cutoff = input_value("direction-history-cutoff");
    let authored_before = match crate::runtime::direction_history_authored_cutoff(&cutoff) {
        Ok(cutoff) => cutoff,
        Err(issue) => {
            validation.set(Some(issue));
            return;
        }
    };
    validation.set(None);
    let mut lanes = selected_agents.iter().copied().collect::<Vec<_>>();
    if let Some(topology) = surface.read().as_ref().and_then(CanvasRuntime::lanes) {
        lanes.extend(
            topology
                .lanes
                .iter()
                .filter(|team| selected_teams.contains(&team.lane))
                .flat_map(|team| team.members.iter().map(|member| member.lane)),
        );
    }
    let dispositions = selected_states.iter().copied().collect();
    queue(sender, surface, move |runtime| {
        runtime.apply_direction_history_filters(query, lanes, dispositions, authored_before)
    });
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
) -> Controls {
    let validation = use_state(|| None);
    let selected_teams = use_state(BTreeSet::new);
    let selected_agents = use_state(BTreeSet::new);
    let selected_states = use_state(BTreeSet::new);
    let (revision, surface_ready) = surface.read().as_ref().map_or((0, false), |runtime| {
        (runtime.direction_history().revision(), true)
    });
    {
        let sender = sender.clone();
        let surface = surface.clone();
        use_effect_with(
            (active, revision, surface_ready),
            move |(active, _, surface_ready)| {
                if *active && *surface_ready {
                    queue(&sender, &surface, CanvasRuntime::begin_direction_history);
                }
            },
        );
    }
    let apply = {
        let sender = sender.clone();
        let surface = surface.clone();
        let selected_teams = selected_teams.clone();
        let selected_agents = selected_agents.clone();
        let selected_states = selected_states.clone();
        let validation = validation.clone();
        Callback::from(move |_: MouseEvent| {
            apply_filters(
                &sender,
                &surface,
                &selected_teams,
                &selected_agents,
                &selected_states,
                &validation,
            );
        })
    };
    let clear = {
        let sender = sender.clone();
        let surface = surface.clone();
        let selected_teams = selected_teams.clone();
        let selected_agents = selected_agents.clone();
        let selected_states = selected_states.clone();
        let validation = validation.clone();
        Callback::from(move |_: MouseEvent| {
            clear_input("direction-history-query");
            clear_input("direction-history-cutoff");
            selected_teams.set(BTreeSet::new());
            selected_agents.set(BTreeSet::new());
            selected_states.set(BTreeSet::new());
            validation.set(None);
            queue(&sender, &surface, |runtime| {
                runtime.apply_direction_history_filters(String::new(), Vec::new(), Vec::new(), None)
            });
        })
    };
    let toggle_agent = toggle_set(&selected_agents);
    let toggle_team = toggle_set(&selected_teams);
    let toggle_state = toggle_set(&selected_states);
    let newer = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |_: MouseEvent| {
            queue(
                &sender,
                &surface,
                CanvasRuntime::begin_newer_direction_history,
            );
        })
    };
    let older = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |_: MouseEvent| {
            queue(
                &sender,
                &surface,
                CanvasRuntime::begin_older_direction_history,
            );
        })
    };
    let retry = {
        let sender = sender;
        let surface = surface;
        Callback::from(move |_: MouseEvent| {
            queue(&sender, &surface, CanvasRuntime::retry_direction_history);
        })
    };
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

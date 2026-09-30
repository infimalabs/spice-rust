//! Demand-driven settled-work pages for every team whose Work pane is shown.

use super::*;
use crate::runtime::TeamSettledRequest;

#[derive(Clone)]
pub(super) struct Controls {
    pub validation: UseStateHandle<BTreeMap<CanonicalUuid, String>>,
    pub apply: Callback<CanonicalUuid>,
    pub clear: Callback<CanonicalUuid>,
    pub newer: Callback<CanonicalUuid>,
    pub older: Callback<CanonicalUuid>,
    pub retry: Callback<CanonicalUuid>,
}

pub(super) fn query_id(team: CanonicalUuid) -> String {
    format!("team-work-history-query-{team}")
}

pub(super) fn cutoff_id(team: CanonicalUuid) -> String {
    format!("team-work-history-cutoff-{team}")
}

fn update_validation(
    validation: &UseStateHandle<BTreeMap<CanonicalUuid, String>>,
    team: CanonicalUuid,
    issue: Option<String>,
) {
    let mut issues = (**validation).clone();
    if let Some(issue) = issue {
        issues.insert(team, issue);
    } else {
        issues.remove(&team);
    }
    validation.set(issues);
}

fn queue(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    reserve: impl FnOnce(&mut CanvasRuntime) -> Option<TeamSettledRequest>,
) {
    let request = {
        let mut data = surface.write();
        let Some(runtime) = data.as_mut() else {
            return;
        };
        reserve(runtime)
    };
    let Some(request) = request else {
        return;
    };
    surface.set(surface.publication());
    if !admit_action(
        sender,
        ClientAction::TeamSettled {
            request: request.clone(),
        },
    ) {
        let mut data = surface.write();
        if let Some(runtime) = data.as_mut() {
            runtime.refuse_team_settled_request(
                &request,
                "Settled work request could not be queued.".to_owned(),
            );
        }
        drop(data);
        surface.set(surface.publication());
    }
}

/// Read each shown team's newest settled page once, and again only after an
/// open-board refresh saw work settle.
#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    shown: Vec<CanonicalUuid>,
) -> Controls {
    let validation = use_state(BTreeMap::new);
    let revision = surface
        .read()
        .as_ref()
        .map_or(0, CanvasRuntime::team_settled_revision);
    {
        let sender = sender.clone();
        let surface = surface.clone();
        use_effect_with((shown, revision), move |(shown, _)| {
            for &team in shown {
                queue(&sender, &surface, |runtime| {
                    runtime.begin_team_settled(team)
                });
            }
        });
    }
    let apply = {
        let sender = sender.clone();
        let surface = surface.clone();
        let validation = validation.clone();
        Callback::from(move |team: CanonicalUuid| {
            let query = super::work_history::input_value(&query_id(team));
            let cutoff = super::work_history::input_value(&cutoff_id(team));
            match crate::runtime::work_history_completion_cutoff(&cutoff) {
                Ok(completed_before) => {
                    update_validation(&validation, team, None);
                    queue(&sender, &surface, move |runtime| {
                        runtime.filter_team_settled(team, query, completed_before)
                    });
                }
                Err(issue) => update_validation(&validation, team, Some(issue)),
            }
        })
    };
    let clear = {
        let sender = sender.clone();
        let surface = surface.clone();
        let validation = validation.clone();
        Callback::from(move |team: CanonicalUuid| {
            super::work_history::clear_input(&query_id(team));
            super::work_history::clear_input(&cutoff_id(team));
            update_validation(&validation, team, None);
            queue(&sender, &surface, move |runtime| {
                runtime.filter_team_settled(team, String::new(), None)
            });
        })
    };
    let newer = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |team: CanonicalUuid| {
            queue(&sender, &surface, |runtime| {
                runtime.begin_newer_team_settled(team)
            });
        })
    };
    let older = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |team: CanonicalUuid| {
            queue(&sender, &surface, |runtime| {
                runtime.begin_older_team_settled(team)
            });
        })
    };
    let retry = Callback::from(move |team: CanonicalUuid| {
        queue(&sender, &surface, |runtime| {
            runtime.retry_team_settled(team)
        });
    });
    Controls {
        validation,
        apply,
        clear,
        newer,
        older,
        retry,
    }
}

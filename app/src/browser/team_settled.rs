//! Demand-driven settled-work pages for every team whose Work pane is shown.

use super::*;
use crate::runtime::TeamSettledRequest;

#[derive(Clone)]
pub(super) struct Controls {
    pub older: Callback<CanonicalUuid>,
    pub retry: Callback<CanonicalUuid>,
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
    Controls { older, retry }
}

//! Demand-driven typed relationship enrichment for the active task overview.

use super::*;
use crate::work::TaskOverviewSelection;

#[hook]
pub(super) fn use_request(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    selected: &UseStateHandle<Option<TaskOverviewSelection>>,
    map: &UseStateHandle<view::WorkMapState>,
) {
    let key = selected
        .as_ref()
        .as_ref()
        .and_then(|selection| map.request_key(selection));
    let revision = surface
        .read()
        .as_ref()
        .map_or(0, AppRuntime::task_relations_revision);
    let sender = sender.clone();
    let surface = surface.clone();
    use_effect_with((key, revision), move |(key, _)| {
        let Some(key) = key.clone() else {
            return;
        };
        let request = {
            let mut data = surface.write();
            let Some(runtime) = data.as_mut() else {
                return;
            };
            if !runtime.offers(CommandKind::TaskRelations)
                || !runtime.task_relations_needs_request(&key)
            {
                false
            } else {
                runtime.begin_task_relations(key.clone());
                true
            }
        };
        if request {
            surface.set(surface.publication());
            let _admitted = admit_action(&sender, ClientAction::TaskRelations { key });
        }
    });
}

pub(super) fn retry_callback(
    surface: UseStateHandle<SurfaceData>,
    selected: UseStateHandle<Option<TaskOverviewSelection>>,
    map: UseStateHandle<view::WorkMapState>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let Some(key) = selected
            .as_ref()
            .as_ref()
            .and_then(|selection| map.request_key(selection))
        else {
            return;
        };
        let retried = surface
            .write()
            .as_mut()
            .is_some_and(|runtime| runtime.retry_task_relations(&key));
        if retried {
            surface.set(surface.publication());
        }
    })
}

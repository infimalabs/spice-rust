//! Demand-driven typed relationship reads for the active Work map lens.

use super::*;

#[hook]
pub(super) fn use_request(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    selected: &UseStateHandle<Option<TaskStamp>>,
    map: &UseStateHandle<view::WorkMapState>,
) {
    let key = selected
        .as_ref()
        .and_then(|selected| map.request_key(selected));
    let revision = surface
        .read()
        .as_ref()
        .map_or(0, CanvasRuntime::task_relations_revision);
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

//! Demand-driven terminal Work paging.

use super::*;

#[derive(Clone)]
pub(super) struct Controls {
    pub open: UseStateHandle<bool>,
    pub toggle: Callback<Event>,
    pub load: Callback<MouseEvent>,
}

fn request(sender: &SenderHandle, surface: &UseStateHandle<SurfaceData>) {
    let request = {
        let mut data = surface.write();
        let Some(runtime) = data.as_mut() else {
            return;
        };
        if !runtime.offers(CommandKind::BoardRead) {
            return;
        }
        runtime.begin_work_history()
    };
    let Some(request) = request else {
        return;
    };
    surface.set(surface.publication());
    if !admit_action(
        sender,
        ClientAction::WorkHistory {
            request: request.clone(),
        },
    ) {
        let mut data = surface.write();
        if let Some(runtime) = data.as_mut() {
            runtime.refuse_work_history_request(
                request.revision,
                "History request could not be queued.".to_owned(),
            );
        }
        drop(data);
        surface.set(surface.publication());
    }
}

#[hook]
pub(super) fn use_controls(sender: SenderHandle, surface: UseStateHandle<SurfaceData>) -> Controls {
    let open = use_state(|| false);
    let revision = surface
        .read()
        .as_ref()
        .map_or(0, |runtime| runtime.work_history().revision());
    {
        let sender = sender.clone();
        let surface = surface.clone();
        use_effect_with((*open, revision), move |(open, _)| {
            if *open {
                request(&sender, &surface);
            }
        });
    }
    let toggle = {
        let open = open.clone();
        Callback::from(move |event: Event| {
            let details: web_sys::HtmlElement = event.target_unchecked_into();
            open.set(details.has_attribute("open"));
        })
    };
    let load = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |_: MouseEvent| request(&sender, &surface))
    };
    Controls { open, toggle, load }
}

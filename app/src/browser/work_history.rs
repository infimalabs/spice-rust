//! Demand-driven terminal Work archive controls.

use super::*;

#[derive(Clone)]
pub(super) struct Controls {
    pub open: UseStateHandle<bool>,
    pub query: UseStateHandle<String>,
    pub cutoff: UseStateHandle<String>,
    pub validation: UseStateHandle<Option<String>>,
    pub toggle: Callback<Event>,
    pub apply: Callback<MouseEvent>,
    pub clear: Callback<MouseEvent>,
    pub newer: Callback<MouseEvent>,
    pub older: Callback<MouseEvent>,
    pub retry: Callback<MouseEvent>,
}

fn queue(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    reserve: impl FnOnce(&mut CanvasRuntime) -> Option<crate::runtime::WorkHistoryRequest>,
) {
    let request = {
        let mut data = surface.write();
        let Some(runtime) = data.as_mut() else {
            return;
        };
        if !runtime.offers(CommandKind::BoardRead) {
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
        ClientAction::WorkHistory {
            request: request.clone(),
        },
    ) {
        let mut data = surface.write();
        if let Some(runtime) = data.as_mut() {
            runtime.refuse_work_history_request(
                &request,
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
    let query = use_state(String::new);
    let cutoff = use_state(String::new);
    let validation = use_state(|| None);
    let revision = surface
        .read()
        .as_ref()
        .map_or(0, |runtime| runtime.work_history().revision());
    {
        let sender = sender.clone();
        let surface = surface.clone();
        use_effect_with((*open, revision), move |(open, _)| {
            if *open {
                queue(&sender, &surface, CanvasRuntime::begin_work_history);
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
    let apply = {
        let sender = sender.clone();
        let surface = surface.clone();
        let query = query.clone();
        let cutoff = cutoff.clone();
        let validation = validation.clone();
        Callback::from(move |_: MouseEvent| {
            match crate::runtime::work_history_completion_cutoff(&cutoff) {
                Ok(completed_before) => {
                    validation.set(None);
                    let query = (*query).clone();
                    queue(&sender, &surface, move |runtime| {
                        runtime.apply_work_history_filters(query, completed_before)
                    });
                }
                Err(issue) => validation.set(Some(issue)),
            }
        })
    };
    let clear = {
        let sender = sender.clone();
        let surface = surface.clone();
        let query = query.clone();
        let cutoff = cutoff.clone();
        let validation = validation.clone();
        Callback::from(move |_: MouseEvent| {
            query.set(String::new());
            cutoff.set(String::new());
            validation.set(None);
            queue(&sender, &surface, |runtime| {
                runtime.apply_work_history_filters(String::new(), None)
            });
        })
    };
    let newer = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |_: MouseEvent| {
            queue(&sender, &surface, CanvasRuntime::begin_newer_work_history);
        })
    };
    let older = {
        let sender = sender.clone();
        let surface = surface.clone();
        Callback::from(move |_: MouseEvent| {
            queue(&sender, &surface, CanvasRuntime::begin_older_work_history);
        })
    };
    let retry = {
        let sender = sender;
        let surface = surface;
        Callback::from(move |_: MouseEvent| {
            queue(&sender, &surface, CanvasRuntime::retry_work_history);
        })
    };
    Controls {
        open,
        query,
        cutoff,
        validation,
        toggle,
        apply,
        clear,
        newer,
        older,
        retry,
    }
}

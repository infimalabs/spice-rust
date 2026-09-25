use super::*;

const TASK_PANEL_TRANSITION_MILLIS: u32 = 180;

pub(super) struct Card {
    pub(super) context: Option<crate::browser::message_task_inspector::Context>,
    pub(super) inspection: Option<crate::browser::message_task_inspector::Inspection>,
    pub(super) root: NodeRef,
}

#[hook]
pub(super) fn use_card(card: String) -> Card {
    let context = use_context::<crate::browser::message_task_inspector::Context>();
    let inspection = context.as_ref().and_then(|context| {
        (*context.inspection)
            .as_ref()
            .filter(|inspection| inspection.card == card)
            .cloned()
    });
    let root = use_node_ref();
    use_inspector_focus(
        &root,
        inspection
            .as_ref()
            .is_some_and(|inspection| inspection.open),
    );
    use_close_completion(&context, &inspection);
    use_focus_departure(
        &card,
        &context,
        inspection
            .as_ref()
            .is_some_and(|inspection| inspection.open),
    );
    Card {
        context,
        inspection,
        root,
    }
}

#[hook]
fn use_inspector_focus(root: &NodeRef, open: bool) {
    let root = root.clone();
    use_effect_with(open, move |open| {
        if *open {
            gloo_timers::callback::Timeout::new(0, move || {
                if let Some(element) = root.cast::<web_sys::HtmlElement>() {
                    focus_without_scrolling(&element);
                }
            })
            .forget();
        }
    });
}

#[hook]
fn use_close_completion(
    context: &Option<crate::browser::message_task_inspector::Context>,
    inspection: &Option<crate::browser::message_task_inspector::Inspection>,
) {
    let dependency = (
        context.clone(),
        inspection
            .as_ref()
            .filter(|inspection| !inspection.open)
            .map(|inspection| inspection.card.clone()),
    );
    use_effect_with(dependency, move |(context, closing_card)| {
        let timeout = closing_card.as_ref().and_then(|card| {
            let context = context.clone()?;
            let card = card.clone();
            Some(gloo_timers::callback::Timeout::new(
                TASK_PANEL_TRANSITION_MILLIS,
                move || context.complete_close(&card),
            ))
        });
        move || drop(timeout)
    });
}

#[hook]
fn use_focus_departure(
    card: &str,
    context: &Option<crate::browser::message_task_inspector::Context>,
    open: bool,
) {
    let context = context.clone();
    let card = card.to_owned();
    use_effect_with(open, move |open| {
        let registration = (*open)
            .then(|| {
                let context = context.clone()?;
                let document = web_sys::window()?.document()?;
                let observed = document.clone();
                let card = card.clone();
                let callback = Closure::<dyn FnMut(web_sys::FocusEvent)>::new(
                    move |event: web_sys::FocusEvent| {
                        let stayed_inside = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                            .and_then(|target| {
                                observed
                                    .get_element_by_id(&card)?
                                    .query_selector(".message-task-inspector")
                                    .ok()
                                    .flatten()
                                    .map(|inspector| inspector.contains(Some(&target)))
                            })
                            .unwrap_or(false);
                        if !stayed_inside {
                            context.close(false);
                        }
                    },
                );
                document
                    .add_event_listener_with_callback("focusin", callback.as_ref().unchecked_ref())
                    .ok()?;
                Some((document, callback))
            })
            .flatten();
        move || {
            if let Some((document, callback)) = registration {
                let _ = document.remove_event_listener_with_callback(
                    "focusin",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

pub(super) fn task_chip_click(
    inspector: Option<crate::browser::message_task_inspector::Context>,
    card: String,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        let Some(handle) = task_handle_from_target(event.target()) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        if let Some(inspector) = inspector.as_ref() {
            inspector.inspect(card.clone(), handle);
        }
    })
}

pub(super) fn task_chip_keydown(
    inspector: Option<crate::browser::message_task_inspector::Context>,
    card: String,
) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() != "Enter" && event.key() != " " {
            return;
        }
        let Some(handle) = task_handle_from_target(event.target()) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        if let Some(inspector) = inspector.as_ref() {
            inspector.inspect(card.clone(), handle);
        }
    })
}

fn task_handle_from_target(target: Option<web_sys::EventTarget>) -> Option<wire::TaskHandle> {
    target
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".message-task-chip").ok().flatten())?
        .get_attribute("data-task-handle")
        .and_then(|handle| handle.parse().ok())
}

pub(super) fn close_inspector(
    inspector: Option<crate::browser::message_task_inspector::Context>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        if let Some(inspector) = inspector.as_ref() {
            inspector.close(true);
        }
    })
}

pub(super) fn focus_without_scrolling(element: &web_sys::HtmlElement) {
    let options = web_sys::FocusOptions::new();
    options.set_prevent_scroll(true);
    let _ = element.focus_with_options(&options);
}

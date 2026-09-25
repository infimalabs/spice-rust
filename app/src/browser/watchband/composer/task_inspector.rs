use super::*;
use crate::browser::message_task_inspector::{Context, Inspection};

pub(super) struct Card {
    pub(super) context: Option<Context>,
    pub(super) inspection: Option<Inspection>,
    pub(super) root: NodeRef,
    /// The sliding task layer whose own transition ends a close.
    pub(super) layer: NodeRef,
}

#[hook]
pub(super) fn use_card(card: String) -> Card {
    let context = use_context::<Context>();
    let inspection = context.as_ref().and_then(|context| {
        (*context.inspection)
            .as_ref()
            .filter(|inspection| inspection.card == card)
            .cloned()
    });
    let root = use_node_ref();
    let layer = use_node_ref();
    let open = inspection
        .as_ref()
        .is_some_and(|inspection| inspection.open);
    use_inspector_focus(&root, open);
    let complete = {
        let context = context.clone();
        let card = card.clone();
        Callback::from(move |()| {
            if let Some(context) = &context {
                context.complete_close(&card);
            }
        })
    };
    crate::browser::overlay::use_close_transition(
        inspection.is_some() && !open,
        !crate::browser::overlay::reduced_motion(),
        layer.clone(),
        complete,
    );
    use_chip_return(&inspection);
    use_focus_departure(&card, &context, open);
    Card {
        context,
        inspection,
        root,
        layer,
    }
}

/// Focus enters the task view once it opens. The effect runs after the view
/// is in the document, so focus needs no deferral.
#[hook]
fn use_inspector_focus(root: &NodeRef, open: bool) {
    let root = root.clone();
    use_effect_with(open, move |open| {
        if *open && let Some(element) = root.cast::<web_sys::HtmlElement>() {
            focus_without_scrolling(&element);
        }
    });
}

/// A view closed toward its chip returns focus to that chip once the view has
/// ended and the card renders the chip interactive again.
#[hook]
fn use_chip_return(inspection: &Option<Inspection>) {
    let last = use_mut_ref(|| None::<Inspection>);
    use_effect_with(inspection.clone(), move |inspection| {
        let ended = last.replace(inspection.clone());
        if inspection.is_some() {
            return;
        }
        let Some(ended) = ended.filter(|ended| ended.return_focus) else {
            return;
        };
        let selector = format!(".message-task-chip[data-task-handle=\"{}\"]", ended.handle);
        if let Some(chip) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(&ended.card))
            .and_then(|card| card.query_selector(&selector).ok().flatten())
            .and_then(|chip| chip.dyn_into::<web_sys::HtmlElement>().ok())
        {
            focus_without_scrolling(&chip);
        }
    });
}

#[hook]
fn use_focus_departure(card: &str, context: &Option<Context>, open: bool) {
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
                        let element = observed.get_element_by_id(&card);
                        // A card that left view ends with its surface, not with focus.
                        if element
                            .as_ref()
                            .is_some_and(|card| !crate::browser::overlay::returnable(card))
                        {
                            return;
                        }
                        let stayed_inside = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                            .zip(element)
                            .is_some_and(|(target, card)| {
                                card.query_selector(".message-task-inspector")
                                    .ok()
                                    .flatten()
                                    .is_some_and(|inspector| inspector.contains(Some(&target)))
                            });
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

pub(super) fn task_chip_click(inspector: Option<Context>, card: String) -> Callback<MouseEvent> {
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
    inspector: Option<Context>,
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

pub(super) fn close_inspector(inspector: Option<Context>) -> Callback<MouseEvent> {
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

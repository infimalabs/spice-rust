use super::*;

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
    use_inspector_focus(&root, inspection.is_some());
    use_frame_guard(&inspection);
    use_focus_departure(&card, &context, inspection.is_some());
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
fn use_frame_guard(inspection: &Option<crate::browser::message_task_inspector::Inspection>) {
    use_effect_with(inspection.clone(), move |inspection| {
        let registration = inspection.as_ref().and_then(|inspection| {
            crate::browser::message_task_inspector::restore_frame(inspection);
            let document = web_sys::window()?.document()?;
            let card = document.get_element_by_id(&inspection.card)?;
            let plane = card.closest(".lane-mosaic-plane").ok().flatten()?;
            let inspection = inspection.clone();
            let callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |_, _| {
                    crate::browser::message_task_inspector::restore_frame(&inspection);
                });
            let observer =
                web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()).ok()?;
            let options = web_sys::MutationObserverInit::new();
            options.set_attributes(true);
            observer.observe_with_options(&card, &options).ok()?;
            observer.observe_with_options(&plane, &options).ok()?;
            Some((observer, callback))
        });
        move || {
            if let Some((observer, _callback)) = registration {
                observer.disconnect();
            }
        }
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
                            context.close();
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
        let Some((handle, scroll_top, layout)) = task_inspection_from_target(event.target()) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        if let Some(inspector) = inspector.as_ref() {
            inspector.inspect(card.clone(), handle, scroll_top, layout);
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
        let Some((handle, scroll_top, layout)) = task_inspection_from_target(event.target()) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        if let Some(inspector) = inspector.as_ref() {
            inspector.inspect(card.clone(), handle, scroll_top, layout);
        }
    })
}

fn task_inspection_from_target(
    target: Option<web_sys::EventTarget>,
) -> Option<(
    wire::TaskHandle,
    i32,
    crate::browser::message_task_inspector::LayoutSnapshot,
)> {
    let chip = target
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".message-task-chip").ok().flatten())?;
    let handle = chip
        .get_attribute("data-task-handle")
        .and_then(|handle| handle.parse().ok())?;
    let card = chip.closest(".mosaic-message-card").ok().flatten()?;
    let plane = chip.closest(".lane-mosaic-plane").ok().flatten()?;
    let layout = crate::browser::message_task_inspector::LayoutSnapshot {
        card: [
            layout_integer(&card, "data-base")?,
            layout_integer(&card, "data-rows")?,
            layout_integer(&card, "data-track")?,
            layout_integer(&card, "data-span")?,
            layout_integer(&card, "data-measure-span")?,
        ],
        plane: [
            layout_integer(&plane, "data-max-row")?,
            layout_integer(&plane, "data-plane-rows")?,
        ],
    };
    Some((handle, plane.scroll_top(), layout))
}

fn layout_integer(element: &web_sys::Element, attribute: &str) -> Option<i32> {
    element.get_attribute(attribute)?.parse().ok()
}

pub(super) fn close_inspector(
    inspector: Option<crate::browser::message_task_inspector::Context>,
    card: String,
    handle: Option<String>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let Some(inspector) = inspector.as_ref() else {
            return;
        };
        inspector.close();
        let card = card.clone();
        let handle = handle.clone();
        gloo_timers::callback::Timeout::new(0, move || {
            let Some(handle) = handle else {
                return;
            };
            let Some(card) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id(&card))
            else {
                return;
            };
            let selector = format!(".message-task-chip[data-task-handle=\"{handle}\"]");
            if let Ok(Some(chip)) = card.query_selector(&selector)
                && let Ok(chip) = chip.dyn_into::<web_sys::HtmlElement>()
            {
                focus_without_scrolling(&chip);
            }
        })
        .forget();
    })
}

pub(super) fn focus_without_scrolling(element: &web_sys::HtmlElement) {
    let options = web_sys::FocusOptions::new();
    options.set_prevent_scroll(true);
    let _ = element.focus_with_options(&options);
}

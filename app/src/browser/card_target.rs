use wasm_bindgen::JsCast as _;
use yew::prelude::*;

use crate::CanvasRuntime;

use super::SurfaceData;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Status {
    pub(super) target: String,
    pub(super) available: bool,
}

/// Focus a card named by the current route once retained Chat state is ready.
///
/// Missing cards are retried when the stream cursor advances because the
/// target may arrive during staged hydration.
#[hook]
pub(super) fn use_target(
    target: Option<String>,
    surface: &SurfaceData,
    status: UseStateHandle<Option<Status>>,
    visible: bool,
) {
    let (ready, revision) = {
        let surface = surface.read();
        let runtime = surface.as_ref();
        (
            runtime.and_then(CanvasRuntime::lanes).is_some(),
            runtime
                .and_then(CanvasRuntime::resume)
                .map(|cursor| cursor.seq.to_string()),
        )
    };
    use_effect_with(
        (target, visible, ready, revision),
        move |(target, visible, ready, _)| {
            let Some(target) = target.as_deref().filter(|_| *visible) else {
                if status.is_some() {
                    status.set(None);
                }
                return;
            };
            if !ready
                || status
                    .as_ref()
                    .is_some_and(|settled| settled.target == target && settled.available)
            {
                return;
            }
            let element = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id(target));
            let available = element.is_some();
            if let Some(element) = element {
                element.scroll_into_view();
                if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
                    let _ = element.focus();
                }
            }
            status.set(Some(Status {
                target: target.to_owned(),
                available,
            }));
        },
    );
}

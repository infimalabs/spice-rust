//! The editor's toolbar and the marks drawn over the image.

use super::*;
use crate::image_annotation::stroke_width;

pub(super) struct Actions {
    pub(super) undo: Callback<MouseEvent>,
    pub(super) redo: Callback<MouseEvent>,
    pub(super) delete: Callback<MouseEvent>,
    pub(super) cancel: Callback<MouseEvent>,
    pub(super) apply: Callback<MouseEvent>,
}

pub(super) fn toolbar(
    tool: &UseStateHandle<Tool>,
    busy: bool,
    ready: bool,
    history: &History,
    actions: Actions,
) -> Html {
    html! {
        <div class="image-lightbox-actions image-annotation-controls" role="toolbar" aria-label="Image annotation tools">
            <span class="image-annotation-tools app-control-rail">
                {for [(Tool::Select, "Select"), (Tool::Rectangle, "Rectangle"), (Tool::Arrow, "Arrow")].into_iter().map(|(value, label)| {
                    let tool = tool.clone();
                    html! { <button type="button" aria-pressed={(*tool == value).to_string()} disabled={busy} onclick={Callback::from(move |_| tool.set(value))}>{label}</button> }
                })}
                <button type="button" disabled={busy || !history.can_undo()} onclick={actions.undo}>{"Undo"}</button>
                <button type="button" disabled={busy || !history.can_redo()} onclick={actions.redo}>{"Redo"}</button>
                <button type="button" disabled={busy || history.selected.is_none()} onclick={actions.delete}>{"Delete"}</button>
            </span>
            <span class="image-annotation-tools app-control-rail">
                <button type="button" onclick={actions.cancel}>{"Cancel"}</button>
                <button type="button" class="image-annotation-apply" disabled={busy || !ready} onclick={actions.apply}>{"Apply"}</button>
            </span>
        </div>
    }
}

/// Every mark in image pixels, with handles on the selected one sized to stay
/// eight screen pixels at the current fit.
pub(super) fn marks(
    session: &Session,
    history: &History,
    dimensions: (u32, u32),
    viewport: (f64, f64),
) -> Html {
    let stroke = stroke_width(dimensions.0, dimensions.1);
    let scale = (viewport.0 / f64::from(dimensions.0)).min(viewport.1 / f64::from(dimensions.1));
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let handle_radius = (8.0 / scale).to_string();
    html! {
        {for history.shapes.iter().enumerate().map(|(index, shape)| {
            let selected = history.selected == Some(index);
            let session = session.clone();
            let focused = Callback::from(move |_: FocusEvent| {
                session.history.borrow_mut().selected = Some(index);
                session.repaint.emit(());
            });
            html! {
                <g data-annotation-shape={index.to_string()} data-selected={selected.to_string()} tabindex="0" role="button" aria-label={format!("{} {}", if shape.kind == Kind::Rectangle { "Rectangle" } else { "Arrow" }, index + 1)} onfocus={focused}>
                    {shape::render(*shape, stroke)}
                    if selected {
                        {for [("start", shape.start), ("end", shape.end)].into_iter().map(|(handle, point)| html! {
                            <circle class="image-annotation-handle" data-annotation-handle={handle} cx={point.x.to_string()} cy={point.y.to_string()} r={handle_radius.clone()} />
                        })}
                    }
                </g>
            }
        })}
    }
}

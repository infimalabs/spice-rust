//! The editor's toolbar and the marks drawn over the image.

use super::*;
use crate::image_annotation::stroke_width;

pub(super) struct Actions {
    pub(super) tool: Callback<Tool>,
    pub(super) undo: Callback<MouseEvent>,
    pub(super) redo: Callback<MouseEvent>,
    pub(super) delete: Callback<MouseEvent>,
    pub(super) route: Callback<MouseEvent>,
    pub(super) color: Callback<Color>,
    pub(super) cancel: Callback<MouseEvent>,
    pub(super) apply: Callback<MouseEvent>,
}

/// The drawing tools, the route control, the palette, then history. The route
/// control cycles the selected line or arrow's route, or the route new ones
/// start with, for pointers without modifier keys; it stays in place,
/// disabled, while neither applies so the rail never reflows. The palette
/// shows the selected mark's colour, or else the colour new marks take.
pub(super) fn toolbar(
    tool: Tool,
    pen: Pen,
    busy: bool,
    ready: bool,
    history: &History,
    actions: Actions,
) -> Html {
    let selected = history.selected.and_then(|index| history.shapes.get(index));
    let selected_route = selected
        .filter(|shape| shape.kind.routed())
        .map(|shape| shape.route);
    let routing = selected_route.is_some() || matches!(tool, Tool::Line | Tool::Arrow);
    let shown = selected_route.unwrap_or(pen.route);
    let ink = selected.map_or(pen.color, |shape| shape.color);
    let swatches = Color::ALL.into_iter().map(|color| {
        let choose = actions.color.reform(move |_: MouseEvent| color);
        html! {
            <button type="button" class="image-annotation-swatch" aria-pressed={(color == ink).to_string()} aria-label={format!("Colour: {}", color.label())} title={color.label()} disabled={busy} onclick={choose}>
                <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill={color.ink()} stroke={color.halo()} stroke-width="1.5" /></svg>
            </button>
        }
    });
    html! {
        <div class="image-lightbox-actions image-annotation-controls" role="toolbar" aria-label="Image annotation tools">
            <span class="image-annotation-tools app-control-rail app-overlay-control">
                {for [(Tool::Select, "Select"), (Tool::Rectangle, "Rectangle"), (Tool::Line, "Line"), (Tool::Arrow, "Arrow"), (Tool::Text, "Text")].into_iter().map(|(value, label)| {
                    let choose = actions.tool.reform(move |_: MouseEvent| value);
                    html! { <button type="button" aria-pressed={(tool == value).to_string()} disabled={busy} onclick={choose}>{label}</button> }
                })}
                <button type="button" class="image-annotation-route" disabled={busy || !routing} aria-label={format!("Route: {}", shown.label())} title="Route: Straight, Elbow (Alt), or Curve (Command or Control); Shift snaps straight routes to 45 degrees" onclick={actions.route}>{shown.label()}</button>
                <span class="image-annotation-palette" role="group" aria-label="Mark colour">{for swatches}</span>
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
    let typing = session
        .label
        .borrow()
        .as_ref()
        .map(|typing| (typing.index, typing.serial));
    // Every mark is keyed so a label entry, keyed by its own serial, mounts a
    // fresh field even where an abandoned label stood.
    html! {
        {for history.shapes.iter().enumerate().map(|(index, shape)| {
            let selected = history.selected == Some(index);
            if let Some((_, serial)) = typing.filter(|(typing, _)| *typing == index) {
                return html! {
                    <g key={format!("label-{serial}")} data-annotation-shape={index.to_string()} data-selected="true">
                        <label::Field session={session.clone()} {serial} shape={shape.clone()} {dimensions} />
                    </g>
                };
            }
            let session = session.clone();
            let focused = Callback::from(move |_: FocusEvent| {
                session.history.borrow_mut().selected = Some(index);
                session.repaint.emit(());
            });
            let (low, high) = shape.bounds();
            let name = if shape.kind == Kind::Text {
                format!("Text {}: {}", index + 1, shape.text)
            } else {
                format!("{} {}", shape.kind.label(), index + 1)
            };
            html! {
                <g key={index.to_string()} data-annotation-shape={index.to_string()} data-selected={selected.to_string()} tabindex="0" role="button" aria-label={name} onfocus={focused}>
                    {shape::render(shape, stroke)}
                    if selected && shape.kind == Kind::Text {
                        <rect class="image-annotation-outline" x={low.x.to_string()} y={low.y.to_string()} width={(high.x-low.x).to_string()} height={(high.y-low.y).to_string()} />
                    } else if selected {
                        {for [("start", shape.start), ("end", shape.end)].into_iter().map(|(handle, point)| html! {
                            <circle class="image-annotation-handle" data-annotation-handle={handle} cx={point.x.to_string()} cy={point.y.to_string()} r={handle_radius.clone()} />
                        })}
                    }
                </g>
            }
        })}
    }
}

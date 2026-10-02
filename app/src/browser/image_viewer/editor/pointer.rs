//! Pointer gestures on the annotation canvas: draw a mark, move one, or drag
//! one of its handles, all measured in the image's own pixels.

use super::*;

/// The SVG the gestures land on and the image size its viewBox spans.
#[derive(Clone, PartialEq)]
pub(super) struct DrawingSurface {
    pub(super) svg: NodeRef,
    pub(super) dimensions: (u32, u32),
}

fn position(canvas: &DrawingSurface, event: &PointerEvent, clamp: bool) -> Option<Point> {
    let element = canvas.svg.cast::<web_sys::Element>()?;
    let bounds = element.get_bounding_client_rect();
    let (width, height) = (
        f64::from(canvas.dimensions.0),
        f64::from(canvas.dimensions.1),
    );
    let scale = (bounds.width() / width).min(bounds.height() / height);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let x =
        (f64::from(event.client_x()) - bounds.x() - (bounds.width() - width * scale) / 2.0) / scale;
    let y = (f64::from(event.client_y()) - bounds.y() - (bounds.height() - height * scale) / 2.0)
        / scale;
    if !clamp && (x < 0.0 || y < 0.0 || x > width || y > height) {
        return None;
    }
    Some(Point {
        x: x.round().clamp(0.0, width) as i32,
        y: y.round().clamp(0.0, height) as i32,
    })
}

pub(super) fn cancel(history: &mut History, gesture: &mut Option<Gesture>) {
    if let Some(gesture) = gesture.take() {
        history.shapes = gesture.before;
        history.selected = None;
    }
}

/// What a press begins: an edit of the mark it hit when the Select tool is
/// active or a handle was grabbed, otherwise a new mark of the drawing tool.
/// A Select press on bare image only clears the selection and begins nothing.
fn begin(
    history: &mut History,
    tool: Tool,
    event: &PointerEvent,
    point: Point,
) -> Option<(Motion, Vec<Shape>)> {
    let hit = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok());
    let handle = hit
        .as_ref()
        .and_then(|target| target.get_attribute("data-annotation-handle"));
    let index = hit
        .and_then(|target| target.closest("[data-annotation-shape]").ok().flatten())
        .and_then(|target| target.get_attribute("data-annotation-shape"))
        .and_then(|index| index.parse::<usize>().ok())
        .filter(|index| {
            *index < history.shapes.len() && (tool == Tool::Select || handle.is_some())
        });
    if let Some(index) = index {
        history.selected = Some(index);
        let motion = match handle.as_deref() {
            Some("start") => Motion::Start(index),
            Some("end") => Motion::End(index),
            _ => Motion::Move(index, history.shapes[index], point),
        };
        return Some((motion, history.shapes.clone()));
    }
    let kind = match tool {
        Tool::Select => {
            history.selected = None;
            return None;
        }
        Tool::Rectangle => Kind::Rectangle,
        Tool::Arrow => Kind::Arrow,
    };
    let before = history.shapes.clone();
    let index = history.shapes.len();
    history.shapes.push(Shape {
        kind,
        start: point,
        end: point,
    });
    history.selected = Some(index);
    Some((Motion::Draw(index), before))
}

pub(super) fn down(
    event: &PointerEvent,
    canvas: &DrawingSurface,
    surface: &NodeRef,
    session: &Session,
    tool: Tool,
) {
    if event.button() != 0 || session.gesture.borrow().is_some() {
        return;
    }
    *session.gesture_click.borrow_mut() = false;
    let Some(point) = position(canvas, event, false) else {
        return;
    };
    event.prevent_default();
    if let Some(surface) = surface.cast::<web_sys::HtmlElement>() {
        let _ = surface.focus();
    }
    let begun = begin(&mut session.history.borrow_mut(), tool, event, point);
    if let Some((motion, before)) = begun {
        if let Some(element) = canvas.svg.cast::<web_sys::Element>() {
            let _ = element.set_pointer_capture(event.pointer_id());
        }
        *session.gesture.borrow_mut() = Some(Gesture {
            pointer: event.pointer_id(),
            motion,
            before,
        });
    }
    session.repaint.emit(());
}

pub(super) fn moved(event: &PointerEvent, canvas: &DrawingSurface, session: &Session) {
    let gesture = session.gesture.borrow();
    let Some(gesture) = gesture
        .as_ref()
        .filter(|gesture| gesture.pointer == event.pointer_id())
    else {
        return;
    };
    let Some(point) = position(canvas, event, true) else {
        return;
    };
    event.prevent_default();
    let mut history = session.history.borrow_mut();
    match gesture.motion {
        Motion::Draw(index) | Motion::End(index) => history.shapes[index].end = point,
        Motion::Start(index) => history.shapes[index].start = point,
        Motion::Move(index, original, start) => {
            history.shapes[index] = original.translated(
                point.x - start.x,
                point.y - start.y,
                canvas.dimensions.0 as i32,
                canvas.dimensions.1 as i32,
            )
        }
    }
    session.repaint.emit(());
}

/// Release ends the gesture as one undoable edit; a mark too small to see is
/// dropped rather than committed.
pub(super) fn up(event: &PointerEvent, canvas: &DrawingSurface, session: &Session) {
    if !session.owns(event) {
        return;
    }
    *session.gesture_click.borrow_mut() = true;
    moved(event, canvas, session);
    let finished = session.gesture.borrow_mut().take().expect("owned gesture");
    let mut history = session.history.borrow_mut();
    if let Some(index) = history.selected
        && !history.shapes[index].visible()
    {
        history.shapes.remove(index);
        history.selected = None;
    }
    history.commit(finished.before);
    if let Some(element) = canvas.svg.cast::<web_sys::Element>() {
        let _ = element.release_pointer_capture(event.pointer_id());
    }
    session.repaint.emit(());
}

pub(super) fn interrupted(event: &PointerEvent, session: &Session) {
    if session.owns(event) {
        *session.gesture_click.borrow_mut() = true;
        cancel(
            &mut session.history.borrow_mut(),
            &mut session.gesture.borrow_mut(),
        );
        session.repaint.emit(());
    }
}

/// A click on the SVG root outside any gesture clears selection without
/// dismissing the editor; the click a finished gesture generates is swallowed.
pub(super) fn backdrop(event: &MouseEvent, svg: &NodeRef, session: &Session) {
    if std::mem::take(&mut *session.gesture_click.borrow_mut()) {
        return;
    }
    if session.gesture.borrow().is_none()
        && event.target().is_some_and(|target| {
            svg.cast::<web_sys::Element>()
                .is_some_and(|svg| JsValue::from(svg) == JsValue::from(target))
        })
    {
        session.history.borrow_mut().selected = None;
        session.repaint.emit(());
    }
}

//! Pointer gestures on the annotation canvas: draw a mark, move one, drag one
//! of its handles, or place a label, all measured in the image's own pixels.

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

/// The route a line or arrow takes while these modifiers are held: Alt or
/// Option turns elbows, Command or Control bends a curve, and otherwise the
/// toolbar's route applies.
fn route_for(event: &PointerEvent, chosen: Route) -> Route {
    if event.alt_key() {
        Route::Elbow
    } else if event.meta_key() || event.ctrl_key() {
        Route::Curve
    } else {
        chosen
    }
}

/// The mark under the press and the handle grabbed on it, if any.
fn hit(history: &History, event: &PointerEvent) -> (Option<usize>, Option<String>) {
    let target = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok());
    let handle = target
        .as_ref()
        .and_then(|target| target.get_attribute("data-annotation-handle"));
    let index = target
        .and_then(|target| target.closest("[data-annotation-shape]").ok().flatten())
        .and_then(|target| target.get_attribute("data-annotation-shape"))
        .and_then(|index| index.parse::<usize>().ok())
        .filter(|index| *index < history.shapes.len());
    (index, handle)
}

/// What a press begins: an edit of the mark it hit when the Select tool is
/// active or a handle was grabbed, otherwise a new mark of the drawing tool.
/// A Select press on bare image only clears the selection and begins nothing.
fn begin(
    history: &mut History,
    tool: Tool,
    route: Route,
    event: &PointerEvent,
    point: Point,
) -> Option<(Motion, Vec<Shape>)> {
    let (index, handle) = hit(history, event);
    if let Some(index) = index.filter(|_| tool == Tool::Select || handle.is_some()) {
        history.selected = Some(index);
        let motion = match handle.as_deref() {
            Some("start") => Motion::Start(index),
            Some("end") => Motion::End(index),
            _ => Motion::Move(index, history.shapes[index].clone(), point),
        };
        return Some((motion, history.shapes.clone()));
    }
    let kind = match tool {
        Tool::Select | Tool::Text => {
            history.selected = None;
            return None;
        }
        Tool::Rectangle => Kind::Rectangle,
        Tool::Line => Kind::Line,
        Tool::Arrow => Kind::Arrow,
    };
    let before = history.shapes.clone();
    let index = history.shapes.len();
    history
        .shapes
        .push(Shape::new(kind, route_for(event, route), point, point));
    history.selected = Some(index);
    Some((Motion::Draw(index), before))
}

pub(super) fn down(
    event: &PointerEvent,
    canvas: &DrawingSurface,
    surface: &NodeRef,
    session: &Session,
    tool: Tool,
    route: Route,
) {
    if event.button() != 0 || session.gesture.borrow().is_some() {
        return;
    }
    *session.gesture_click.borrow_mut() = false;
    let Some(point) = position(canvas, event, false) else {
        return;
    };
    event.prevent_default();
    // A press elsewhere keeps the label being typed before it acts, then
    // takes the keyboard back to the stage.
    label::finish(session, true);
    if let Some(surface) = surface.cast::<web_sys::HtmlElement>() {
        let _ = surface.focus();
    }
    if tool == Tool::Text {
        let (index, handle) = hit(&session.history.borrow(), event);
        let editable = index.filter(|index| {
            handle.is_none() && session.history.borrow().shapes[*index].kind == Kind::Text
        });
        if handle.is_none() {
            match editable {
                Some(index) => label::edit(session, index),
                None => label::place(session, point, canvas.dimensions),
            }
            session.repaint.emit(());
            return;
        }
    }
    let begun = begin(&mut session.history.borrow_mut(), tool, route, event, point);
    if let Some((motion, before)) = begun {
        if let Some(element) = canvas.svg.cast::<web_sys::Element>() {
            let _ = element.set_pointer_capture(event.pointer_id());
        }
        *session.gesture.borrow_mut() = Some(Gesture {
            pointer: event.pointer_id(),
            motion,
            before,
            route,
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
    match &gesture.motion {
        Motion::Draw(index) => {
            let shape = &mut history.shapes[*index];
            if shape.kind.routed() {
                shape.route = route_for(event, gesture.route);
            }
            shape.end = steered(event, shape, shape.start, point);
        }
        Motion::End(index) => {
            let shape = &mut history.shapes[*index];
            shape.end = steered(event, shape, shape.start, point);
        }
        Motion::Start(index) => {
            let shape = &mut history.shapes[*index];
            shape.start = steered(event, shape, shape.end, point);
        }
        Motion::Move(index, original, start) => {
            history.shapes[*index] = original.translated(
                point.x - start.x,
                point.y - start.y,
                canvas.dimensions.0 as i32,
                canvas.dimensions.1 as i32,
            )
        }
    }
    session.repaint.emit(());
}

/// Shift holds a straight line or arrow to 45-degree steps from its fixed end.
fn steered(event: &PointerEvent, shape: &Shape, anchor: Point, point: Point) -> Point {
    if event.shift_key() && shape.kind.routed() && shape.route == Route::Straight {
        snapped(anchor, point)
    } else {
        point
    }
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

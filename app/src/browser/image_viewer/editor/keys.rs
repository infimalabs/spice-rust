//! Keyboard commands on the focused annotation stage.

use super::*;

pub(super) fn handle(event: &KeyboardEvent, session: &Session, dimensions: (u32, u32)) {
    let handled = {
        let mut gesture = session.gesture.borrow_mut();
        if gesture.is_some() {
            *session.gesture_click.borrow_mut() = true;
        }
        command(
            event,
            &mut session.history.borrow_mut(),
            &mut gesture,
            dimensions,
        )
    };
    if handled {
        event.prevent_default();
        event.stop_propagation();
        session.repaint.emit(());
    }
}

/// Escape abandons the gesture or selection, Mod+Z / Mod+Shift+Z / Mod+Y walk
/// history, Delete or Backspace removes the selection, and arrows nudge it by
/// one pixel (ten with Shift). Returns whether the key was an editor command.
fn command(
    event: &KeyboardEvent,
    history: &mut History,
    gesture: &mut Option<Gesture>,
    dimensions: (u32, u32),
) -> bool {
    let key = event.key();
    let modified = event.meta_key() || event.ctrl_key();
    if key == "Escape" && (gesture.is_some() || history.selected.is_some()) {
        pointer::cancel(history, gesture);
        history.selected = None;
    } else if modified && key.eq_ignore_ascii_case("z") {
        pointer::cancel(history, gesture);
        if event.shift_key() {
            history.redo();
        } else {
            history.undo();
        }
    } else if modified && key.eq_ignore_ascii_case("y") {
        pointer::cancel(history, gesture);
        history.redo();
    } else if matches!(key.as_str(), "Delete" | "Backspace") && history.selected.is_some() {
        pointer::cancel(history, gesture);
        history.delete();
    } else if gesture.is_none()
        && let Some(index) = history.selected
        && let Some((dx, dy)) = nudge(&key, if event.shift_key() { 10 } else { 1 })
    {
        let before = history.shapes.clone();
        history.shapes[index] =
            history.shapes[index].translated(dx, dy, dimensions.0 as i32, dimensions.1 as i32);
        history.commit(before);
    } else {
        return false;
    }
    true
}

fn nudge(key: &str, step: i32) -> Option<(i32, i32)> {
    match key {
        "ArrowLeft" => Some((-step, 0)),
        "ArrowRight" => Some((step, 0)),
        "ArrowUp" => Some((0, -step)),
        "ArrowDown" => Some((0, step)),
        _ => None,
    }
}

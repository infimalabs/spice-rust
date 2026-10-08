//! Keyboard commands on the focused annotation stage.

use super::*;

pub(super) fn handle(
    event: &KeyboardEvent,
    session: &Session,
    dimensions: (u32, u32),
    idle: bool,
    commands: &KeyboardCommands,
) {
    if session.label.borrow().is_some() || event.is_composing() {
        return;
    }
    let key = event.key();
    // One physical Escape or Enter performs one layer of work. Key repeat must
    // not clear a selection and then immediately cancel the whole editor, or
    // start more than one export.
    if event.repeat() && matches!(key.as_str(), "Escape" | "Enter") {
        return;
    }
    let modified = event.meta_key() || event.ctrl_key();
    let mut handled = false;
    if idle {
        let mut gesture = session.gesture.borrow_mut();
        if gesture.is_some() {
            *session.gesture_click.borrow_mut() = true;
        }
        handled = command(
            event,
            &mut session.history.borrow_mut(),
            &mut gesture,
            dimensions,
        );
    }
    if !handled && idle && !modified && key == "Enter" {
        handled = label::reopen(session);
    }
    if !handled && let Some(shortcut) = shortcut(&key, modified, event.alt_key()) {
        handled = true;
        match shortcut {
            Shortcut::Tool(tool) if idle => commands.tool.emit(tool),
            Shortcut::Route if idle && commands.route_enabled => commands.route.emit(()),
            Shortcut::Color(color) if idle => commands.color.emit(color),
            Shortcut::Apply if commands.apply_enabled => commands.apply.emit(()),
            Shortcut::Cancel => commands.cancel.emit(()),
            Shortcut::Tool(_) | Shortcut::Route | Shortcut::Color(_) | Shortcut::Apply => {}
        }
    }
    if handled {
        event.prevent_default();
        event.stop_propagation();
        session.repaint.emit(());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    Tool(Tool),
    Route,
    Color(Color),
    Apply,
    Cancel,
}

/// Printable keys mirror common image-editor tools; digits follow the visible
/// swatch order. Command/Control is reserved for document commands.
fn shortcut(key: &str, modified: bool, alt: bool) -> Option<Shortcut> {
    if alt {
        return None;
    }
    if modified {
        return key.eq_ignore_ascii_case("Enter").then_some(Shortcut::Apply);
    }
    if key == "Escape" {
        return Some(Shortcut::Cancel);
    }
    Some(match key.to_ascii_lowercase().as_str() {
        "v" => Shortcut::Tool(Tool::Select),
        "r" => Shortcut::Tool(Tool::Rectangle),
        "l" => Shortcut::Tool(Tool::Line),
        "a" => Shortcut::Tool(Tool::Arrow),
        "t" => Shortcut::Tool(Tool::Text),
        "g" => Shortcut::Route,
        "1" => Shortcut::Color(Color::Red),
        "2" => Shortcut::Color(Color::Yellow),
        "3" => Shortcut::Color(Color::Green),
        "4" => Shortcut::Color(Color::Blue),
        "5" => Shortcut::Color(Color::Black),
        "6" => Shortcut::Color(Color::White),
        _ => return None,
    })
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

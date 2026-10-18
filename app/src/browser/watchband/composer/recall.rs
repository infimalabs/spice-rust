use super::*;

/// Recall the lane's history for an unmodified vertical arrow whose caret
/// would leave the text, answering whether it replaced the text. A recall
/// upward puts the caret at the start and downward at the end, so repeated
/// presses keep walking while multiline text stays navigable within.
pub(super) fn recall_arrow(
    event: &KeyboardEvent,
    textarea: &web_sys::HtmlTextAreaElement,
    lane: &str,
    recall: &RefCell<crate::composer_history::Recall>,
) -> bool {
    let up = match event.key().as_str() {
        "ArrowUp" => true,
        "ArrowDown" => false,
        _ => return false,
    };
    if event.is_composing()
        || event.shift_key()
        || event.alt_key()
        || event.ctrl_key()
        || event.meta_key()
    {
        return false;
    }
    let (Ok(Some(start)), Ok(Some(end))) = (textarea.selection_start(), textarea.selection_end())
    else {
        return false;
    };
    let value = textarea.value();
    if !crate::composer_history::leaves_text(&value, start, end, up) {
        return false;
    }
    let mut recall = recall.borrow_mut();
    recall.sync(crate::browser::input_history::sends(lane));
    let shown = if up {
        recall.older(|| crate::browser::input_history::entries(lane), &value)
    } else {
        recall.newer(&value)
    };
    let Some(shown) = shown else {
        return false;
    };
    event.prevent_default();
    textarea.set_value(&shown);
    let caret = if up {
        0
    } else {
        u32::try_from(shown.encode_utf16().count()).unwrap_or(u32::MAX)
    };
    let _ = textarea.set_selection_range(caret, caret);
    announce_value(textarea);
    true
}

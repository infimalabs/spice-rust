use super::*;

/// Recall the lane's history for an unmodified vertical arrow whose caret
/// cannot move further that way, answering whether it replaced the text. The
/// caret must sit on the first rendered line for up or the last for down, so
/// soft-wrapped and multiline text stays navigable within. A recall upward
/// puts the caret at the start and downward at the end, so repeated presses
/// keep walking.
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
    if !crate::composer_history::leaves_text(&value, start, end, up)
        || !on_outer_rendered_line(textarea, &value, start, up).unwrap_or(true)
    {
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

/// Computed properties that decide where the textarea wraps its text.
const WRAPPING: [&str; 23] = [
    "direction",
    "font-family",
    "font-feature-settings",
    "font-kerning",
    "font-size",
    "font-stretch",
    "font-style",
    "font-variant",
    "font-weight",
    "letter-spacing",
    "line-height",
    "overflow-wrap",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "tab-size",
    "text-align",
    "text-indent",
    "text-transform",
    "white-space",
    "word-break",
    "word-spacing",
];

/// Whether the caret at UTF-16 `offset` sits on the first rendered line of
/// `textarea` for `up`, or its last for down, or `None` when the browser
/// cannot measure. A hidden mirror wraps the same text at the same width
/// with empty markers at the start, the caret, and the end; an empty element
/// adds no break opportunity, so markers sharing a top share a line.
fn on_outer_rendered_line(
    textarea: &web_sys::HtmlTextAreaElement,
    value: &str,
    offset: u32,
    up: bool,
) -> Option<bool> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let computed = window.get_computed_style(textarea).ok()??;
    let mirror: web_sys::HtmlElement = document.create_element("div").ok()?.dyn_into().ok()?;
    let style = mirror.style();
    for property in WRAPPING {
        let value = computed.get_property_value(property).ok()?;
        style.set_property(property, &value).ok()?;
    }
    let width = format!("{}px", textarea.client_width());
    for (property, value) in [
        ("position", "fixed"),
        ("top", "0"),
        ("left", "0"),
        ("visibility", "hidden"),
        ("pointer-events", "none"),
        ("box-sizing", "border-box"),
        ("border", "0"),
        ("margin", "0"),
        ("height", "auto"),
        ("width", width.as_str()),
    ] {
        style.set_property(property, value).ok()?;
    }
    let marker = || -> Option<web_sys::HtmlElement> {
        document.create_element("span").ok()?.dyn_into().ok()
    };
    let (start, caret, end) = (marker()?, marker()?, marker()?);
    let (before, after) = crate::composer_history::split_utf16(value, offset);
    mirror.append_with_node_1(&start).ok()?;
    mirror.append_with_str_1(before).ok()?;
    mirror.append_with_node_1(&caret).ok()?;
    mirror.append_with_str_1(after).ok()?;
    mirror.append_with_node_1(&end).ok()?;
    document.body()?.append_with_node_1(&mirror).ok()?;
    let edge = if up { &start } else { &end };
    let outer = caret.offset_top() == edge.offset_top();
    mirror.remove();
    Some(outer)
}

//! Carry table column alignment as an attribute the stylesheet reads.
//!
//! pulldown-cmark writes an aligned cell as `style="text-align: …"`, and the
//! Canvas `style-src 'self'` policy refuses every inline style attribute, so
//! the alignment is written here as `data-align` instead and the writer never
//! sees one.

use pulldown_cmark::{Alignment, CowStr, Event, Tag, TagEnd};

pub(super) fn align_by_attribute<'a>(
    events: impl IntoIterator<Item = Event<'a>>,
) -> impl Iterator<Item = Event<'a>> {
    let mut alignments = Vec::new();
    let mut head = false;
    let mut column = 0usize;
    let mut open = None::<&'static str>;
    events.into_iter().map(move |event| match event {
        Event::Start(Tag::Table(table)) => {
            alignments = table;
            Event::Start(Tag::Table(Vec::new()))
        }
        Event::Start(Tag::TableHead) => {
            head = true;
            column = 0;
            event
        }
        Event::End(TagEnd::TableHead) => {
            head = false;
            event
        }
        Event::Start(Tag::TableRow) => {
            column = 0;
            event
        }
        Event::Start(Tag::TableCell) => {
            let align = match alignments.get(column) {
                Some(Alignment::Left) => "left",
                Some(Alignment::Center) => "center",
                Some(Alignment::Right) => "right",
                Some(Alignment::None) | None => return event,
            };
            let cell = if head { "th" } else { "td" };
            open = Some(cell);
            Event::Html(CowStr::from(format!("<{cell} data-align=\"{align}\">")))
        }
        Event::End(TagEnd::TableCell) => {
            column += 1;
            match open.take() {
                Some(cell) => Event::Html(CowStr::from(format!("</{cell}>"))),
                None => event,
            }
        }
        event => event,
    })
}

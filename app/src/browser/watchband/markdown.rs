//! Safe Markdown rendering for watchband cards and composer previews.

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};
use yew::{AttrValue, Html};

pub(super) fn render(markdown: &str) -> Html {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options).map(sanitize_event);
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events);
    Html::from_html_unchecked(AttrValue::from(output))
}

fn sanitize_event(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Start(tag) => Event::Start(sanitize_tag(tag)),
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        other => other,
    }
}

fn sanitize_tag(tag: Tag<'_>) -> Tag<'_> {
    match tag {
        Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Link {
            link_type,
            dest_url: sanitize_url(dest_url, false),
            title,
            id,
        },
        Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Image {
            link_type,
            dest_url: sanitize_url(dest_url, true),
            title,
            id,
        },
        other => other,
    }
}

fn sanitize_url(url: CowStr<'_>, image: bool) -> CowStr<'_> {
    let candidate = url.trim().to_ascii_lowercase();
    let safe_scheme = candidate.starts_with("https://")
        || candidate.starts_with("http://")
        || candidate.starts_with("mailto:")
        || (image && candidate.starts_with("data:image/"));
    let relative = !candidate.starts_with("//")
        && !candidate
            .split(['/', '#', '?'])
            .next()
            .is_some_and(|head| head.contains(':'));
    if safe_scheme || relative {
        url
    } else {
        CowStr::Borrowed("")
    }
}

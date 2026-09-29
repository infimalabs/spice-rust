//! Present a prose card's terminal colon as a period.
//!
//! Canvas stacks cards newest-first, so whatever a trailing colon introduces
//! lands above it and the colon points at nothing. Only the final text of a
//! card that ends in prose changes: a card ending in code, a quote, a table,
//! a link, an image, math, or markup keeps its content verbatim, and every
//! colon before the end stays as written.

use pulldown_cmark::{CowStr, Event, TagEnd};

pub(super) fn period(events: &mut [Event<'_>]) {
    for event in events.iter_mut().rev() {
        match event {
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::List(_)
                | TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough,
            ) => {}
            Event::Text(text) => {
                let body = text.trim_end();
                if let Some(stem) = body.strip_suffix(':') {
                    let presented = format!("{stem}.{}", &text[body.len()..]);
                    *text = CowStr::from(presented);
                }
                return;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::{render_prose_card, render_received};

    #[test]
    fn a_prose_card_presents_only_its_terminal_prose_colon_as_a_period() {
        let card = |markdown: &str| {
            render_prose_card(markdown, &[], &[], &[], &BTreeMap::new(), &BTreeMap::new()).html
        };
        let cases = [
            (
                "Reading the show call path:",
                "<p>Reading the show call path.</p>\n",
            ),
            ("Reading the path:  \n", "<p>Reading the path.</p>\n"),
            (
                "Note: see https://example.test:8767 at 10:30 first:",
                "<p>Note: see https://example.test:8767 at 10:30 first.</p>\n",
            ),
            ("**Next:**", "<p><strong>Next.</strong></p>\n"),
            (
                "Steps:\n\n- read\n- check:",
                "<p>Steps:</p>\n<ul>\n<li>read</li>\n<li>check.</li>\n</ul>\n",
            ),
            (
                "Run `spice task next:`",
                "<p>Run <code>spice task next:</code></p>\n",
            ),
            (
                "Run:\n\n```\nkey:\n```",
                "<p>Run:</p>\n<pre><code>key:\n</code></pre>\n",
            ),
            (
                "Quoted:\n\n> wait:",
                "<p>Quoted:</p>\n<blockquote>\n<p>wait:</p>\n</blockquote>\n",
            ),
            (
                "Colons: inside, then a stop.",
                "<p>Colons: inside, then a stop.</p>\n",
            ),
        ];
        for (markdown, expected) in cases {
            assert_eq!(card(markdown), expected, "{markdown:?}");
        }
        assert_eq!(
            render_received("Nested quote bodies stay verbatim:").html,
            "<p>Nested quote bodies stay verbatim:</p>\n"
        );
    }
}

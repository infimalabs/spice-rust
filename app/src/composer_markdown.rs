//! Target-neutral Markdown composition for the browser lane composer.
//!
//! The module owning the forms is wasm-only, so everything it can compose
//! without a DOM lives here and is reachable from an ordinary native test.
//! Nothing here may be duplicated there: a second copy is what let the shipped
//! quote drift from the tested one until the copies disagreed on blank input.

/// One quoted message and the reply the operator typed beneath it.
pub(crate) struct QuotedReply<'a> {
    /// Quoted text, prefixed line by line as one Markdown block quote.
    pub(crate) quote: &'a str,
    /// Reply body shown under the quote, blank when the operator wrote none.
    pub(crate) body: &'a str,
}

/// One image the operator attached, carried inline as a Markdown image.
pub(crate) struct Attachment<'a> {
    /// Display name, which becomes the alt text.
    pub(crate) name: &'a str,
    /// Data URL carrying the image bytes.
    pub(crate) data_url: &'a str,
}

/// Join every visible quote shard, the primary body, and every attachment.
///
/// The quote shards precede the primary composer in the visible vertical stack,
/// so their document parts precede the primary body here as well. Attachments
/// belong to the assembled document and follow its text. Parts are separated by
/// the blank line Markdown reads as a block boundary, and a part that would
/// contribute nothing is dropped rather than emitted as an empty block.
pub(crate) fn compose(
    body: &str,
    quotes: &[QuotedReply<'_>],
    attachments: &[Attachment<'_>],
) -> String {
    let mut parts = Vec::new();
    parts.extend(quotes.iter().filter_map(|quoted| {
        let quote = block_quote(quoted.quote);
        let body = quoted.body.trim();
        let part = [quote.as_str(), body]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!part.is_empty()).then_some(part)
    }));
    let body = body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    parts.extend(attachments.iter().map(|attachment| {
        let name = attachment
            .name
            .replace(['\r', '\n'], " ")
            .replace(']', "\\]");
        format!("![{name}]({})", attachment.data_url)
    }));
    parts.join("\n\n")
}

/// Normalize line endings, then prefix every line as one Markdown block quote.
///
/// The trim happens to the source and never to the joined result. Trimming the
/// result takes the space off the marker on a final blank line, leaving a bare
/// `>` on the last line, and it collapses whitespace-only text to a lone `>`
/// instead of to nothing, which is a directive the caller then has no reason to
/// suppress. Trimming the source empties that case, so callers can drop it.
pub(crate) fn block_quote(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Display the producer's preview or the explicit absent-preview label.
///
/// This is the module's runtime boundary: agent transcript cards deliberately
/// pass absence instead of reparsing authored prose in the browser.
pub(crate) fn quote_label(preview: Option<&str>) -> &str {
    preview.unwrap_or("quoted message")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted<'a>(values: &'a [(&'a str, &'a str)]) -> Vec<QuotedReply<'a>> {
        values
            .iter()
            .map(|(quote, body)| QuotedReply { quote, body })
            .collect()
    }

    #[test]
    fn composed_markdown_follows_visible_shard_order_and_drops_blank_parts() {
        assert_eq!(compose("  body only  ", &[], &[]), "body only");
        assert_eq!(
            compose(
                "",
                &quoted(&[("first quote", ""), ("  ", ""), ("second quote", "")]),
                &[]
            ),
            "> first quote\n\n> second quote"
        );
        assert_eq!(
            compose("body", &quoted(&[("quoted line", "")]), &[]),
            "> quoted line\n\nbody"
        );
    }

    #[test]
    fn a_reply_follows_the_quote_it_answers_inside_one_part() {
        assert_eq!(
            compose("", &quoted(&[("quoted line", "  my reply  ")]), &[]),
            "> quoted line\n\nmy reply"
        );
    }

    #[test]
    fn attachment_names_lose_the_characters_that_would_break_the_link() {
        assert_eq!(
            compose(
                "",
                &[],
                &[Attachment {
                    name: "one]two\nthree",
                    data_url: "data:image/png;base64,AA"
                }]
            ),
            "![one\\]two three](data:image/png;base64,AA)"
        );
    }

    #[test]
    fn block_quotes_normalize_every_line_ending_before_prefixing() {
        assert_eq!(block_quote("first\r\nsecond"), "> first\n> second");
        assert_eq!(block_quote("first\rsecond"), "> first\n> second");
        assert_eq!(block_quote("first\n\nthird"), "> first\n> \n> third");
    }

    /// Quoted message text routinely ends on a blank line, and the blank line is
    /// the one the marker can lose. The copy that trimmed the joined result left
    /// `>` alone on the last line: a bare marker, not the `> ` every line of a
    /// block quote carries. Asserting the whole string and the per-line marker
    /// together states both the exact output and the invariant behind it.
    #[test]
    fn a_quote_that_ends_on_a_blank_line_keeps_the_marker_on_every_line() {
        let quote = block_quote("first\n\n");

        assert_eq!(quote, "> first");
        assert!(
            quote.lines().all(|line| line.starts_with("> ")),
            "every line of a block quote carries the marker, got {quote:?}"
        );
    }

    /// The composer drops a quote that would contribute nothing, and it can only
    /// drop what arrives empty. The copy that trimmed the joined result returned
    /// a lone `>` here, which is not empty, so the drag handler sent a directive
    /// whose entire body was a marker.
    #[test]
    fn whitespace_only_quoted_text_composes_to_nothing_at_all() {
        assert_eq!(block_quote("   \n\t\n  "), "");
        assert_eq!(block_quote(""), "");
        assert_eq!(compose("", &quoted(&[("   \n  ", "")]), &[]), "");
    }

    #[test]
    fn absent_preview_has_one_documented_label() {
        assert_eq!(quote_label(Some("author words")), "author words");
        assert_eq!(quote_label(None), "quoted message");
    }
}

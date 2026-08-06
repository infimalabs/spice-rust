//! Target-neutral Markdown composition for the browser lane composer.

/// One block the browser renders from the composer preview.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// Ordinary authored prose.
    Paragraph(String),
    /// Quoted message text with the Markdown prefix removed.
    Quote(String),
}

/// Join one body and its nonblank quotes in the order the composer displays.
#[cfg(test)]
pub(crate) fn compose(body: &str, quotes: &[String]) -> String {
    let mut parts = Vec::new();
    let body = body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    parts.extend(
        quotes
            .iter()
            .map(|quote| block_quote(quote))
            .filter(|quote| !quote.is_empty()),
    );
    parts.join("\n\n")
}

/// Normalize line endings, then prefix every line as one Markdown block quote.
///
/// The trim happens to the source and never to the joined result. Trimming the
/// result takes the space off the marker on a final blank line, leaving `>`
/// alone, and [`render`] reads a line that is not `> ` as prose: the whole
/// quote reaches the reader as literal markers instead of a blockquote.
#[cfg(test)]
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

/// Parse the exact blank-line boundary [`compose`] emits into render blocks.
#[cfg(test)]
pub(crate) fn render(markdown: &str) -> Vec<Block> {
    markdown
        .split("\n\n")
        .map(|block| {
            if block.lines().all(|line| line.starts_with("> ")) {
                Block::Quote(
                    block
                        .lines()
                        .map(|line| &line[2..])
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            } else {
                Block::Paragraph(block.to_owned())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotes(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn composed_markdown_keeps_the_body_first_and_drops_blank_parts() {
        assert_eq!(compose("  body only  ", &[]), "body only");
        assert_eq!(
            compose("", &quotes(&["first quote", "  ", "second quote"])),
            "> first quote\n\n> second quote"
        );
        assert_eq!(
            compose("body", &quotes(&["quoted line"])),
            "body\n\n> quoted line"
        );
    }

    #[test]
    fn block_quotes_normalize_every_line_ending_before_prefixing() {
        assert_eq!(block_quote("first\r\nsecond"), "> first\n> second");
        assert_eq!(block_quote("first\rsecond"), "> first\n> second");
        assert_eq!(block_quote("first\n\nthird"), "> first\n> \n> third");
    }

    #[test]
    fn renderer_reads_the_exact_separator_the_composer_writes() {
        let markdown = compose("body", &quotes(&["quoted\nlines"]));

        assert_eq!(
            render(&markdown),
            [
                Block::Paragraph("body".to_owned()),
                Block::Quote("quoted\nlines".to_owned()),
            ]
        );
    }

    /// Quoted message text routinely ends on a blank line, and the blank line is
    /// the one the marker can lose. Trimming the joined result rather than the
    /// source left `>` alone on the last line, which is not the `> ` the
    /// renderer classifies on, so the reader saw the markers as prose. Asserting
    /// the composed text and the rendered block together is what catches it:
    /// either half alone passes while the two disagree.
    #[test]
    fn a_quote_that_ends_on_a_blank_line_still_renders_as_a_quote() {
        let markdown = compose("", &quotes(&["first\n\n"]));

        assert_eq!(markdown, "> first");
        assert_eq!(render(&markdown), [Block::Quote("first".to_owned())]);
    }

    /// The marker is `> ` and not `>`. A body line that opens on a bare angle
    /// bracket is prose the author typed, and classifying it as a quote would
    /// strip a second character the line never spent on a marker.
    #[test]
    fn a_body_line_opening_on_a_bare_angle_bracket_stays_prose() {
        let markdown = compose(">note", &[]);

        assert_eq!(render(&markdown), [Block::Paragraph(">note".to_owned())]);
    }

    #[test]
    fn absent_preview_has_one_documented_label() {
        assert_eq!(quote_label(Some("author words")), "author words");
        assert_eq!(quote_label(None), "quoted message");
    }
}

//! Target-neutral Markdown composition for the browser lane composer.

/// One block the browser renders from the composer preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    /// Ordinary authored prose.
    Paragraph(String),
    /// Quoted message text with the Markdown prefix removed.
    Quote(String),
}

/// Join one body and its nonblank quotes in the order the composer displays.
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
pub(crate) fn block_quote(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().is_empty() {
        return String::new();
    }
    normalized
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Parse the exact blank-line boundary [`compose`] emits into render blocks.
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
}

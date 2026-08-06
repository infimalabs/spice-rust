//! Target-neutral Markdown quoting for the browser lane composer.
//!
//! The composer itself is wasm-only and no native test can reach it, so the
//! two decisions worth pinning live here instead: how a quoted message becomes
//! a Markdown block quote, and which line of one stands for the whole in a
//! composer band. Composition and rendering used to live here as well and no
//! longer do. The composer carries attachments and per-quote reply bodies that
//! a body-and-quotes join cannot express, and it renders through
//! `pulldown_cmark` into sanitized HTML rather than into a block list, so both
//! were replaced outright rather than kept alongside.

/// Normalize line endings, then prefix every line as one Markdown block quote.
///
/// The trim happens to the source and never to the joined result. Trimming the
/// result takes the space off the marker on a final blank line, leaving a bare
/// `>` behind, and turns a quote of nothing but whitespace into a marker with
/// no quote behind it. The composer carried its own copy that trimmed the
/// result; this is the one that does not, and now the only one.
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

/// The one line that stands for a quoted message in a composer band.
///
/// A message worth quoting routinely opens on material its own author was
/// quoting, and Markdown carries that as `>` prefixed lines. Previewing the
/// first line therefore shows a run of angle brackets belonging to someone
/// else, which is the opposite of the glance the band exists to give. This
/// answers with the author's own first line and with nothing when the message
/// carries no words of its own.
///
/// The marker test here is a bare `>`, one character shorter than the `> ` that
/// [`block_quote`] writes. Nothing here strips a character, so the wider test
/// costs nothing, and `>note` is somebody else's quote to every Markdown
/// reader whether or not the space survived the trip.
pub(crate) fn preview(raw: &str) -> String {
    raw.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('>'))
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_quotes_normalize_every_line_ending_before_prefixing() {
        assert_eq!(block_quote("first\r\nsecond"), "> first\n> second");
        assert_eq!(block_quote("first\rsecond"), "> first\n> second");
        assert_eq!(block_quote("first\n\nthird"), "> first\n> \n> third");
    }

    /// Quoted message text routinely ends on a blank line, and that blank line
    /// is the one the marker can lose. Trimming the joined result rather than
    /// the source strips the trailing marker's space, and turns a quote that is
    /// only whitespace into a lone marker instead of nothing at all. Both
    /// endings are asserted because they fail differently: the first keeps a
    /// bare `>`, the second invents a quote out of blank input.
    #[test]
    fn a_quote_that_ends_on_a_blank_line_keeps_its_last_marker() {
        assert_eq!(block_quote("first\n\n"), "> first");
        assert_eq!(block_quote("   \n  "), "");
    }

    /// A steering payload arrives quoting the whole thread back, so the words
    /// its author actually wrote sit below several marker lines rather than at
    /// the top. The band has to reach past them.
    #[test]
    fn the_preview_is_the_first_line_the_author_wrote_themselves() {
        let quoted =
            "> > older reply\n> \n> earlier reply\n\nwhat I actually said\nand a second line";

        assert_eq!(preview(quoted), "what I actually said");
    }

    /// A message with nothing but quoted material has no words to preview, and
    /// the band says nothing rather than repeating someone else back.
    #[test]
    fn a_message_that_is_only_quoted_text_previews_as_nothing() {
        assert_eq!(preview("> only\n> quoted\n>\n   > indented too"), "");
        assert_eq!(preview("   \n\n  "), "");
        assert_eq!(preview(""), "");
    }

    /// Whatever the band shows, it is never a marker. Asserting the absence
    /// separately from the chosen line is what catches a selector that reached
    /// the right line by counting rather than by reading.
    #[test]
    fn no_quote_marker_reaches_the_preview() {
        for raw in [
            "> > deep\n> shallow\nmine",
            "> > deep\r\n> shallow\r\nmine",
            "> > deep\r> shallow\rmine",
            "\t> tabbed marker\n\tmine",
        ] {
            let shown = preview(raw);
            assert!(!shown.contains('>'), "marker reached the band: {shown:?}");
            assert_eq!(shown, "mine", "from {raw:?}");
        }
    }
}

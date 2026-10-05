//! Text rules shared by the resident's Speak selection and the browser's
//! automatic narration, so both voices cut a message in the same place.

use crate::AutomaticSpeech;

impl AutomaticSpeech {
    /// The body of one trimmed Markdown list-item line, or `None` for prose.
    ///
    /// Automatic Speak and Narrate voice a message's edge paragraphs only up to
    /// its first list: items are detail to read on the card, and a list that
    /// closes a message would otherwise be voiced whole as its last paragraph.
    #[must_use]
    pub fn list_item(line: &str) -> Option<&str> {
        let after_marker = match line.strip_prefix(['-', '*', '+']) {
            Some(rest) => rest,
            None => {
                let digits = line.bytes().take_while(u8::is_ascii_digit).count();
                if !(1..=9).contains(&digits) {
                    return None;
                }
                line[digits..].strip_prefix(['.', ')'])?
            }
        };
        after_marker.strip_prefix([' ', '\t']).map(str::trim_start)
    }
}

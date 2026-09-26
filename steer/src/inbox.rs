//! In-memory rendering and delivery state for pending direction.

use wire::intent::PendingDirective;

use crate::ack::keys::Key;
use crate::directives::Directive;

/// Renders the direction a lane must hear, sealed under the lane's own key.
///
/// The seal wraps the whole rendering so a lane can tell direction spice
/// delivered from text that merely resembles it. Prose arriving any other way
/// is not steering, however much it looks the part, and a rendering with
/// nothing pending is empty rather than an empty seal.
#[must_use]
pub fn render(seal: &Key, pending: &[&Directive]) -> String {
    if pending.is_empty() {
        return String::new();
    }
    let mut rendered = format!("<{seal}>\n");
    for directive in pending {
        rendered.push_str(&format!(
            "directive={} key={} stage={} expires={}",
            directive.directive(),
            directive.key(),
            directive.stage(),
            directive.expires()
        ));
        if let Some(control) = directive.control() {
            rendered.push_str(&format!(" control={control}"));
        }
        rendered.push('\n');
        rendered.push_str(&render_directive_body(directive));
        rendered.push('\n');
    }
    rendered.push_str(&format!("</{seal}>\n"));
    rendered
}
/// Renders a journal-backed inbox answer for one lane.
#[must_use]
pub fn render_pending(seal: &Key, pending: &[PendingDirective]) -> String {
    if pending.is_empty() {
        return String::new();
    }
    let mut rendered = format!("<{seal}>\n");
    for directive in pending {
        rendered.push_str(&format!(
            "directive={} key={} stage={} expires={}",
            directive.directive, directive.key, directive.stage, directive.expires
        ));
        if let Some(control) = &directive.control {
            rendered.push_str(&format!(" control={control}"));
        }
        rendered.push('\n');
        rendered.push_str(&render_body(
            directive.directive,
            &directive.text,
            &directive.attachments,
        ));
        rendered.push('\n');
    }
    rendered.push_str(&format!("</{seal}>\n"));
    rendered
}

/// The keys a rendering carries, in the order it names them.
///
/// A reader that already holds an earlier rendering learns from these which
/// direction is new to the lane and which is the same direction said again.
pub fn keys(rendered: &str) -> impl Iterator<Item = &str> {
    rendered
        .lines()
        .filter(|line| line.starts_with("directive="))
        .flat_map(|line| {
            line.split(' ')
                .filter_map(|field| field.strip_prefix("key="))
        })
}

/// What an occupant still holds of one directive, and what it has said since.
///
/// A resend that only says "still unanswered" cannot tell an occupant that
/// never wrote a visible reply apart from one whose replies named the wrong
/// key or buried the marker, so it misreads its own silence as a stalled
/// scanner. The supervisor observes the occupant's records, so it states
/// which of the two it saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    /// Told, and no visible reply has been observed since.
    Silent,
    /// Told, and visible replies followed without answering this key.
    Replied,
}

impl Told {
    /// The pointer that replaces a telling the occupant still holds.
    #[must_use]
    pub const fn pointer(self) -> &'static str {
        match self {
            Self::Silent => NUDGE_SILENT,
            Self::Replied => NUDGE_REPLIED,
        }
    }
}

/// Pointer for a held directive after which no visible reply was observed.
///
/// Reasoning never reaches the transcript as a reply, which is the usual way
/// an occupant believes it answered when it did not.
pub const NUDGE_SILENT: &str =
    "[NUDGE] unanswered, no visible reply yet (thinking is not one); ACK or NACK this key";

/// Pointer for a held directive after which visible replies did not answer it.
pub const NUDGE_REPLIED: &str =
    "[NUDGE] unanswered; no visible line since led with ACK or NACK this key";

/// Condenses every directive the occupant was already told into a pointer.
///
/// A resend exists to say a directive is still unanswered, not to say the
/// direction again: the words are already verbatim in the occupant's context,
/// so repeating them charges the occupant a second time for text it holds.
/// `told` answers what this occupant still holds of each key: `None` for a
/// context that dropped the telling, by compaction or by never having heard
/// it, which is told in full. The header line is never condensed, because the
/// key, the stage, and any control field are what the resend is for, and a
/// telling shorter than the pointer is simply said again.
#[must_use]
pub fn nudge(rendered: &str, told: impl Fn(&str) -> Option<Told>) -> String {
    let mut condensed = String::with_capacity(rendered.len());
    let mut body = String::new();
    let mut held = None;
    let mut lines = rendered.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if lines.peek().is_none() {
            // The closing seal belongs to the rendering rather than to the
            // directive body it follows.
            push_body(&mut condensed, &body, held);
            condensed.push_str(line);
            break;
        }
        if let Some(key) = header_key(line) {
            push_body(&mut condensed, &body, held);
            body.clear();
            held = told(key);
            condensed.push_str(line);
        } else {
            body.push_str(line);
        }
    }
    condensed
}

/// The key a directive header names, for a line that is one.
fn header_key(line: &str) -> Option<&str> {
    line.strip_prefix("directive=")
        .and_then(|fields| {
            fields
                .split(' ')
                .find_map(|field| field.strip_prefix("key="))
        })
        .map(str::trim_end)
}

/// Spend the body, or the pointer when the occupant holds the body already
/// and the pointer is the cheaper of the two.
fn push_body(condensed: &mut String, body: &str, held: Option<Told>) {
    // What the pointer itself costs, its own line ending included.
    match held.map(Told::pointer) {
        Some(pointer) if body.len() > pointer.len() + "\n".len() => {
            condensed.push_str(pointer);
            condensed.push('\n');
        }
        _ => condensed.push_str(body),
    }
}

fn render_directive_body(directive: &Directive) -> String {
    render_body(
        directive.directive(),
        directive.text(),
        directive.attachments(),
    )
}

fn render_body(
    directive: wire::CanonicalUuid,
    text: &str,
    attachments: &[wire::body::DirectiveAttachment],
) -> String {
    let mut rendered = wire::media::without_media_payloads(text, attachments);
    for reference in wire::media::agent_media_references(directive, attachments)
        .expect("an inbox answer contains only admitted media")
    {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str("media-reference=");
        rendered.push_str(
            &String::from_utf8(wire::encode(&reference).expect("media reference encodes"))
                .expect("JSON media reference is UTF-8"),
        );
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{NUDGE_REPLIED, NUDGE_SILENT, Told, keys, nudge};

    #[test]
    fn a_rendering_names_its_keys_in_order_and_nothing_else() {
        let rendered = "<seal>\ndirective=d1 key=1kJK6N3R stage=heard expires=never\n\
                        say key=nothing here\n\ndirective=d2 key=1kJK6M0V stage=new \
                        expires=never control=stop\ngo\n</seal>\n";
        assert_eq!(keys(rendered).collect::<Vec<_>>(), ["1kJK6N3R", "1kJK6M0V"]);
        assert_eq!(keys("").count(), 0);
    }

    /// One directive from a real delivery, shaped and sized as the lane
    /// received it: the operator's broadcast dSvXD9bv3 cost 2702 bytes the
    /// first time and 2701 the second, the whole text over again.
    fn delivered(stage: &str) -> String {
        // Sentences enough to size the fixture to that real delivery.
        const SENTENCES: usize = 66;

        let body =
            "[BROADCAST] ".to_owned() + &"measure the cost before changing it. ".repeat(SENTENCES);
        format!(
            "<Qk8zdj5rQ>\ndirective=1bea6ea6-a894-4899-923e-16df120968b3 key=dSvXD9bv3 \
             stage={stage} expires=2026-09-20T02:25:15.856Z\n{body}\n</Qk8zdj5rQ>\n",
        )
    }

    #[test]
    fn a_resend_costs_a_pointer_and_still_names_what_to_answer() {
        let first = delivered("published");
        let again = delivered("reminded");

        // What a pointer is allowed to cost against the telling it replaces.
        const SHARE: usize = 10;

        for (since, pointer) in [(Told::Silent, NUDGE_SILENT), (Told::Replied, NUDGE_REPLIED)] {
            let resent = nudge(&again, |key| (key == "dSvXD9bv3").then_some(since));
            assert!(
                resent.len() * SHARE < first.len(),
                "a resend of {} bytes against a telling of {} bytes is not a pointer",
                resent.len(),
                first.len(),
            );
            assert_eq!(keys(&resent).collect::<Vec<_>>(), ["dSvXD9bv3"]);
            assert!(
                resent.contains("stage=reminded") && resent.contains("key=dSvXD9bv3"),
                "the pointer must still name the directive and why it arrived: {resent}",
            );
            assert!(
                resent.contains(pointer),
                "the pointer must say what it saw: {resent}"
            );
            assert!(
                !resent.contains("measure the cost"),
                "the telling is already in the occupant's context: {resent}",
            );
            assert!(
                resent.starts_with("<Qk8zdj5rQ>\n") && resent.ends_with("</Qk8zdj5rQ>\n"),
                "the seal authenticates the pointer exactly as it authenticates a telling: {resent}",
            );
        }
    }

    #[test]
    fn a_silent_occupant_is_told_its_reasoning_was_not_a_reply() {
        let again = delivered("reminded");
        let silent = nudge(&again, |_| Some(Told::Silent));
        let replied = nudge(&again, |_| Some(Told::Replied));

        assert_ne!(
            silent, replied,
            "the two observations must read differently"
        );
        assert!(
            silent.contains("no visible reply") && silent.contains("thinking"),
            "an occupant that only thought its answer must learn that: {silent}",
        );
        assert!(
            !replied.contains("no visible reply"),
            "an occupant that replied must not be told it was silent: {replied}",
        );
    }

    #[test]
    fn a_telling_this_context_never_heard_is_spent_in_full() {
        let held = "hold the line while the suite runs, and say so when it finishes running \
                    along with what it reported";
        let rendering = format!(
            "<seal>\ndirective=d1 key=1kJK6N3R stage=reminded expires=never\n{held}\n{}",
            &delivered("published")["<Qk8zdj5rQ>\n".len()..],
        );
        let resent = nudge(&rendering, |key| {
            (key == "1kJK6N3R").then_some(Told::Replied)
        });

        assert!(
            !resent.contains(held),
            "the key this context holds is pointed at, not repeated: {resent}",
        );
        assert!(
            resent.contains("measure the cost"),
            "a key this context never heard is told in full: {resent}",
        );
        assert_eq!(
            keys(&resent).collect::<Vec<_>>(),
            ["1kJK6N3R", "dSvXD9bv3"],
            "condensing one directive does not drop another",
        );
    }
}

//! Atomically rendered pending direction.
//!
//! A lane hears direction from a file, not from a database. The wrapper sitting
//! in front of every command must not open authority to find out whether
//! anything is waiting, so what a lane owes is rendered ahead of time and left
//! where a hook can read it.
//!
//! That file is replaced whole through the publication primitive, which is
//! precisely why atomic sits in this crate's row: a hook reading mid-render
//! sees the previous rendering rather than half of the next one. The rendering
//! is disposable — it may be deleted or stale, and disposition still lives in
//! the journal.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use wire::CanonicalUuid;

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

/// What a directive costs the second time, once its telling is already in the
/// occupant's context.
///
/// The header still carries the key and the stage, so the pointer names the
/// direction and what answering it looks like without spending the text again.
pub const NUDGE: &str = "[NUDGE] told in full above and still unanswered; ACK or NACK this key";

/// Condenses every directive the occupant was already told into a pointer.
///
/// A resend exists to say a directive is still unanswered, not to say the
/// direction again: the words are already verbatim in the occupant's context,
/// so repeating them charges the occupant a second time for text it holds.
/// `told` answers whether this occupant still holds the telling — a context
/// that dropped it, by compaction or by never having heard it, is told in
/// full. The header line is never condensed, because the key, the stage, and
/// any control field are what the resend is for, and a telling shorter than
/// the pointer is simply said again.
#[must_use]
pub fn nudge(rendered: &str, told: impl Fn(&str) -> bool) -> String {
    let mut condensed = String::with_capacity(rendered.len());
    let mut body = String::new();
    let mut held = false;
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
fn push_body(condensed: &mut String, body: &str, held: bool) {
    // What the pointer itself costs, its own line ending included.
    let pointer = NUDGE.len() + "\n".len();
    if held && body.len() > pointer {
        condensed.push_str(NUDGE);
        condensed.push('\n');
    } else {
        condensed.push_str(body);
    }
}

fn render_directive_body(directive: &Directive) -> String {
    let mut rendered =
        wire::media::without_media_payloads(directive.text(), directive.attachments());
    for reference in directive
        .media_references()
        .expect("a directive ledger contains only admitted media")
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

/// Where a lane's rendering sits under the inbox directory.
#[must_use]
pub fn path(inbox: &Path, lane: CanonicalUuid) -> PathBuf {
    inbox.join(lane.to_string())
}

/// Publishes a lane's rendering, replacing whatever was there whole.
///
/// The inbox directory is created on the way, because a lane's first pending
/// directive is exactly when the directory begins to exist.
///
/// # Errors
///
/// Returns [`Error::Directory`] when the inbox directory cannot be created and
/// [`Error::Publication`] when the rendering cannot be placed.
pub fn publish(
    inbox: &Path,
    lane: CanonicalUuid,
    seal: &Key,
    pending: &[&Directive],
) -> Result<(), Error> {
    fs::create_dir_all(inbox).map_err(Error::Directory)?;
    let rendered = render(seal, pending);
    let destination = atomic::Destination::new(path(inbox, lane))?;
    destination
        .publish(|staged| staged.write_all(rendered.as_bytes()))
        .map_err(Error::Publication)
}

/// Why a rendering could not be published.
#[derive(Debug)]
pub enum Error {
    /// The inbox directory could not be created.
    Directory(std::io::Error),
    /// The rendering could not replace its destination.
    Publication(atomic::Error),
}

impl From<atomic::Error> for Error {
    fn from(error: atomic::Error) -> Self {
        Self::Publication(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Directory(error) => write!(formatter, "inbox directory: {error}"),
            Self::Publication(error) => write!(formatter, "inbox rendering: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory(error) => Some(error),
            Self::Publication(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NUDGE, keys, nudge};

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
        let resent = nudge(&again, |key| key == "dSvXD9bv3");

        // What a pointer is allowed to cost against the telling it replaces.
        const SHARE: usize = 10;

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
        assert!(resent.contains(NUDGE), "the pointer must say what it wants");
        assert!(
            !resent.contains("measure the cost"),
            "the telling is already in the occupant's context: {resent}",
        );
        assert!(
            resent.starts_with("<Qk8zdj5rQ>\n") && resent.ends_with("</Qk8zdj5rQ>\n"),
            "the seal authenticates the pointer exactly as it authenticates a telling: {resent}",
        );
    }

    #[test]
    fn a_telling_this_context_never_heard_is_spent_in_full() {
        let held = "hold the line while the suite runs, and say so when it finishes running";
        let rendering = format!(
            "<seal>\ndirective=d1 key=1kJK6N3R stage=reminded expires=never\n{held}\n{}",
            &delivered("published")["<Qk8zdj5rQ>\n".len()..],
        );
        let resent = nudge(&rendering, |key| key == "1kJK6N3R");

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

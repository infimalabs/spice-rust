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
    use super::keys;

    #[test]
    fn a_rendering_names_its_keys_in_order_and_nothing_else() {
        let rendered = "<seal>\ndirective=d1 key=1kJK6N3R stage=heard expires=never\n\
                        say key=nothing here\n\ndirective=d2 key=1kJK6M0V stage=new \
                        expires=never control=stop\ngo\n</seal>\n";
        assert_eq!(keys(rendered).collect::<Vec<_>>(), ["1kJK6N3R", "1kJK6M0V"]);
        assert_eq!(keys("").count(), 0);
    }
}

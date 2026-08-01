use std::fmt::Write;

use crate::graph::{View, render};
use crate::task::Entry;

/// Compose every named diagram into one stable shareable document.
#[must_use]
pub fn compose(entries: &[Entry]) -> String {
    let mut handout = String::from("# Board handout\n\n");
    for view in View::ALL {
        let diagram = render(view, entries);
        writeln!(handout, "## {}\n", view.as_str()).expect("String writes are infallible");
        handout.push_str("```text\n");
        handout.push_str(diagram.text());
        handout.push_str("```\n\n");
    }
    handout
}

//! Binary-rim translation for daemonless release-note derivation.

use std::fs;
use std::path::Path;

use wire::{Fault, FaultClass, Outcome, Refusal, ReleaseNotes, Reply, ReplyPosition};

pub(crate) fn notes(matches: &clap::ArgMatches) -> Outcome<ReleaseNotes> {
    let input = matches
        .get_one::<String>("INPUT")
        .expect("release notes requires its generated INPUT operand");
    let bytes = match fs::read(Path::new(input)) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "release::input-unreadable".to_owned(),
                detail: format!("cannot read release input {input:?}: {error}"),
            });
        }
    };
    match dist::release_notes(&bytes) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn failure(error: dist::NotesError) -> Outcome<ReleaseNotes> {
    Outcome::Refusal(Refusal {
        reason: "release::notes-refused".to_owned(),
        witness: Some("release-input".to_owned()),
        expected: Some("one canonical current ordered release evidence document".to_owned()),
        observed: Some(error.to_string()),
        remedy: "supply both target-matched proof and gate receipts for one source".to_owned(),
    })
}

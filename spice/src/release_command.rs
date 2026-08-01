//! Binary-rim translation for daemonless release-note derivation.

use std::path::Path;

use wire::{Fault, FaultClass, Outcome, Refusal, ReleaseNotes, Reply, ReplyPosition};

pub(crate) fn notes(matches: &clap::ArgMatches) -> Outcome<ReleaseNotes> {
    let range = matches
        .get_one::<String>("RANGE")
        .expect("release notes requires its generated RANGE operand");
    let manifest = matches
        .get_one::<String>("MANIFEST")
        .expect("release notes requires its generated MANIFEST operand");
    let repository = match std::env::current_dir() {
        Ok(repository) => repository,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "release::working-directory-unavailable".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let battery = proof::Battery::current();
    match dist::release_notes(&repository, range, Path::new(manifest), battery.digest()) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn failure(error: dist::NotesError) -> Outcome<ReleaseNotes> {
    use dist::NotesError;

    match error {
        refusal @ (NotesError::ManifestMissing(_)
        | NotesError::ManifestInvalid(_)
        | NotesError::Range(_)
        | NotesError::Schema(_)
        | NotesError::SourceMismatch { .. }
        | NotesError::Authored(_)) => Outcome::Refusal(Refusal {
            reason: "release::notes-refused".to_owned(),
            witness: Some("release-evidence".to_owned()),
            expected: Some(
                "one immutable first-parent range and its canonical sealed manifest".to_owned(),
            ),
            observed: Some(refusal.to_string()),
            remedy: "seal consistent evidence and resolve every authored placeholder".to_owned(),
        }),
        unavailable @ NotesError::Repository { .. } => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "release::repository-unavailable".to_owned(),
            detail: unavailable.to_string(),
        }),
        unreadable @ NotesError::ManifestIo { .. } => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "release::manifest-unreadable".to_owned(),
            detail: unreadable.to_string(),
        }),
        invalid @ NotesError::GitObject { .. } => Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "release::git-object-invalid".to_owned(),
            detail: invalid.to_string(),
        }),
    }
}

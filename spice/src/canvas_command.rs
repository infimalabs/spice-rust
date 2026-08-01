//! Binary-rim translation for opening the resident's browser surface.

use wire::{CanvasOpen, Fault, FaultClass, Outcome};

/// Open the exact loopback origin in the current repository's discovery.
pub(crate) fn open() -> Outcome<CanvasOpen> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            return environment("canvas::working-directory-unavailable", &error.to_string());
        }
    };
    let repository = match spice::repository::Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => return environment("canvas::git-unavailable", &error.to_string()),
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    crate::command_client::open_canvas(state.canvas())
}

fn environment(reason: &str, detail: &str) -> Outcome<CanvasOpen> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

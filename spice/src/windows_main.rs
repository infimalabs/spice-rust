//! Native Windows release entry point.
//!
//! The hosted release boundary currently needs only the immutable product and
//! linked-canvas identity surface. Other appliance commands remain explicitly
//! unavailable until their process, lock, and resident contracts have native
//! Windows implementations.

use std::io::Write as _;
use std::process::ExitCode;

use wire::{Fault, FaultClass, Outcome, Reply, ReplyPosition, Version};

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("{}", spice::version::report());
        return ExitCode::SUCCESS;
    }
    if matches!(
        arguments.as_slice(),
        [first, second]
            if (first == "--json" && second == "version")
                || (first == "version" && second == "--json")
    ) {
        return deliver(version_outcome());
    }

    eprintln!(
        "the Windows release executable supports --version and --json version; repository execution uses the hosted-windows runner"
    );
    ExitCode::from(2)
}

fn version_outcome() -> Outcome<Version> {
    let start = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => return fault("version::working-directory-unavailable", error),
    };
    let running = match spice::version::running() {
        Ok(running) => running,
        Err(error) => return Outcome::Fault(error),
    };
    let repository = match spice::repository::Repository::discover(&start) {
        Ok(repository) => repository,
        Err(error) => return fault(spice::repository::ResolveError::REASON, error),
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return fault("repository::state-path-unavailable", error),
    };
    let app = match serve::AppSource::resolve(repository.worktree_root())
        .and_then(|source| source.current_generation())
    {
        Ok(generation) => generation.identity().clone(),
        Err(error) => return fault("version::app-unavailable", error),
    };
    let journal = match journal::JournalReader::open(state.journal())
        .and_then(|reader| reader.schema_version())
    {
        Ok(version) => version,
        Err(error) => return fault("journal::version-unavailable", error),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: Version {
            product: running.product.clone(),
            binary: running.binary.clone(),
            wire: wire::WireVersion::CURRENT,
            journal,
            app_protocol: wire::AppProtocolVersion::CURRENT,
            app,
        },
    })
}

fn fault(reason: &'static str, error: impl std::fmt::Display) -> Outcome<Version> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: error.to_string(),
    })
}

fn deliver(outcome: Outcome<Version>) -> ExitCode {
    let encoded = match wire::encode(&outcome) {
        Ok(encoded) => encoded,
        Err(error) => {
            eprintln!("version::encode-failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let reply = matches!(outcome, Outcome::Reply(_));
    let written = if reply {
        let mut output = std::io::stdout().lock();
        output
            .write_all(&encoded)
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush())
    } else {
        let mut output = std::io::stderr().lock();
        output
            .write_all(&encoded)
            .and_then(|()| output.write_all(b"\n"))
            .and_then(|()| output.flush())
    };
    if written.is_err() {
        return ExitCode::FAILURE;
    }
    if reply {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

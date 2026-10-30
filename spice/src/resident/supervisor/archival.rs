//! Opportunistic storage maintenance at the supervisor's capture handoff.
//!
//! The resident reserves one launch per lane. Its off-loop establishment waits
//! for a terminal predecessor's supervisor to exit before spawning a successor.
//! Shutdown publishes the real completion before maintenance; its successor
//! therefore waits outside both launch horizons, not inside an agent deadline.

use std::io;

use journal::JournalReader;
use tape::Tape;
use wire::{Actor, CanonicalUuid};

use super::{
    Phase, Request, RuntimePaths, StateEvents, process_exists, read_state, supervisor_directory,
};

/// A minimum for opportunistic work, never an active-capture rotation trigger.
const MINIMUM: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Boundary {
    Startup,
    Shutdown,
}

struct Authority {
    previous: Option<CanonicalUuid>,
    eligible: bool,
}

/// Before any successor process or launch clock exists, finish its handoff.
/// An unreadable predecessor cannot authorize overlapping capture.
pub(super) fn startup(paths: &RuntimePaths, request: &Request) -> io::Result<()> {
    let authority = match authority(paths, request, Boundary::Startup) {
        Ok(Some(authority)) => authority,
        Ok(None) => return Ok(()),
        Err(error) => {
            diagnostic(&error);
            return Ok(());
        }
    };
    let Some(previous) = authority.previous else {
        return Ok(());
    };
    let path = supervisor_directory(paths, request.lane, previous).join("state.json");
    let state = match read_state(&path) {
        Ok(state) => state,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if state.lane != request.lane || state.occupant != previous || state.vendor != request.vendor {
        return Ok(());
    }
    if state.phase != Phase::Exited {
        return Ok(());
    }
    if process_exists(state.supervisor).map_err(super::outcome_error)? {
        StateEvents::arm(&path, state.supervisor)?.wait_for_exit()?;
    }
    if !state.capture_drained {
        return Ok(());
    }
    // Waiting may have admitted a claim or changed the lane's ownership.
    match ready(paths, request, Boundary::Startup) {
        Ok(Some(tape)) => compress(&tape),
        Ok(None) => {}
        Err(error) => diagnostic(&error),
    }
    Ok(())
}

/// Called only after local child reaping, complete capture, and driver closure.
pub(super) fn shutdown(paths: &RuntimePaths, request: &Request) -> Option<Tape> {
    match ready(paths, request, Boundary::Shutdown) {
        Ok(tape) => tape,
        Err(error) => {
            diagnostic(&error);
            None
        }
    }
}

fn authority(
    paths: &RuntimePaths,
    request: &Request,
    boundary: Boundary,
) -> io::Result<Option<Authority>> {
    let reader = JournalReader::open(&paths.journal).map_err(io::Error::other)?;
    reader
        .read_with(|read| -> io::Result<Option<Authority>> {
            let Some(head) = read.residency_head().map_err(io::Error::other)? else {
                return Ok(None);
            };
            let head = head.body();
            let Some(row) = head.lanes.iter().find(|row| {
                row.lane == request.lane
                    && row.vendor.as_str() == request.vendor
                    && row.worktree == request.worktree
            }) else {
                return Ok(None);
            };
            let owned = match boundary {
                Boundary::Startup => row.occupant.is_none(),
                Boundary::Shutdown => row.occupant == Some(request.occupant),
            };
            let claimed = read
                .active_claim_for_actor(&Actor::Lane(request.lane))
                .map_err(io::Error::other)?
                .is_some();
            Ok(Some(Authority {
                previous: row.lineage.last().copied(),
                eligible: owned && head.epoch == request.epoch && !claimed,
            }))
        })
        .map_err(io::Error::other)
}

fn ready(paths: &RuntimePaths, request: &Request, boundary: Boundary) -> io::Result<Option<Tape>> {
    if !authority(paths, request, boundary)?.is_some_and(|authority| authority.eligible)
        || !super::terminal::Jobs::for_lane(paths, request.lane).quiescent()?
    {
        return Ok(None);
    }
    let root = paths
        .tape
        .join(request.lane.to_string())
        .join(&request.vendor);
    let tape = match Tape::open_existing(root) {
        Ok(tape) => tape,
        Err(tape::Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(io::Error::other(error)),
    };
    let eligible = tape.plaintext_bytes().map_err(io::Error::other)? >= MINIMUM;
    Ok(eligible.then_some(tape))
}

/// Storage owns verification, publication, recovery, and exclusive writer access.
/// A refusal changes neither the already-published agent result nor its bytes.
pub(super) fn compress(tape: &Tape) {
    match tape.seal() {
        Ok(tape::Seal::Sealed {
            member,
            plaintext,
            compressed,
        }) => {
            eprintln!(
                "spice supervisor: archived tape member {member}: {plaintext} -> {compressed} bytes"
            );
        }
        Ok(tape::Seal::Empty) => {}
        Err(error) => diagnostic(&error),
    }
}

fn diagnostic(error: &impl std::fmt::Display) {
    eprintln!("spice supervisor: tape archival skipped: {error}");
}

#[cfg(test)]
mod tests;

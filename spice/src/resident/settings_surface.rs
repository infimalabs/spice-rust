//! Durable global Canvas settings and their command surface.

use bus::Bus;
use journal::FactCause;
use wire::intent::{CommandAnswer, GlobalSettingsAnswer, GlobalSettingsSet};
use wire::{Command, Outcome, Producer, Reply, ReplyPosition, SeqRange};

use super::{CommandResult, commit, seq, store_fault, typed_outcome, typed_store_fault, write};

/// Resolve global settings from durable authority. Absence is the predecessor default.
pub(super) fn resolve(envelope: Option<&wire::Envelope<wire::AnyFact>>) -> GlobalSettingsAnswer {
    envelope
        .and_then(|envelope| envelope.fact.read::<wire::body::GlobalSettingsChanged>())
        .map_or(GlobalSettingsAnswer { fast_mode: false }, |settings| {
            GlobalSettingsAnswer {
                fast_mode: settings.fast_mode,
            }
        })
}

/// Resolve the current fast-mode value for one later occupant launch.
pub(super) fn fast_mode(bus: &Bus) -> CommandResult<bool> {
    bus.read(|read| read.latest_fact(wire::FactKind::GlobalSettingsChanged))
        .map(|fact| resolve(fact.as_ref()).fast_mode)
        .map_err(|error| store_fault("settings::journal-unreadable", &error.to_string()))
}

/// Read the current global settings at one immutable journal horizon.
pub(super) fn read(bus: &Bus) -> Outcome<CommandAnswer> {
    let (cursor, fact) = match bus.read(|read| {
        Ok((
            read.cursor()?,
            read.latest_fact(wire::FactKind::GlobalSettingsChanged)?,
        ))
    }) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return typed_store_fault("settings::journal-unreadable", &error.to_string());
        }
    };
    let Some(cursor) = cursor else {
        return typed_store_fault(
            "settings::journal-empty",
            "a running resident has no startup authority fact",
        );
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::GlobalSettings(resolve(fact.as_ref())),
    })
}

/// Commit one global fast-mode replacement and its replayable command receipt.
pub(super) fn set(
    bus: &Bus,
    epoch: wire::CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &GlobalSettingsSet,
) -> Outcome<CommandAnswer> {
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(error) = proposal.fact(
        wire::body::GlobalSettingsChanged {
            fast_mode: request.fast_mode,
        },
        FactCause::Root,
    ) {
        return typed_store_fault("settings::fact-invalid", &error.to_string());
    }
    let answer = GlobalSettingsAnswer {
        fast_mode: request.fast_mode,
    };
    if let Err(error) = proposal.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: *request,
        },
        epoch,
        answer,
    ) {
        return typed_store_fault("settings::receipt-invalid", &error.to_string());
    }
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::GlobalSettings(answer),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

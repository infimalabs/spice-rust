//! Durable global app settings and their command surface.
//!
//! Operator identity is repository Git configuration, resolved on every read;
//! settings never carry or change it.

use bus::Bus;
use gix::bstr::ByteSlice as _;
use journal::FactCause;
use sha2::{Digest as _, Sha256};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, GlobalSettingsAnswer, GlobalSettingsSet};
use wire::{Command, OperatorIdentity, Outcome, Producer, Reply, ReplyPosition, SeqRange};

use super::{CommandResult, commit, seq, store_fault, typed_outcome, typed_store_fault, write};

fn stored_fast_mode(envelope: Option<&wire::Envelope<wire::AnyFact>>) -> bool {
    envelope
        .and_then(|envelope| envelope.fact.read::<wire::body::GlobalSettingsChanged>())
        .is_some_and(|settings| settings.fast_mode)
}

fn normalize(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn malformed(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value.chars().any(char::is_control)
    })
}

fn initials(name: Option<&str>) -> Option<String> {
    let badge = name
        .into_iter()
        .flat_map(|name| name.split_whitespace().take(2))
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect::<String>();
    (!badge.is_empty()).then_some(badge)
}

fn identity(name: Option<String>, email: Option<String>) -> OperatorIdentity {
    let gravatar = email.as_ref().map(|email| {
        format!("{:x}", Sha256::digest(email.to_lowercase().as_bytes()))
            .parse()
            .expect("a SHA-256 lowercase hex spelling is a Gravatar hash")
    });
    OperatorIdentity {
        initials: initials(name.as_deref()),
        name,
        email,
        gravatar,
    }
}

fn git_operator(repository: &Repository) -> CommandResult<OperatorIdentity> {
    let git = gix::open(repository.worktree_root()).map_err(|error| {
        super::environment_fault("serve::operator-identity-unavailable", &error.to_string())
    })?;
    let config = git.config_snapshot();
    let read = |key| {
        config
            .string(key)
            .and_then(|value| value.to_str().ok().and_then(|value| normalize(Some(value))))
            .filter(|value| !malformed(Some(value)))
    };
    Ok(identity(read("user.name"), read("user.email")))
}

fn answer(repository: &Repository, fast_mode: bool) -> CommandResult<GlobalSettingsAnswer> {
    Ok(GlobalSettingsAnswer {
        fast_mode,
        operator: git_operator(repository)?,
    })
}

/// Resolve the current settings and effective operator identity for bootstrap.
pub(super) fn current(bus: &Bus, repository: &Repository) -> CommandResult<GlobalSettingsAnswer> {
    let fact = bus
        .read(|read| read.latest_fact(wire::FactKind::GlobalSettingsChanged))
        .map_err(|error| store_fault("settings::journal-unreadable", &error.to_string()))?;
    answer(repository, stored_fast_mode(fact.as_ref()))
}

/// The fast-mode levels a residency head does not hold: the global setting
/// and the lanes fast-flagged working groups run fast.
pub(super) struct FastLevels {
    global: bool,
    groups: std::collections::BTreeSet<wire::CanonicalUuid>,
}

impl FastLevels {
    pub(super) const fn new(
        global: bool,
        groups: std::collections::BTreeSet<wire::CanonicalUuid>,
    ) -> Self {
        Self { global, groups }
    }

    /// Read both levels at one journal horizon.
    pub(super) fn read(bus: &Bus) -> CommandResult<Self> {
        bus.read(|read| {
            Ok(Self::new(
                stored_fast_mode(
                    read.latest_fact(wire::FactKind::GlobalSettingsChanged)?
                        .as_ref(),
                ),
                crate::board_command::group_fast_lanes(read)?,
            ))
        })
        .map_err(|error| store_fault("settings::journal-unreadable", &error.to_string()))
    }

    /// Whether one seat runs fast: its own flag, its team's flag, the global
    /// setting, or a fast-flagged working group whose design it claims.
    pub(super) fn lane(&self, head: &wire::ResidencyHead, row: &wire::LaneResidency) -> bool {
        self.held(head, row) || self.groups.contains(&row.lane)
    }

    /// Whether a level other than the working groups runs one seat fast.
    pub(super) fn held(&self, head: &wire::ResidencyHead, row: &wire::LaneResidency) -> bool {
        head.fast_mode(row, self.global)
    }

    /// The lanes fast-flagged working groups run fast.
    pub(super) const fn groups(&self) -> &std::collections::BTreeSet<wire::CanonicalUuid> {
        &self.groups
    }

    /// The lanes fast-flagged working groups run fast.
    pub(super) fn into_groups(self) -> std::collections::BTreeSet<wire::CanonicalUuid> {
        self.groups
    }
}

/// Resolve one seat's fast mode for its next occupant launch across every
/// level.
pub(super) fn lane_fast_mode(
    bus: &Bus,
    head: &wire::ResidencyHead,
    row: &wire::LaneResidency,
) -> CommandResult<bool> {
    Ok(FastLevels::read(bus)?.lane(head, row))
}

/// Read the current global settings at one immutable journal horizon.
pub(super) fn read(bus: &Bus, repository: &Repository) -> Outcome<CommandAnswer> {
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
    let answer = match answer(repository, stored_fast_mode(fact.as_ref())) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::GlobalSettings(answer),
    })
}

/// Commit one global settings replacement and its replayable command receipt.
pub(super) fn set(
    bus: &Bus,
    repository: &Repository,
    epoch: wire::CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &GlobalSettingsSet,
) -> Outcome<CommandAnswer> {
    let answer = match answer(repository, request.fast_mode) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(error) = proposal.fact(
        wire::body::GlobalSettingsChanged::new(answer.fast_mode),
        FactCause::Root,
    ) {
        return typed_store_fault("settings::fact-invalid", &error.to_string());
    }
    if let Err(error) = proposal.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: request.clone(),
        },
        epoch,
        answer.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_two_words_of_the_git_name() {
        let one_word = identity(Some("c".to_owned()), None);
        assert_eq!(one_word.initials.as_deref(), Some("C"));
        assert_eq!(one_word.gravatar, None);

        let three_words = identity(
            Some("céline adams third".to_owned()),
            Some("git@example.invalid".to_owned()),
        );
        assert_eq!(three_words.initials.as_deref(), Some("CA"));
        assert!(three_words.gravatar.is_some());
    }

    #[test]
    fn identity_normalization_clears_blank_values_and_refuses_controls() {
        assert_eq!(normalize(Some("  C A  ")).as_deref(), Some("C A"));
        assert_eq!(normalize(Some(" \t ")), None);
        assert!(malformed(Some("C\nA")));
        assert!(!malformed(Some("C A")));
    }
}

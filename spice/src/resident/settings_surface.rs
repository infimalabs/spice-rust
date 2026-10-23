//! Durable global app settings and their command surface.
//!
//! The settings answer carries every born principal's display identity,
//! derived from that principal's recorded profile at the answer's horizon;
//! settings never carry or change it. Git configuration only seeds the profile
//! an admission records.

use bus::Bus;
use gix::bstr::ByteSlice as _;
use journal::FactCause;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, GlobalSettingsAnswer, GlobalSettingsSet};
use wire::principal::{PrincipalDirectory, PrincipalProfile, PrincipalRoles};
use wire::{Command, Outcome, Producer, Reply, ReplyPosition, SeqRange};

use super::shutdown::current_residency;
use super::{
    CommandEffects, CommandResult, commit, seq, store_fault, typed_outcome, typed_store_fault,
    write,
};

fn stored_fast_mode(envelope: Option<&wire::Envelope<wire::AnyFact>>) -> bool {
    envelope
        .and_then(|envelope| envelope.fact.read::<wire::body::GlobalSettingsChanged>())
        .is_some_and(|settings| settings.fast_mode)
}

pub(super) fn normalize(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn malformed(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value.chars().any(char::is_control)
    })
}

/// The profile the repository's Git `user.name` and `user.email` describe,
/// which an admission copies when it is given none.
pub(super) fn git_profile(repository: &Repository) -> CommandResult<PrincipalProfile> {
    let git = gix::open(repository.worktree_root()).map_err(|error| {
        super::environment_fault("principal::git-profile-unavailable", &error.to_string())
    })?;
    let config = git.config_snapshot();
    let read = |key| {
        config
            .string(key)
            .and_then(|value| value.to_str().ok().and_then(|value| normalize(Some(value))))
            .filter(|value| !malformed(Some(value)))
    };
    Ok(PrincipalProfile {
        name: read("user.name"),
        email: read("user.email"),
    })
}

fn answer(fast_mode: bool, roles: &PrincipalRoles) -> GlobalSettingsAnswer {
    GlobalSettingsAnswer {
        fast_mode,
        principals: PrincipalDirectory::display(roles.heads()),
    }
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

    /// The global setting.
    pub(super) const fn global(&self) -> bool {
        self.global
    }

    /// Whether one seat runs fast: its own flag, its team's flag, the global
    /// setting, or a fast-flagged working group whose design it claims.
    pub(super) fn lane(&self, head: &wire::ResidencyHead, row: &wire::LaneResidency) -> bool {
        self.lane_under(self.global, head, row)
    }

    /// Whether one seat runs fast were the global setting `global`.
    pub(super) fn lane_under(
        &self,
        global: bool,
        head: &wire::ResidencyHead,
        row: &wire::LaneResidency,
    ) -> bool {
        head.fast_mode(row, global) || self.groups.contains(&row.lane)
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
/// level, off while its vendor's refusal of fast mode stands.
pub(super) fn lane_fast_mode(
    bus: &Bus,
    head: &wire::ResidencyHead,
    row: &wire::LaneResidency,
) -> CommandResult<bool> {
    Ok(FastLevels::read(bus)?.lane(head, row) && !super::lane_surface::refuses_fast_mode(row))
}

/// Read the current global settings at one immutable journal horizon.
pub(super) fn read(bus: &Bus) -> Outcome<CommandAnswer> {
    let (cursor, fact, roles) = match bus.read(|read| {
        Ok((
            read.cursor()?,
            read.latest_fact(wire::FactKind::GlobalSettingsChanged)?,
            read.principal_roles()?,
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
    let answer = answer(stored_fast_mode(fact.as_ref()), &roles);
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::GlobalSettings(answer),
    })
}

/// Commit one global settings replacement and its replayable command receipt,
/// then owe each running member whose effective fast mode it flips a live
/// change.
pub(super) fn set(
    bus: &Bus,
    epoch: wire::CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &GlobalSettingsSet,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    let answer = match super::principal_surface::roles(bus) {
        Ok(roles) => answer(request.fast_mode, &roles),
        Err(outcome) => return typed_outcome(outcome),
    };
    let (levels, residency) = match FastLevels::read(bus).and_then(|levels| {
        current_residency(bus, epoch).map(|residency| (levels, residency.into_body()))
    }) {
        Ok(current) => current,
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
        Ok(committed) => {
            super::lane_surface::owe_global_fast_flips(
                effects,
                command,
                &residency,
                &levels,
                answer.fast_mode,
            );
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(SeqRange {
                    first: seq(committed.first()),
                    last: seq(committed.last()),
                }),
                body: CommandAnswer::GlobalSettings(answer),
            })
        }
        Err(outcome) => typed_outcome(outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_two_words_of_the_recorded_name() {
        let one_word = PrincipalProfile {
            name: Some("c".to_owned()),
            email: None,
        }
        .identity();
        assert_eq!(one_word.initials.as_deref(), Some("C"));
        assert_eq!(one_word.gravatar, None);

        let three_words = PrincipalProfile {
            name: Some("céline adams third".to_owned()),
            email: Some("git@example.invalid".to_owned()),
        }
        .identity();
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

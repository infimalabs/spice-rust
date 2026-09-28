//! Durable global Canvas settings and their command surface.

use bus::Bus;
use gix::bstr::ByteSlice as _;
use journal::FactCause;
use sha2::{Digest as _, Sha256};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, GlobalSettingsAnswer, GlobalSettingsSet};
use wire::{Command, OperatorIdentity, Outcome, Producer, Reply, ReplyPosition, SeqRange};

use super::{CommandResult, commit, seq, store_fault, typed_outcome, typed_store_fault, write};

#[derive(Default)]
struct OperatorValues {
    name: Option<String>,
    email: Option<String>,
}

struct StoredSettings {
    fast_mode: bool,
    operator: OperatorValues,
}

fn stored(envelope: Option<&wire::Envelope<wire::AnyFact>>) -> StoredSettings {
    envelope
        .and_then(|envelope| envelope.fact.read::<wire::body::GlobalSettingsChanged>())
        .map_or(
            StoredSettings {
                fast_mode: false,
                operator: OperatorValues::default(),
            },
            |settings| StoredSettings {
                fast_mode: settings.fast_mode,
                operator: OperatorValues {
                    name: settings.operator_name.clone(),
                    email: settings.operator_email.clone(),
                },
            },
        )
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

fn identity(explicit: &OperatorValues, inherited: &OperatorValues) -> OperatorIdentity {
    let name = explicit.name.clone().or_else(|| inherited.name.clone());
    let email = explicit.email.clone().or_else(|| inherited.email.clone());
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

fn inherited_operator(repository: &Repository) -> CommandResult<OperatorValues> {
    let git = gix::open(repository.worktree_root()).map_err(|error| {
        super::environment_fault("canvas::operator-identity-unavailable", &error.to_string())
    })?;
    let config = git.config_snapshot();
    let read = |key| {
        config
            .string(key)
            .and_then(|value| value.to_str().ok().and_then(|value| normalize(Some(value))))
            .filter(|value| !malformed(Some(value)))
    };
    Ok(OperatorValues {
        name: read("user.name"),
        email: read("user.email"),
    })
}

fn answer(
    repository: &Repository,
    settings: StoredSettings,
) -> CommandResult<GlobalSettingsAnswer> {
    let operator = identity(&settings.operator, &inherited_operator(repository)?);
    Ok(GlobalSettingsAnswer {
        fast_mode: settings.fast_mode,
        operator_name: settings.operator.name,
        operator_email: settings.operator.email,
        operator,
    })
}

/// Resolve the current settings and effective operator identity for bootstrap.
pub(super) fn current(bus: &Bus, repository: &Repository) -> CommandResult<GlobalSettingsAnswer> {
    let fact = bus
        .read(|read| read.latest_fact(wire::FactKind::GlobalSettingsChanged))
        .map_err(|error| store_fault("settings::journal-unreadable", &error.to_string()))?;
    answer(repository, stored(fact.as_ref()))
}

/// Resolve the current fast-mode value for one later occupant launch.
pub(super) fn fast_mode(bus: &Bus) -> CommandResult<bool> {
    bus.read(|read| read.latest_fact(wire::FactKind::GlobalSettingsChanged))
        .map(|fact| stored(fact.as_ref()).fast_mode)
        .map_err(|error| store_fault("settings::journal-unreadable", &error.to_string()))
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
    let answer = match answer(repository, stored(fact.as_ref())) {
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
    if malformed(request.operator_name.as_deref()) || malformed(request.operator_email.as_deref()) {
        return settings_refusal(
            "settings::operator-identity-invalid",
            "name and email must each be one trimmed line",
        );
    }
    let stored = StoredSettings {
        fast_mode: request.fast_mode,
        operator: OperatorValues {
            name: normalize(request.operator_name.as_deref()),
            email: normalize(request.operator_email.as_deref()),
        },
    };
    let answer = match answer(repository, stored) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(error) = proposal.fact(
        wire::body::GlobalSettingsChanged {
            fast_mode: answer.fast_mode,
            operator_name: answer.operator_name.clone(),
            operator_email: answer.operator_email.clone(),
            operator: Some(answer.operator.clone()),
        },
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

fn settings_refusal(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(wire::Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("global-settings".to_owned()),
            Some("single-line operator identity values".to_owned()),
            None,
        )
        .with_detail(detail),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_identity_precedence_and_initials_are_field_local() {
        let inherited = OperatorValues {
            name: Some("c".to_owned()),
            email: Some("git@example.invalid".to_owned()),
        };
        let one_word = identity(&OperatorValues::default(), &inherited);
        assert_eq!(one_word.name.as_deref(), Some("c"));
        assert_eq!(one_word.initials.as_deref(), Some("C"));

        let explicit = OperatorValues {
            name: Some("céline adams third".to_owned()),
            email: None,
        };
        let two_words = identity(&explicit, &inherited);
        assert_eq!(two_words.name.as_deref(), Some("céline adams third"));
        assert_eq!(two_words.email.as_deref(), Some("git@example.invalid"));
        assert_eq!(two_words.initials.as_deref(), Some("CA"));
    }

    #[test]
    fn identity_normalization_clears_blank_values_and_refuses_controls() {
        assert_eq!(normalize(Some("  C A  ")).as_deref(), Some("C A"));
        assert_eq!(normalize(Some(" \t ")), None);
        assert!(malformed(Some("C\nA")));
        assert!(!malformed(Some("C A")));
    }
}

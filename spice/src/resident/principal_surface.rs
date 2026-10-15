//! Born human principals: the operator's admission, profile, and role
//! commands, and the first start's birth of the repository's own human.
//!
//! Every write re-proves, inside its transaction, the head it was computed
//! from, so concurrent edits refuse rather than overwrite one another.

use std::collections::BTreeSet;

use bus::{Bus, JournalCommit};
use journal::{BuildError, FactCause, GuardError, JournalGuard, JournalWrite};
use spice::repository::Repository;
use wire::intent::CommandAnswer;
use wire::principal::{
    PrincipalAdmit, PrincipalAdmitted, PrincipalEdit, PrincipalGranted, PrincipalHead,
    PrincipalProfile, PrincipalProfiled, PrincipalRevoked, PrincipalRole, PrincipalRoleRef,
    PrincipalStanding, PrincipalsAnswer,
};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Outcome, Producer, Refusal, RefusalDiagnostic,
    Reply, ReplyPosition, Seq, SeqRange,
};

use super::settings_surface::{git_operator, malformed, normalize};
use super::{
    CommandResult, commit, new_uuid, seq, store_fault, typed_outcome, typed_store_fault, write,
};

/// Every born principal in birth order at one immutable journal horizon.
pub(super) fn read(bus: &Bus) -> Outcome<CommandAnswer> {
    let (cursor, heads) = match bus.read(|read| Ok((read.cursor()?, read.principal_heads()?))) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return typed_store_fault("principal::journal-unreadable", &error.to_string());
        }
    };
    let Some(cursor) = cursor else {
        return typed_store_fault(
            "principal::journal-empty",
            "a running resident has no startup authority fact",
        );
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::Principals(PrincipalsAnswer {
            principals: heads
                .into_iter()
                .map(journal::JournalHead::into_body)
                .collect(),
        }),
    })
}

/// Bear one principal with the given or Git-copied profile.
pub(super) fn admit(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &PrincipalAdmit,
) -> Outcome<CommandAnswer> {
    let profile = if request.name.is_none() && request.email.is_none() {
        match git_operator(repository) {
            Ok(operator) => PrincipalProfile {
                name: operator.name,
                email: operator.email,
            },
            Err(outcome) => return typed_outcome(outcome),
        }
    } else {
        match given_profile(request.name.as_deref(), request.email.as_deref()) {
            Ok(profile) => profile,
            Err(refusal) => return refusal,
        }
    };
    let standing = PrincipalStanding {
        principal: new_uuid(),
        profile,
        roles: request.roles.clone(),
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(error) = birth(&mut proposal, &standing, FactCause::Root) {
        return typed_store_fault("principal::fact-invalid", &error.to_string());
    }
    finish(bus, proposal, epoch, command, request.clone(), standing)
}

/// Replace the given fields of one principal's profile; an absent field keeps
/// its value and a blank one clears it.
pub(super) fn edit(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &PrincipalEdit,
) -> Outcome<CommandAnswer> {
    let given = match given_profile(request.name.as_deref(), request.email.as_deref()) {
        Ok(given) => given,
        Err(refusal) => return refusal,
    };
    let (position, head) = match current(bus, request.principal) {
        Ok(current) => current,
        Err(outcome) => return outcome,
    };
    let profile = PrincipalProfile {
        name: if request.name.is_some() {
            given.name
        } else {
            head.profile.name.clone()
        },
        email: if request.email.is_some() {
            given.email
        } else {
            head.profile.email.clone()
        },
    };
    if profile == head.profile {
        return refused(
            "principal::profile-unchanged",
            request.principal,
            "a profile differing from the recorded one",
            None,
        );
    }
    let standing = PrincipalStanding {
        principal: head.principal,
        profile: profile.clone(),
        roles: head.roles.clone(),
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(unmoved(head.principal, position));
    let fact = match proposal.fact(
        PrincipalProfiled {
            principal: head.principal,
            from: head.profile.clone(),
            profile: profile.clone(),
        },
        FactCause::Existing(position),
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("principal::fact-invalid", &error.to_string()),
    };
    proposal.principal_head(fact, move |_| PrincipalHead { profile, ..head });
    finish(bus, proposal, epoch, command, request.clone(), standing)
}

/// Grant or revoke one role. The operator role never leaves its last holder.
pub(super) fn role(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &PrincipalRoleRef,
) -> Outcome<CommandAnswer> {
    let grant = command.kind() == CommandKind::PrincipalGrant;
    let (principal, role) = (request.principal, request.role);
    let (position, head) = match current(bus, principal) {
        Ok(current) => current,
        Err(outcome) => return outcome,
    };
    if grant == head.holds(role) {
        let (reason, expected) = if grant {
            ("principal::role-held", "a role the principal does not hold")
        } else {
            ("principal::role-absent", "a role the principal holds")
        };
        return refused(reason, principal, expected, Some(role.as_str().to_owned()));
    }
    let last = !grant && role == PrincipalRole::Operator;
    if last {
        match bus.read(move |read| other_holder(read, principal, role)) {
            Ok(true) => {}
            Ok(false) => return last_operator(principal),
            Err(error) => {
                return typed_store_fault("principal::journal-unreadable", &error.to_string());
            }
        }
    }
    let mut roles = head.roles.clone();
    if grant {
        roles.insert(role);
    } else {
        roles.remove(&role);
    }
    let standing = PrincipalStanding {
        principal,
        profile: head.profile.clone(),
        roles: roles.clone(),
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(unmoved(principal, position));
    if last {
        proposal.guard(JournalGuard::new(move |read| {
            if other_holder(read, principal, role)? {
                Ok(())
            } else {
                Err(GuardError::refused(LAST_OPERATOR))
            }
        }));
    }
    let cause = FactCause::Existing(position);
    let fact = if grant {
        proposal.fact(PrincipalGranted { principal, role }, cause)
    } else {
        proposal.fact(PrincipalRevoked { principal, role }, cause)
    };
    let fact = match fact {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("principal::fact-invalid", &error.to_string()),
    };
    proposal.principal_head(fact, move |_| PrincipalHead { roles, ..head });
    finish(bus, proposal, epoch, command, request.clone(), standing)
}

/// The current role heads every principal authority decision reads.
pub(super) fn roles(bus: &Bus) -> CommandResult<wire::principal::PrincipalRoles> {
    bus.read(|read| read.principal_roles())
        .map_err(|error| store_fault("principal::journal-unreadable", &error.to_string()))
}

/// On a journal holding no principal, bear the repository's human from its
/// Git identity as the operator. Any born principal makes this a no-op, so
/// restarts bear nothing more.
pub(super) fn bootstrap(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    cause: FactCause,
) -> CommandResult<Option<JournalCommit>> {
    let born = bus
        .read(|read| Ok(!read.principal_heads()?.is_empty()))
        .map_err(|error| store_fault("principal::journal-unreadable", &error.to_string()))?;
    if born {
        return Ok(None);
    }
    let operator = git_operator(repository)?;
    let standing = PrincipalStanding {
        principal: new_uuid(),
        profile: PrincipalProfile {
            name: operator.name,
            email: operator.email,
        },
        roles: BTreeSet::from([PrincipalRole::Operator]),
    };
    let mut pending = write(Actor::Spice, epoch, Producer::CommandDoor)?;
    pending.guard(JournalGuard::new(|read| {
        if read.principal_heads()?.is_empty() {
            Ok(())
        } else {
            Err(GuardError::refused("principal::already-born"))
        }
    }));
    birth(&mut pending, &standing, cause)
        .map_err(|error| store_fault("principal::fact-invalid", &error.to_string()))?;
    commit(bus, pending).map(Some)
}

const LAST_OPERATOR: &str = "principal::last-operator";

fn birth(
    proposal: &mut JournalWrite,
    standing: &PrincipalStanding,
    cause: FactCause,
) -> Result<(), BuildError> {
    let PrincipalStanding {
        principal,
        profile,
        roles,
    } = standing.clone();
    let fact = proposal.fact(
        PrincipalAdmitted {
            principal,
            profile: profile.clone(),
            roles: roles.clone(),
        },
        cause,
    )?;
    proposal.principal_head(fact, move |born| PrincipalHead {
        principal,
        born,
        profile,
        roles,
    });
    Ok(())
}

fn given_profile(
    name: Option<&str>,
    email: Option<&str>,
) -> Result<PrincipalProfile, Outcome<CommandAnswer>> {
    for (field, value) in [("name", name), ("email", email)] {
        if malformed(value) {
            return Err(Outcome::Refusal(Refusal::bare(RefusalDiagnostic::new(
                "principal::profile-malformed",
                Some(field.to_owned()),
                Some("one line without control characters".to_owned()),
                None,
            ))));
        }
    }
    Ok(PrincipalProfile {
        name: normalize(name),
        email: normalize(email),
    })
}

fn current(
    bus: &Bus,
    principal: CanonicalUuid,
) -> Result<(Seq, PrincipalHead), Outcome<CommandAnswer>> {
    match bus.read(move |read| read.principal_head(principal)) {
        Ok(Some(head)) => Ok((head.position(), head.into_body())),
        Ok(None) => Err(refused(
            "principal::missing",
            principal,
            "a born principal",
            None,
        )),
        Err(error) => Err(typed_store_fault(
            "principal::journal-unreadable",
            &error.to_string(),
        )),
    }
}

fn other_holder(
    read: &journal::JournalRead<'_>,
    principal: CanonicalUuid,
    role: PrincipalRole,
) -> Result<bool, GuardError> {
    Ok(read
        .principal_heads()?
        .iter()
        .any(|head| head.body().principal != principal && head.body().holds(role)))
}

fn unmoved(principal: CanonicalUuid, position: Seq) -> JournalGuard {
    JournalGuard::new(move |read| match read.principal_head(principal)? {
        Some(head) if head.position() == position => Ok(()),
        _ => Err(GuardError::refused("principal::moved")),
    })
}

fn last_operator(principal: CanonicalUuid) -> Outcome<CommandAnswer> {
    refused(
        LAST_OPERATOR,
        principal,
        "another principal holding the operator role",
        Some("none".to_owned()),
    )
}

fn refused(
    reason: &str,
    principal: CanonicalUuid,
    expected: &str,
    observed: Option<String>,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(RefusalDiagnostic::new(
        reason,
        Some(principal.to_string()),
        Some(expected.to_owned()),
        observed,
    )))
}

fn finish<B>(
    bus: &Bus,
    mut proposal: JournalWrite,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: B,
    standing: PrincipalStanding,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize,
{
    if let Err(error) = proposal.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: request,
        },
        epoch,
        standing.clone(),
    ) {
        return typed_store_fault("principal::receipt-invalid", &error.to_string());
    }
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Principal(standing),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

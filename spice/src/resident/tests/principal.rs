//! Human principal birth, profile, and role authority.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;

use wire::principal::{
    PrincipalAdmit, PrincipalEdit, PrincipalHead, PrincipalRead, PrincipalRole, PrincipalRoleRef,
};

use super::*;
use crate::resident::principal_surface;

const GIT_NAME: &str = "Ada Operator";
const GIT_EMAIL: &str = "ada@example.invalid";

struct Seated {
    repository: Repository,
    surface: crate::trust_command::Surface,
    bus: Bus,
}

fn seated(name: &str) -> Seated {
    let (root, paths) = resident_fixture(name);
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(root.join(".git/config"))
            .expect("open fixture repository config"),
        "[user]\n\tname = {GIT_NAME}\n\temail = {GIT_EMAIL}"
    )
    .expect("configure fixture Git identity");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &surface, test_uuid(1), None).expect("resident seats");
    Seated {
        repository,
        surface,
        bus,
    }
}

fn heads(bus: &Bus) -> Vec<PrincipalHead> {
    bus.read(|read| read.principal_heads())
        .expect("principal heads read")
        .into_iter()
        .map(journal::JournalHead::into_body)
        .collect()
}

fn command<B: serde::Serialize>(
    kind: CommandKind,
    body: &B,
    actor: Actor,
    id: u8,
) -> Result<wire::AdmittedCommand, wire::CommandRejection> {
    wire::Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(id),
        epoch: test_uuid(1),
        kind,
        expected: Vec::new(),
        body: serde_json::to_value(body).expect("principal body encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor,
        epoch: test_uuid(1),
        release: "1.2.3".parse().expect("fixture release"),
    })
}

fn operator<B: serde::Serialize>(kind: CommandKind, body: &B, id: u8) -> wire::AdmittedCommand {
    command(kind, body, Actor::Operator, id).expect("the operator's principal command admits")
}

fn role(
    seated: &Seated,
    kind: CommandKind,
    reference: PrincipalRoleRef,
    id: u8,
) -> Outcome<CommandAnswer> {
    let command = operator(kind, &reference, id);
    principal_surface::role(&seated.bus, test_uuid(1), &command, &reference)
}

fn refusal_reason(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn admitted(seated: &Seated, request: PrincipalAdmit, id: u8) -> wire::CanonicalUuid {
    let command = operator(CommandKind::PrincipalAdmit, &request, id);
    match principal_surface::admit(
        &seated.bus,
        &seated.repository,
        test_uuid(1),
        &command,
        &request,
    ) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Principal(standing),
            ..
        }) => standing.principal,
        other => panic!("expected an admission, got {other:?}"),
    }
}

#[test]
fn first_start_bears_one_git_profiled_operator_and_restarts_bear_nothing_more() {
    let seated = seated("principal-bootstrap");
    let [born] = heads(&seated.bus)
        .try_into()
        .expect("exactly one principal");
    assert!(born.holds(PrincipalRole::Operator));
    assert_eq!(born.profile.name.as_deref(), Some(GIT_NAME));
    assert_eq!(born.profile.email.as_deref(), Some(GIT_EMAIL));

    commit_startup(
        &seated.bus,
        &seated.repository,
        &seated.surface,
        test_uuid(9),
        None,
    )
    .expect("the resident restarts");
    assert_eq!(heads(&seated.bus), vec![born], "a restart bears nobody");
    let admissions = seated
        .bus
        .fact_snapshot()
        .expect("facts read")
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::PrincipalAdmitted)
        .count();
    assert_eq!(admissions, 1);
}

#[test]
fn the_operator_role_never_leaves_its_last_holder() {
    let seated = seated("principal-last-operator");
    let first = heads(&seated.bus)[0].principal;
    let revoke_first = PrincipalRoleRef {
        principal: first,
        role: PrincipalRole::Operator,
    };

    let refused = role(
        &seated,
        CommandKind::PrincipalRevoke,
        revoke_first.clone(),
        10,
    );
    assert_eq!(refusal_reason(&refused), "principal::last-operator");

    let second = admitted(
        &seated,
        PrincipalAdmit {
            name: Some("Bea".to_owned()),
            email: None,
            roles: BTreeSet::from([PrincipalRole::Operator]),
        },
        11,
    );
    let before = heads(&seated.bus);
    assert!(matches!(
        role(&seated, CommandKind::PrincipalRevoke, revoke_first, 12),
        Outcome::Reply(_)
    ));
    let after = heads(&seated.bus);
    assert!(
        after[0].roles.is_empty(),
        "the revoked holder keeps no role"
    );
    assert_eq!(
        (after[0].principal, after[0].born, &after[0].profile),
        (before[0].principal, before[0].born, &before[0].profile),
        "a role change moves only the role"
    );
    assert_eq!(after[1], before[1], "the other principal is untouched");

    let refused = role(
        &seated,
        CommandKind::PrincipalRevoke,
        PrincipalRoleRef {
            principal: second,
            role: PrincipalRole::Operator,
        },
        13,
    );
    assert_eq!(refusal_reason(&refused), "principal::last-operator");
    assert_eq!(
        heads(&seated.bus),
        after,
        "a refused revocation writes nothing"
    );
}

#[test]
fn a_profile_edit_moves_only_its_own_profile_and_keeps_omitted_fields() {
    let seated = seated("principal-profile");
    let other = admitted(
        &seated,
        PrincipalAdmit {
            name: Some("Bea".to_owned()),
            email: Some("bea@example.invalid".to_owned()),
            roles: BTreeSet::new(),
        },
        10,
    );
    let before = heads(&seated.bus);
    let edit = PrincipalEdit {
        principal: before[0].principal,
        name: Some("Ada Renamed".to_owned()),
        email: None,
    };
    let command = operator(CommandKind::PrincipalEdit, &edit, 11);
    assert!(matches!(
        principal_surface::edit(&seated.bus, test_uuid(1), &command, &edit),
        Outcome::Reply(_)
    ));

    let after = heads(&seated.bus);
    assert_eq!(after[0].profile.name.as_deref(), Some("Ada Renamed"));
    assert_eq!(after[0].profile.email.as_deref(), Some(GIT_EMAIL));
    assert_eq!(
        (after[0].principal, after[0].born, &after[0].roles),
        (before[0].principal, before[0].born, &before[0].roles),
        "identity, birth, and roles never move with the profile"
    );
    assert_eq!(after[1].principal, other);
    assert_eq!(after[1], before[1], "the other principal is untouched");
}

#[test]
fn lanes_cannot_list_admit_profile_or_change_principal_roles() {
    let lane = Actor::Lane(test_uuid(2));
    let principal = test_uuid(5);
    let reference = PrincipalRoleRef {
        principal,
        role: PrincipalRole::Operator,
    };
    let refused = [
        command(
            CommandKind::PrincipalRead,
            &PrincipalRead {},
            lane.clone(),
            10,
        ),
        command(
            CommandKind::PrincipalAdmit,
            &PrincipalAdmit::default(),
            lane.clone(),
            11,
        ),
        command(
            CommandKind::PrincipalEdit,
            &PrincipalEdit {
                principal,
                name: Some("Lane".to_owned()),
                email: None,
            },
            lane.clone(),
            12,
        ),
        command(CommandKind::PrincipalGrant, &reference, lane.clone(), 13),
        command(CommandKind::PrincipalRevoke, &reference, lane, 14),
    ];
    for rejection in refused {
        let Err(wire::CommandRejection::Refusal(refusal)) = rejection else {
            panic!("a lane's principal command must refuse, got {rejection:?}");
        };
        assert_eq!(refusal.reason(), "command::authority");
    }
}

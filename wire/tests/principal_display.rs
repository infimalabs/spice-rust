//! Naming an acting or authoring human by their principal's recorded profile.

use std::collections::BTreeSet;

use wire::principal::{
    PrincipalDirectory, PrincipalHead, PrincipalProfile, PrincipalRole, PrincipalRoles,
};
use wire::{Actor, CanonicalUuid, Seq};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("canonical fixture UUID")
}

fn head(principal: u8, born: u64, name: Option<&str>) -> PrincipalHead {
    PrincipalHead {
        principal: uuid(principal),
        born: Seq::new(born).expect("fixture position"),
        profile: PrincipalProfile {
            name: name.map(str::to_owned),
            ..PrincipalProfile::default()
        },
        roles: BTreeSet::from([PrincipalRole::Operator]),
        capability: None,
    }
}

fn named(directory: &PrincipalDirectory, actor: &Actor) -> String {
    directory.named(actor).to_string()
}

#[test]
fn each_actor_is_named_by_the_principal_it_acts_or_authored_as() {
    // Heads arrive in any order; birth position alone decides which principal
    // the historical operator actor authored as.
    let roles = PrincipalRoles::new([head(92, 9, Some("Second")), head(91, 3, Some("Owner"))]);
    let directory = PrincipalDirectory::names(&roles);

    assert_eq!(named(&directory, &Actor::Operator), "Owner <operator>");
    assert_eq!(
        named(&directory, &Actor::Human(uuid(91))),
        format!("Owner <human:{}>", uuid(91))
    );
    assert_eq!(
        named(&directory, &Actor::Human(uuid(92))),
        format!("Second <human:{}>", uuid(92))
    );
    for nobody in [Actor::Human(uuid(93)), Actor::Lane(uuid(91)), Actor::Spice] {
        assert_eq!(directory.identity(&nobody), None, "{nobody} named a human");
        assert_eq!(named(&directory, &nobody), nobody.to_string());
    }
    for actor in [
        Actor::Operator,
        Actor::Human(uuid(92)),
        Actor::Human(uuid(93)),
    ] {
        assert_eq!(
            roles
                .head(&actor)
                .and_then(|head| head.profile.name.as_deref()),
            directory
                .identity(&actor)
                .and_then(|identity| identity.name.as_deref()),
            "display and authority resolved {actor} to different principals"
        );
    }

    let admitted = PrincipalRoles::new([
        head(92, 9, Some("Second")),
        head(91, 3, Some("Owner")),
        head(94, 12, Some("Later")),
    ]);
    let directory = PrincipalDirectory::names(&admitted);
    assert_eq!(named(&directory, &Actor::Operator), "Owner <operator>");
    assert_eq!(
        named(&directory, &Actor::Human(uuid(94))),
        format!("Later <human:{}>", uuid(94))
    );
}

#[test]
fn a_human_whose_profile_records_no_name_reads_as_its_identity() {
    let directory = PrincipalDirectory::names(&PrincipalRoles::new([head(91, 3, None)]));
    assert!(directory.identity(&Actor::Operator).is_some());
    assert_eq!(named(&directory, &Actor::Operator), "operator");
    assert_eq!(
        named(&directory, &Actor::Human(uuid(91))),
        format!("human:{}", uuid(91))
    );
}

#[test]
fn before_any_birth_no_actor_is_named() {
    let directory = PrincipalDirectory::names(&PrincipalRoles::default());
    assert_eq!(directory.identity(&Actor::Operator), None);
    assert_eq!(directory.identity(&Actor::Human(uuid(91))), None);
    assert_eq!(named(&directory, &Actor::Operator), "operator");
}

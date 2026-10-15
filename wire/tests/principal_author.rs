//! Read-time author resolution: the historical operator actor names the
//! repository's first-born principal, and no later admission moves it.

use std::collections::BTreeSet;

use wire::principal::{PrincipalHead, PrincipalProfile, PrincipalRole, PrincipalRoles};
use wire::{Actor, CanonicalUuid, Seq};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("canonical fixture UUID")
}

fn head(principal: u8, born: u64, roles: &[PrincipalRole]) -> PrincipalHead {
    PrincipalHead {
        principal: uuid(principal),
        born: Seq::new(born).expect("fixture position"),
        profile: PrincipalProfile::default(),
        roles: roles.iter().copied().collect::<BTreeSet<_>>(),
        capability: None,
    }
}

const BOOTSTRAP: u8 = 40;
const SECOND: u8 = 30;

fn others() -> [Actor; 4] {
    [
        Actor::Spice,
        Actor::Lane(uuid(20)),
        Actor::Human(uuid(BOOTSTRAP)),
        Actor::Human(uuid(SECOND)),
    ]
}

#[test]
fn before_the_bootstrap_birth_every_actor_names_itself() {
    let roles = PrincipalRoles::default();
    assert_eq!(roles.author(&Actor::Operator), Actor::Operator);
    for actor in others() {
        assert_eq!(roles.author(&actor), actor);
    }
}

#[test]
fn the_operator_names_the_first_born_principal_through_later_admissions() {
    let bootstrap = Actor::Human(uuid(BOOTSTRAP));
    let first = PrincipalRoles::new([head(BOOTSTRAP, 7, &[PrincipalRole::Operator])]);
    assert_eq!(first.author(&Actor::Operator), bootstrap);

    // A later birth with a lower identity, listed first, that takes the
    // operator role from the bootstrap principal still moves nothing.
    let second = PrincipalRoles::new([
        head(SECOND, 9, &[PrincipalRole::Operator]),
        head(BOOTSTRAP, 7, &[]),
    ]);
    assert_eq!(second.author(&Actor::Operator), bootstrap);
    for roles in [&first, &second] {
        for actor in others() {
            assert_eq!(roles.author(&actor), actor);
        }
    }
}

//! Naming an acting or authoring human by the short name its principal's
//! recorded profile yields, or by its principal's code when it yields none.

use std::collections::BTreeSet;

use wire::principal::{
    PrincipalCode, PrincipalCodeError, PrincipalDirectory, PrincipalHead, PrincipalProfile,
    PrincipalRole, PrincipalRoles,
};
use wire::{Actor, CanonicalUuid, Digest, Domain, OccupantAddress, Seq};

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

fn code(born: u64) -> PrincipalCode {
    PrincipalCode::of(&head(0, born, None))
}

fn named(directory: &PrincipalDirectory, actor: &Actor) -> String {
    directory.named(actor).to_string()
}

#[test]
fn each_actor_is_named_by_the_principal_it_acts_or_authored_as() {
    // Heads arrive in any order; birth position alone decides which principal
    // the historical operator actor authored as.
    let roles = PrincipalRoles::new([
        head(92, 18_731, Some("Second Person")),
        head(91, 18_730, Some("C Anthony Risinger")),
    ]);
    let directory = PrincipalDirectory::names(&roles);

    assert_eq!(named(&directory, &Actor::Operator), "CA");
    assert_eq!(named(&directory, &Actor::Human(uuid(91))), "CA");
    assert_eq!(named(&directory, &Actor::Human(uuid(92))), "SP");
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
            roles.head(&actor).map(PrincipalCode::of),
            directory.principal(&actor).map(|display| display.code),
            "display and authority resolved {actor} to different principals"
        );
    }

    let admitted = PrincipalRoles::new([
        head(92, 18_731, Some("Second Person")),
        head(91, 18_730, Some("C Anthony Risinger")),
        head(94, 20_000, Some("later")),
    ]);
    let directory = PrincipalDirectory::names(&admitted);
    assert_eq!(named(&directory, &Actor::Operator), "CA");
    assert_eq!(named(&directory, &Actor::Human(uuid(94))), "L");
}

#[test]
fn a_human_whose_profile_yields_no_short_name_reads_as_its_code() {
    for name in [None, Some("  ")] {
        let directory = PrincipalDirectory::names(&PrincipalRoles::new([head(91, 18_730, name)]));
        assert!(directory.identity(&Actor::Operator).is_some());
        assert_eq!(named(&directory, &Actor::Operator), "6wB");
        assert_eq!(named(&directory, &Actor::Human(uuid(91))), "6wB");
    }
}

#[test]
fn before_any_birth_no_actor_is_named() {
    let directory = PrincipalDirectory::names(&PrincipalRoles::default());
    assert_eq!(directory.identity(&Actor::Operator), None);
    assert_eq!(directory.identity(&Actor::Human(uuid(91))), None);
    assert_eq!(named(&directory, &Actor::Operator), "operator");
    assert_eq!(
        named(&directory, &Actor::Human(uuid(91))),
        format!("human:{}", uuid(91))
    );
}

#[test]
fn a_code_names_one_birth_whatever_its_head_later_records() {
    let born = head(91, 18_730, Some("Owner"));
    let mut moved = born.clone();
    moved.profile = PrincipalProfile {
        name: Some("Renamed".to_owned()),
        email: Some("owner@example.invalid".to_owned()),
    };
    moved.roles.clear();
    moved.capability = Some(Digest::compute(Domain::PrincipalCapability, b"issued"));
    assert_eq!(PrincipalCode::of(&moved), PrincipalCode::of(&born));

    let roles = PrincipalRoles::new([moved, head(92, 18_731, None)]);
    let owner = PrincipalCode::of(&born);
    assert_eq!(
        roles.coded(owner).map(|head| head.principal),
        Some(uuid(91))
    );
    assert_eq!(
        roles.coded(code(18_731)).map(|head| head.principal),
        Some(uuid(92))
    );
    // A position bearing a lane occupant, or nothing, bore no principal.
    let occupant: OccupantAddress = "6wD".parse().expect("canonical address");
    let unborn: PrincipalCode = occupant.to_string().parse().expect("same spelling");
    assert_eq!(roles.coded(unborn), None);
}

#[test]
fn only_a_canonical_nonzero_spelling_is_a_code() {
    for born in [1, 51, 52, 18_730, u64::MAX] {
        let code = code(born);
        assert_eq!(code.to_string().parse(), Ok(code));
        let encoded = serde_json::to_string(&code).expect("code encodes");
        assert_eq!(encoded, format!("\"{code}\""));
        assert_eq!(
            serde_json::from_str::<PrincipalCode>(&encoded).ok(),
            Some(code)
        );
    }
    assert_eq!("".parse::<PrincipalCode>(), Err(PrincipalCodeError::Empty));
    assert_eq!("0".parse::<PrincipalCode>(), Err(PrincipalCodeError::Zero));
    assert_eq!(
        "06wB".parse::<PrincipalCode>(),
        Err(PrincipalCodeError::LeadingZero("06wB".to_owned()))
    );
    for misspelled in ["6wa", "6 wB", "-1"] {
        assert_eq!(
            misspelled.parse::<PrincipalCode>(),
            Err(PrincipalCodeError::Alphabet(misspelled.to_owned()))
        );
    }
    let widest = "z".repeat(12);
    assert_eq!(
        widest.parse::<PrincipalCode>(),
        Err(PrincipalCodeError::Overflow(widest.clone()))
    );
    assert!(serde_json::from_str::<PrincipalCode>("\"0\"").is_err());
}

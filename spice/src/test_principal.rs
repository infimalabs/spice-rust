//! Principal role fixtures shared by resident and board tests.

use wire::principal::{PrincipalHead, PrincipalProfile, PrincipalRole, PrincipalRoles};

/// Role heads in which the operator actor's bootstrap principal is born
/// holding exactly `roles`.
pub(crate) fn bootstrap_holding(roles: &[PrincipalRole]) -> PrincipalRoles {
    PrincipalRoles::new([PrincipalHead {
        principal: "018f82d4-6f59-7d06-92e3-bce9131163b0"
            .parse()
            .expect("fixture principal"),
        born: wire::Seq::new(1).expect("fixture birth"),
        profile: PrincipalProfile::default(),
        roles: roles.iter().copied().collect(),
        capability: None,
    }])
}

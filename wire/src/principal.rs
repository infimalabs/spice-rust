//! Born human principals: their recorded profile, held roles, and the facts
//! and commands that change them.
//!
//! A principal is born once, by an explicit admission fact, at an immutable
//! journal position. Its identity and birth position never change; a profile
//! edit or a role grant or revocation is its own fact and moves only that
//! principal's head. Git configuration is at most the source an admission
//! copies its profile from. No principal carries a short code or any other
//! addressing route: humans are reached through concern pointers only.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CanonicalUuid, Seq};

/// A principal's recorded display profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalProfile {
    /// Display name, absent when none was recorded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Email address, absent when none was recorded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub email: Option<String>,
}

/// One authority a principal may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalRole {
    /// Operator authority over the repository's lanes, board, and policy.
    Operator,
}

impl PrincipalRole {
    /// The role as it is spelled on the wire and the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }
}

impl std::str::FromStr for PrincipalRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "operator" => Ok(Self::Operator),
            other => Err(format!(
                "unknown principal role {other:?}; expected operator"
            )),
        }
    }
}

/// One principal's current standing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalHead {
    /// Identity fixed at birth.
    pub principal: CanonicalUuid,
    /// Journal position of the admission fact that bore this principal.
    pub born: Seq,
    /// Current recorded profile.
    pub profile: PrincipalProfile,
    /// Roles currently held.
    pub roles: BTreeSet<PrincipalRole>,
}

impl PrincipalHead {
    /// Whether this principal currently holds `role`.
    #[must_use]
    pub fn holds(&self, role: PrincipalRole) -> bool {
        self.roles.contains(&role)
    }
}

/// A human principal was born with its first profile and roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalAdmitted {
    /// Identity of the principal born here.
    pub principal: CanonicalUuid,
    /// Profile recorded at birth.
    pub profile: PrincipalProfile,
    /// Roles held from birth.
    pub roles: BTreeSet<PrincipalRole>,
}

/// One principal's recorded profile changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalProfiled {
    /// Principal whose profile changed.
    pub principal: CanonicalUuid,
    /// Profile replaced by this fact.
    pub from: PrincipalProfile,
    /// Profile recorded by this fact.
    pub profile: PrincipalProfile,
}

/// One principal gained a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalGranted {
    /// Principal gaining the role.
    pub principal: CanonicalUuid,
    /// Role granted.
    pub role: PrincipalRole,
}

/// One principal lost a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRevoked {
    /// Principal losing the role.
    pub principal: CanonicalUuid,
    /// Role revoked.
    pub role: PrincipalRole,
}

/// Read every born principal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRead {}

/// Bear one new principal.
///
/// With neither field given, the profile is copied from the repository's Git
/// `user.name` and `user.email`; otherwise exactly the given fields are
/// recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalAdmit {
    /// Explicit display name.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Explicit email address.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub email: Option<String>,
    /// Roles held from birth.
    pub roles: BTreeSet<PrincipalRole>,
}

/// Change one principal's recorded profile. An absent field keeps its current
/// value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalEdit {
    /// Principal whose profile changes.
    pub principal: CanonicalUuid,
    /// Replacement display name.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Replacement email address.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub email: Option<String>,
}

/// Grant or revoke one role of one principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRoleRef {
    /// Principal whose role changes.
    pub principal: CanonicalUuid,
    /// Role granted or revoked.
    pub role: PrincipalRole,
}

/// Every born principal in birth order, as of the answering position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalsAnswer {
    /// Current principal heads, oldest birth first.
    pub principals: Vec<PrincipalHead>,
}

/// One principal's standing as an admission, profile edit, or role change
/// left it. Birth position is assigned at commit, so a write answers without
/// it; [`PrincipalsAnswer`] carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalStanding {
    /// Principal the write named or bore.
    pub principal: CanonicalUuid,
    /// Profile recorded after the write.
    pub profile: PrincipalProfile,
    /// Roles held after the write.
    pub roles: BTreeSet<PrincipalRole>,
}

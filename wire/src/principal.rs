//! Born human principals: their recorded profile, held roles, and the facts
//! and commands that change them.
//!
//! A principal is born once, by an explicit admission fact, at an immutable
//! journal position. Its identity and birth position never change; a profile
//! edit or a role grant or revocation is its own fact and moves only that
//! principal's head. Git configuration is at most the source an admission
//! copies its profile from. No principal carries a short code or any other
//! addressing route: humans are reached through concern pointers only.
//!
//! [`PrincipalRoles`] is the one predicate deciding what an actor may do as a
//! principal: the resident's command door, its operator-only controls, lane
//! selection, steering wake, operator-direction recognition, and review
//! coverage all ask it rather than comparing an actor against the operator.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Actor, CanonicalUuid, Seq};

/// The born principal an actor acts as, before any role head is read.
///
/// Until human actors exist, only the operator actor acts as a principal, and
/// it acts as the bootstrap principal: the repository's first-born human. A
/// lane or the resident acts as no principal. This resolution alone is what
/// immutable journal soundness checks consult, because a fact's author must
/// stay valid whatever roles later change; current authority is
/// [`PrincipalRoles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActingPrincipal {
    /// The repository's first-born principal.
    Bootstrap,
}

impl ActingPrincipal {
    /// The principal `actor` acts as, absent for a lane or the resident.
    #[must_use]
    pub const fn of(actor: &Actor) -> Option<Self> {
        match actor {
            Actor::Operator => Some(Self::Bootstrap),
            Actor::Spice | Actor::Lane(_) => None,
        }
    }
}

/// Every born principal's current role head at one journal horizon: the one
/// predicate deciding which actor holds which role.
///
/// A role change is a fact moving one head, so a predicate read after it
/// answers the changed role for that principal's next command. A journal
/// holding no principal has not yet recorded its bootstrap birth, which
/// always bears the repository's human as the operator; until then the
/// operator actor holds exactly that operator role.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrincipalRoles {
    /// Born principals, oldest birth first.
    heads: Vec<PrincipalHead>,
}

impl PrincipalRoles {
    /// The predicate over these principal heads, in any order.
    #[must_use]
    pub fn new(heads: impl IntoIterator<Item = PrincipalHead>) -> Self {
        let mut heads: Vec<PrincipalHead> = heads.into_iter().collect();
        heads.sort_by_key(|head| head.born);
        Self { heads }
    }

    /// The roles `actor` currently holds as a principal, absent when it acts
    /// as no principal.
    #[must_use]
    pub fn roles(&self, actor: &Actor) -> Option<BTreeSet<PrincipalRole>> {
        match ActingPrincipal::of(actor)? {
            ActingPrincipal::Bootstrap => Some(self.heads.first().map_or_else(
                || BTreeSet::from([PrincipalRole::Operator]),
                |head| head.roles.clone(),
            )),
        }
    }

    /// Whether the principal `actor` acts as currently holds `role`.
    #[must_use]
    pub fn holds(&self, actor: &Actor, role: PrincipalRole) -> bool {
        self.roles(actor).is_some_and(|roles| roles.contains(&role))
    }

    /// Every actor whose principal currently holds `role`, in actor order.
    #[must_use]
    pub fn holders(&self, role: PrincipalRole) -> Vec<Actor> {
        std::iter::once(Actor::Operator)
            .filter(|actor| self.holds(actor, role))
            .collect()
    }
}

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

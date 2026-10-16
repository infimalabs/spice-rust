//! Born human principals: their recorded profile, held roles, and the facts
//! and commands that change them.
//!
//! A principal is born once, by an explicit admission fact, at an immutable
//! journal position. Its identity and birth position never change; a profile
//! edit or a role grant or revocation is its own fact and moves only that
//! principal's head. Git configuration is at most the source an admission
//! copies its profile from. A principal's [`PrincipalCode`] spells its birth
//! position, identifying the person as briefly as an occupant address
//! identifies an agent, but it is no addressing route: humans are reached
//! through concern pointers only.
//!
//! [`PrincipalRoles`] is the one predicate deciding what an actor may do as a
//! principal: the resident's command door, its operator-only controls, lane
//! selection, steering wake, operator-direction recognition, and review
//! coverage all ask it rather than comparing an actor against the operator.
//! [`PrincipalDirectory`] is the display counterpart: every surface naming an
//! acting or authoring human reads that principal's recorded profile through
//! the same resolution, never Git configuration.

mod code;

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub use code::{PrincipalCode, PrincipalCodeError};

use crate::{Actor, CanonicalUuid, Digest, Domain, HumanIdentity, Seq};

/// The born principal an actor acts as, before any role head is read.
///
/// A human actor acts as the principal its capability names. The historical
/// operator actor, which authored as the repository's human before human
/// connections authenticated as themselves, acts as the bootstrap principal:
/// the repository's first-born human. A lane or the resident acts as no
/// principal. This resolution alone is what immutable journal soundness checks
/// consult, because a fact's author must stay valid whatever roles later
/// change; current authority is [`PrincipalRoles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActingPrincipal {
    /// The repository's first-born principal.
    Bootstrap,
    /// The born principal with this identity.
    Born(CanonicalUuid),
}

impl ActingPrincipal {
    /// The principal `actor` acts as, absent for a lane or the resident.
    #[must_use]
    pub const fn of(actor: &Actor) -> Option<Self> {
        match actor {
            Actor::Operator => Some(Self::Bootstrap),
            Actor::Human(principal) => Some(Self::Born(*principal)),
            Actor::Spice | Actor::Lane(_) => None,
        }
    }
}

/// The entry, among `entries` in birth order, of the principal `actor` acts or
/// authored as: the first for the historical operator actor, the one `born`
/// names for a human actor, and none for a lane or the resident.
fn acting_entry<'a, T>(
    entries: &'a [T],
    actor: &Actor,
    born: impl Fn(&T) -> CanonicalUuid,
) -> Option<&'a T> {
    match ActingPrincipal::of(actor)? {
        ActingPrincipal::Bootstrap => entries.first(),
        ActingPrincipal::Born(principal) => entries.iter().find(|entry| born(entry) == principal),
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

    /// The current head of the principal `actor` acts as, or authored as: the
    /// earliest-born principal for the historical operator actor, the named
    /// principal for a human actor. Absent for a lane, the resident, a
    /// principal never born, and the operator actor before any birth. Every
    /// display of an acting or authoring human resolves through this alone.
    #[must_use]
    pub fn head(&self, actor: &Actor) -> Option<&PrincipalHead> {
        acting_entry(&self.heads, actor, |head| head.principal)
    }

    /// The current head of the principal `code` identifies, absent when no
    /// principal was born at that position.
    #[must_use]
    pub fn coded(&self, code: PrincipalCode) -> Option<&PrincipalHead> {
        self.heads
            .iter()
            .find(|head| PrincipalCode::of(head) == code)
    }

    /// Every born principal's current head, oldest birth first.
    #[must_use]
    pub fn heads(&self) -> &[PrincipalHead] {
        &self.heads
    }

    /// The roles `actor` currently holds as a principal, absent when it acts
    /// as no principal or names one never born.
    #[must_use]
    pub fn roles(&self, actor: &Actor) -> Option<BTreeSet<PrincipalRole>> {
        let acting = ActingPrincipal::of(actor)?;
        match self.head(actor) {
            Some(head) => Some(head.roles.clone()),
            None => (acting == ActingPrincipal::Bootstrap)
                .then(|| BTreeSet::from([PrincipalRole::Operator])),
        }
    }

    /// Whether the principal `actor` acts as currently holds `role`.
    #[must_use]
    pub fn holds(&self, actor: &Actor, role: PrincipalRole) -> bool {
        self.roles(actor).is_some_and(|roles| roles.contains(&role))
    }

    /// Every actor whose principal currently holds `role`, in actor order:
    /// each holding principal's human actor, and the historical operator
    /// actor while the bootstrap principal holds it.
    #[must_use]
    pub fn holders(&self, role: PrincipalRole) -> Vec<Actor> {
        let mut holders: Vec<Actor> = std::iter::once(Actor::Operator)
            .filter(|actor| self.holds(actor, role))
            .chain(
                self.heads
                    .iter()
                    .filter(|head| head.holds(role))
                    .map(|head| Actor::Human(head.principal)),
            )
            .collect();
        holders.sort();
        holders
    }

    /// The repository's first-born principal, absent before its birth.
    #[must_use]
    pub fn bootstrap(&self) -> Option<CanonicalUuid> {
        self.heads.first().map(|head| head.principal)
    }

    /// The actor a read names as the author of a fact recorded under
    /// `actor`. The historical operator actor names the bootstrap principal's
    /// human actor once that principal is born, so its history and that
    /// principal's own later facts name one person; later births never move
    /// it. Every other actor names itself. Facts keep the actor they recorded.
    #[must_use]
    pub fn author(&self, actor: &Actor) -> Actor {
        match (actor, self.bootstrap()) {
            (Actor::Operator, Some(bootstrap)) => Actor::Human(bootstrap),
            _ => actor.clone(),
        }
    }

    /// Each principal holding an issued capability, with that capability's
    /// digest.
    pub fn issued(&self) -> impl Iterator<Item = (CanonicalUuid, &Digest)> {
        self.heads.iter().filter_map(|head| {
            head.capability
                .as_ref()
                .map(|capability| (head.principal, capability))
        })
    }
}

/// One born principal's display identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDisplay {
    /// The principal displayed.
    pub principal: CanonicalUuid,
    /// The code its birth position spells.
    pub code: PrincipalCode,
    /// How its recorded profile presents.
    pub identity: HumanIdentity,
}

/// Every born principal's display identity, oldest birth first, at one
/// journal horizon: the one source naming an acting or authoring human.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrincipalDirectory(Vec<PrincipalDisplay>);

impl PrincipalDirectory {
    /// Each current head in birth order, with the complete display identity
    /// derived from its recorded profile.
    #[must_use]
    pub fn display(heads: &[PrincipalHead]) -> Self {
        Self(
            heads
                .iter()
                .map(|head| PrincipalDisplay {
                    principal: head.principal,
                    code: PrincipalCode::of(head),
                    identity: head.profile.identity(),
                })
                .collect(),
        )
    }

    /// Each head of `roles` named by its recorded profile's name and initials
    /// alone, for a reader that shows no avatar.
    #[must_use]
    pub fn names(roles: &PrincipalRoles) -> Self {
        Self(
            roles
                .heads()
                .iter()
                .map(|head| PrincipalDisplay {
                    principal: head.principal,
                    code: PrincipalCode::of(head),
                    identity: HumanIdentity {
                        name: head.profile.name.clone(),
                        initials: head.profile.initials(),
                        ..HumanIdentity::default()
                    },
                })
                .collect(),
        )
    }

    /// The principal `actor` acts or authored as, by the resolution
    /// [`PrincipalRoles::head`] applies: the earliest-born principal for the
    /// historical operator actor. Absent for a lane, the resident, and a
    /// principal this directory does not hold.
    #[must_use]
    pub fn principal(&self, actor: &Actor) -> Option<&PrincipalDisplay> {
        acting_entry(&self.0, actor, |display| display.principal)
    }

    /// How the principal `actor` acts or authored as presents, by
    /// [`Self::principal`].
    #[must_use]
    pub fn identity(&self, actor: &Actor) -> Option<&HumanIdentity> {
        self.principal(actor).map(|display| &display.identity)
    }

    /// `actor` as a reader sees it: by its principal's code and short name
    /// when it resolves, by [`Self::principal`], to a born principal.
    #[must_use]
    pub fn named<'a>(&'a self, actor: &'a Actor) -> NamedActor<'a> {
        NamedActor {
            actor,
            principal: self
                .principal(actor)
                .map(|display| (display.code, display.identity.initials.as_deref())),
        }
    }
}

/// One actor as a reader sees it, attributed the way an agent is: by its short
/// name alone. A human resolving to a born principal reads as the initials its
/// recorded profile yields, never its full name or code; a principal whose
/// profile yields no initials reads as its code, the only identifier it has.
/// Every other actor, including a human whose principal is unknown, reads as
/// its identity alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedActor<'a> {
    /// The actor itself.
    pub actor: &'a Actor,
    /// The code and short name of the born principal it resolves to.
    pub principal: Option<(PrincipalCode, Option<&'a str>)>,
}

impl fmt::Display for NamedActor<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.principal {
            Some((_, Some(short))) => formatter.write_str(short),
            Some((code, None)) => write!(formatter, "{code}"),
            None => write!(formatter, "{}", self.actor),
        }
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

impl PrincipalProfile {
    /// The short name a recorded name yields: the first letters of its first
    /// two words, uppercased. Absent when no name, or only whitespace, was
    /// recorded.
    #[must_use]
    pub fn initials(&self) -> Option<String> {
        self.name.as_deref().and_then(|name| {
            let badge = name
                .split_whitespace()
                .take(2)
                .filter_map(|word| word.chars().next())
                .flat_map(char::to_uppercase)
                .collect::<String>();
            (!badge.is_empty()).then_some(badge)
        })
    }

    /// Complete browser presentation derived only from this recorded profile.
    #[must_use]
    pub fn identity(&self) -> HumanIdentity {
        let initials = self.initials();
        let gravatar = self.email.as_ref().map(|email| {
            format!(
                "{:x}",
                Sha256::digest(email.trim().to_lowercase().as_bytes())
            )
            .parse()
            .expect("a SHA-256 lowercase hex spelling is a Gravatar hash")
        });
        HumanIdentity {
            name: self.name.clone(),
            email: self.email.clone(),
            initials,
            gravatar,
        }
    }
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

/// The digest the journal keeps of a capability issued to a principal; the
/// capability itself is handed off once and never journaled.
#[must_use]
pub fn capability_digest(capability: CanonicalUuid) -> Digest {
    Digest::compute(Domain::PrincipalCapability, capability.get().as_bytes())
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
    /// Digest of the capability this principal's admission issued, absent
    /// once revoked. The bootstrap principal has none: each resident
    /// generation mints its capability and publishes it locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<Digest>,
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
    /// Digest of the capability handed off to this principal, absent for the
    /// bootstrap birth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<Digest>,
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

/// One principal's issued capability was retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDeauthorized {
    /// Principal whose capability was retired.
    pub principal: CanonicalUuid,
    /// Digest of the retired capability.
    pub capability: Digest,
}

/// Read every born principal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRead {}
/// Read every principal's display identity and the connected principal's roles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDirectoryRead {}

/// Connected-readable principal identities and only the reader's current roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDirectoryAnswer {
    /// Every born principal's display identity, oldest birth first.
    pub principals: PrincipalDirectory,
    /// Roles held by the principal that issued this read.
    pub roles: BTreeSet<PrincipalRole>,
}

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

/// Retire the capability one principal's admission issued, ending every
/// session it opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDeauthorize {
    /// Principal whose capability is retired.
    pub principal: CanonicalUuid,
}

/// One born principal as a listing presents it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalListing {
    /// Current standing.
    pub head: PrincipalHead,
    /// Commit instant of the admission fact that bore this principal.
    pub admitted: crate::Instant,
}

/// Every born principal in birth order, as of the answering position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalsAnswer {
    /// Current principals, oldest birth first.
    pub principals: Vec<PrincipalListing>,
}

/// One principal's standing as an admission, profile edit, role change, or
/// capability revocation left it. Birth position is assigned at commit, so a
/// write answers without it; [`PrincipalsAnswer`] carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalStanding {
    /// Principal the write named or bore.
    pub principal: CanonicalUuid,
    /// Profile recorded after the write.
    pub profile: PrincipalProfile,
    /// Roles held after the write.
    pub roles: BTreeSet<PrincipalRole>,
    /// The capability an admission issued, handed off in that admission's
    /// reply alone. The command receipt and every fact keep only its digest,
    /// so a replayed admission never hands it off again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<CanonicalUuid>,
}

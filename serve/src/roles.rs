//! The principal role heads one resident generation decides authority by.
//!
//! The resident publishes the journal's current heads before serving and
//! after every principal change; the command door, principal capability
//! lookups, and stream subject admission all read the same book, so a role
//! change governs that principal's next request without reconnecting.

use std::sync::{Arc, RwLock};

use wire::principal::PrincipalRoles;

/// Shared, replaceable principal role heads. An unpublished book holds no
/// born principal, so it decides as [`PrincipalRoles`] does before the
/// bootstrap principal is born.
#[derive(Clone, Debug, Default)]
pub struct RoleBook(Arc<RwLock<PrincipalRoles>>);

/// The role book's lock was poisoned by a panicking writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleBookUnavailable;

impl std::fmt::Display for RoleBookUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("principal role book is unavailable")
    }
}

impl std::error::Error for RoleBookUnavailable {}

impl RoleBookUnavailable {
    /// The identity fault a request answers when no role book is readable.
    #[must_use]
    pub fn fault(self) -> wire::Fault {
        wire::Fault {
            class: wire::FaultClass::Protocol,
            reason: "serve::roles-unavailable".to_owned(),
            detail: self.to_string(),
        }
    }
}

impl RoleBook {
    /// A book already holding `roles`.
    #[must_use]
    pub fn new(roles: PrincipalRoles) -> Self {
        Self(Arc::new(RwLock::new(roles)))
    }

    /// Replace the heads every later request decides by.
    pub fn publish(&self, roles: PrincipalRoles) -> Result<(), RoleBookUnavailable> {
        *self.0.write().map_err(|_| RoleBookUnavailable)? = roles;
        Ok(())
    }

    /// The heads current for the next request.
    pub fn current(&self) -> Result<PrincipalRoles, RoleBookUnavailable> {
        self.0
            .read()
            .map(|roles| roles.clone())
            .map_err(|_| RoleBookUnavailable)
    }
}

/// A book in which the operator actor resolves to a bootstrap principal
/// holding the operator role.
#[cfg(test)]
pub(crate) fn operator_book() -> RoleBook {
    use wire::principal::{PrincipalHead, PrincipalProfile, PrincipalRole};
    RoleBook::new(PrincipalRoles::new([PrincipalHead {
        principal: "018f82d4-6f59-7d06-92e3-bce9131163b0"
            .parse()
            .expect("fixture principal"),
        born: wire::Seq::new(1).expect("fixture birth"),
        profile: PrincipalProfile::default(),
        roles: [PrincipalRole::Operator].into(),
        capability: None,
    }]))
}

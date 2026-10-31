//! Whether the lifecycle state this process would write belongs to the
//! repository user.
//!
//! Windows makes every object a process creates owned by its token's default
//! owner. An elevated administrator token commonly defaults to
//! Administrators, so an install from it would leave records, logs, and
//! discovery that the user's own unelevated CLI, app, and keeper cannot admit.
//! Only ownership decides: an administrator whose token defaults to its own
//! user is admitted, and an existing runtime root owned by anyone else is
//! refused rather than taken over.

/// The SIDs that decide who owns lifecycle state.
pub(super) struct Owners<'a> {
    /// The token's user.
    pub(super) user: &'a str,
    /// The owner of every object this process would create.
    pub(super) default_owner: &'a str,
    /// The owner of the existing runtime root; `None` when it is absent.
    pub(super) runtime: Option<&'a str>,
}

/// The principal that would own, or already owns, lifecycle state in place of
/// the token's user.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Mismatch<'a> {
    /// New state would belong to this default owner.
    DefaultOwner(&'a str),
    /// The existing runtime root belongs to this principal.
    Runtime(&'a str),
}

/// The first ownership divergence, or `None` when every object the lifecycle
/// writes or reuses belongs to the token's user.
pub(super) fn mismatch<'a>(owners: &Owners<'a>) -> Option<Mismatch<'a>> {
    if owners.default_owner != owners.user {
        return Some(Mismatch::DefaultOwner(owners.default_owner));
    }
    owners
        .runtime
        .filter(|runtime| *runtime != owners.user)
        .map(Mismatch::Runtime)
}

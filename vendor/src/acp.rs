//! ACP permission requests at the consent boundary.

use std::error::Error;
use std::fmt;

use wire::CanonicalUuid;
use wire::body::ConsentAsked;

/// Emit the wire request that consent policy can actuate.
///
/// This function validates shape only. It does not grant, deny, queue, budget,
/// or otherwise select a policy disposition.
///
/// # Errors
///
/// Returns [`PermissionRequestError`] when `operation` is empty or whitespace
/// only.
pub fn permission_request(
    request: CanonicalUuid,
    lane: CanonicalUuid,
    operation: impl Into<String>,
) -> Result<ConsentAsked, PermissionRequestError> {
    let operation = operation.into();
    if operation.trim().is_empty() {
        return Err(PermissionRequestError);
    }
    Ok(ConsentAsked {
        request,
        lane,
        operation,
    })
}

/// An ACP request without an operation to decide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionRequestError;

impl fmt::Display for PermissionRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ACP permission operation must not be empty")
    }
}

impl Error for PermissionRequestError {}

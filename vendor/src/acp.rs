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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_emits_only_the_typed_request_for_consent() {
        let request: CanonicalUuid = "018f7777-7777-7777-8777-777777777777"
            .parse()
            .expect("canonical request");
        let lane: CanonicalUuid = "018f8888-8888-7888-8888-888888888888"
            .parse()
            .expect("canonical lane");

        let asked = permission_request(request, lane, "filesystem.write").expect("valid request");
        assert_eq!(asked.request, request);
        assert_eq!(asked.lane, lane);
        assert_eq!(asked.operation, "filesystem.write");
        assert!(permission_request(request, lane, "  ").is_err());
    }
}

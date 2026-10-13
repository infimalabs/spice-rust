//! The operator inbox's personal heading.

/// The inbox heading for the current operator: their initials' inbox once
/// identity supplies initials, else "Your inbox".
pub(crate) fn heading(operator: &wire::OperatorIdentity) -> String {
    match operator.initials.as_deref() {
        Some(initials) => format!("{initials}'s inbox"),
        None => "Your inbox".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heading_names_resolved_initials_and_otherwise_addresses_the_reader() {
        let resolved = wire::OperatorIdentity {
            name: Some("C Anthony Risinger".to_owned()),
            initials: Some("CA".to_owned()),
            ..wire::OperatorIdentity::default()
        };
        assert_eq!(heading(&resolved), "CA's inbox");
        assert_eq!(heading(&wire::OperatorIdentity::default()), "Your inbox");
        let nameless = wire::OperatorIdentity {
            email: Some("operator@example.test".to_owned()),
            ..wire::OperatorIdentity::default()
        };
        assert_eq!(heading(&nameless), "Your inbox");
    }
}

//! The connected principal's personal inbox heading.

/// The inbox heading for the connected principal: their initials' inbox once
/// their recorded profile supplies initials, else "Your inbox".
pub(crate) fn heading(identity: &wire::HumanIdentity) -> String {
    match identity.initials.as_deref() {
        Some(initials) => format!("{initials}'s inbox"),
        None => "Your inbox".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heading_names_resolved_initials_and_otherwise_addresses_the_reader() {
        let resolved = wire::HumanIdentity {
            name: Some("C Anthony Risinger".to_owned()),
            initials: Some("CA".to_owned()),
            ..wire::HumanIdentity::default()
        };
        assert_eq!(heading(&resolved), "CA's inbox");
        assert_eq!(heading(&wire::HumanIdentity::default()), "Your inbox");
        let nameless = wire::HumanIdentity {
            email: Some("operator@example.test".to_owned()),
            ..wire::HumanIdentity::default()
        };
        assert_eq!(heading(&nameless), "Your inbox");
    }
}

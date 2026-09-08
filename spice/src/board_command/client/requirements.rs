use super::*;

fn journal_schema_requirements(
    written: &str,
) -> Result<wire::EnvironmentRequirements, Outcome<CommandAnswer>> {
    let value = written.parse::<u32>().map_err(|_| {
        refusal(
            "task::journal-schema-invalid",
            Some(written.to_owned()),
            "name one canonical positive decimal journal schema",
        )
    })?;
    if value.to_string() != written {
        return Err(refusal(
            "task::journal-schema-invalid",
            Some(written.to_owned()),
            "name one canonical positive decimal journal schema",
        ));
    }
    let requirement = wire::EnvironmentRequirement::journal_schema(value).map_err(|error| {
        refusal(
            "task::journal-schema-invalid",
            Some(written.to_owned()),
            &error.to_string(),
        )
    })?;
    Ok(wire::EnvironmentRequirements::one(requirement))
}

pub(super) fn environment_requirements(
    schema: Option<&str>,
    trust: Option<&str>,
) -> Result<wire::EnvironmentRequirements, Outcome<CommandAnswer>> {
    let mut requirements = Vec::new();
    if let Some(written) = schema {
        requirements.extend_from_slice(journal_schema_requirements(written)?.as_slice());
    }
    if let Some(written) = trust {
        let digest = written.parse::<wire::Digest>().map_err(|_| {
            refusal(
                "task::trust-grant-invalid",
                Some(written.to_owned()),
                "name one canonical lowercase 64-character executable-configuration digest",
            )
        })?;
        requirements.push(wire::EnvironmentRequirement::trust_grant(digest));
    }
    wire::EnvironmentRequirements::new(requirements).map_err(|error| {
        protocol_fault(
            "task::requirements-noncanonical",
            &format!("CLI assembled noncanonical environmental requirements: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_schema_cli_values_are_positive_canonical_decimals() {
        let maximum = u32::MAX.to_string();
        for admitted in ["1", "15", maximum.as_str()] {
            assert!(
                journal_schema_requirements(admitted).is_ok(),
                "{admitted:?} is canonical"
            );
        }
        for refused in ["", "0", "01", "+1", "-1", "1.0", "4294967296"] {
            assert!(
                journal_schema_requirements(refused).is_err(),
                "{refused:?} must not normalize"
            );
        }
    }

    #[test]
    fn trust_grant_cli_values_are_canonical_and_join_schema_in_fact_order() {
        let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let requirements =
            environment_requirements(Some("18"), Some(digest)).expect("canonical conjunction");
        assert!(matches!(
            requirements.as_slice(),
            [
                wire::EnvironmentRequirement::JournalSchema { .. },
                wire::EnvironmentRequirement::TrustGrant { .. }
            ]
        ));
        for refused in [
            "",
            "abc",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let outcome = environment_requirements(None, Some(refused))
                .expect_err("noncanonical digest refuses");
            assert!(matches!(
                outcome,
                Outcome::Refusal(ref refusal)
                    if refusal.reason() == "task::trust-grant-invalid"
            ));
        }
    }
}

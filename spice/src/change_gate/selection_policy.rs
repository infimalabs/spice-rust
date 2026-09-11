use super::*;

pub(super) struct PackagedPolicy {
    pub(super) selection: gate::ChangeSelectionPolicy,
    pub(super) test_engines: Vec<crate::config::TestEngine>,
}

#[cfg(test)]
pub(super) fn packaged_selection_policy(
    root: &Path,
    tracked_sources: &[String],
    environment: &Environment,
) -> Result<gate::ChangeSelectionPolicy, ChangeGateError> {
    packaged_policy(root, tracked_sources, environment).map(|policy| policy.selection)
}

pub(super) fn packaged_policy(
    root: &Path,
    tracked_sources: &[String],
    environment: &Environment,
) -> Result<PackagedPolicy, ChangeGateError> {
    const TRACKED_POLICY: &str = "spice-rust.toml";

    let path = root.join(TRACKED_POLICY);
    let tracked = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ChangeGateError::environment(
                "change-selection-policy-unavailable",
                format!("{}: {error}", path.display()),
            ));
        }
    };
    let document = tracked
        .as_deref()
        .map(|text| crate::config::Document::new(TRACKED_POLICY, text));
    let resolved = crate::config::resolve(crate::config::Inputs::new(document, None, &[]))
        .map_err(|error| ChangeGateError::refusal("change-selection-policy-refused", error))?;
    let policy = resolved.root().policy();
    let declared_rust_sources = gate::cargo_declared_compile_time_sources_at(root, environment)
        .map_err(|error| {
            ChangeGateError::refusal("change-selection-cargo-source-refused", error)
        })?;
    let selection = gate::ChangeSelectionPolicy::new(gate::ChangeSelectionPolicyInputs {
        tracked_sources,
        policy_authorities: &[TRACKED_POLICY.to_owned(), "baseline.toml".to_owned()],
        configured: tracked.is_some(),
        repository_documents: policy.repo_truth().docs(),
        file_shape_suffixes: policy.file_shape().source_suffixes(),
        complexity_suffixes: policy.languages().complexity(),
        magic_suffixes: policy.languages().magic(),
        package_roots: policy.package_roots(),
        declared_rust_sources: &declared_rust_sources,
        generated_paths: policy.generated_paths(),
        test_paths: policy.test_paths(),
    })
    .map_err(|error| ChangeGateError::refusal("change-selection-policy-refused", error))?;
    Ok(PackagedPolicy {
        selection,
        test_engines: policy.test_engines().to_vec(),
    })
}

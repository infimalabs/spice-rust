use super::*;

pub(super) fn validate_retirement(retirement: &Retirement) -> Result<(), InstallError> {
    validate_relative(&retirement.path)?;
    validate_relative(&retirement.destination)?;
    let expected_parent = Path::new(&retirement.parent.ancestor).join(&retirement.parent.missing);
    if Path::new(&retirement.destination).parent() != Some(expected_parent.as_path())
        || retirement
            .previous
            .as_ref()
            .is_some_and(|observed| !observed.image.validate())
        || retirement
            .observed
            .as_ref()
            .is_some_and(|observed| !observed.image.validate())
        || retirement
            .superseded
            .as_ref()
            .is_some_and(|superseded| !superseded.validate())
        || (retirement.observed.is_some() && retirement.superseded.is_some())
    {
        return Err(InstallError::InvalidRecord(
            "install retirement carries an invalid path or file image".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_operation(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
    content: GeneratedContentAuthority,
    common_git: &Path,
) -> Result<(), InstallError> {
    let (location, path, expected_mode) = asset_contract(operation.asset);
    validate_operation_contract(operation, location, path, expected_mode)?;
    validate_generated_content(operation, content, common_git)?;
    validate_introduced_parents(operation, introduced)
}

fn validate_operation_contract(
    operation: &Operation,
    location: Location,
    path: &str,
    expected_mode: u32,
) -> Result<(), InstallError> {
    if operation.location != location
        || operation.path != path
        || operation.generated.mode != expected_mode
    {
        return Err(InstallError::InvalidRecord(format!(
            "install operation {:?} does not match its fixed contract",
            operation.asset
        )));
    }
    validate_relative(&operation.path)?;
    validate_relative(&operation.destination)?;
    let expected_parent = Path::new(&operation.parent.ancestor).join(&operation.parent.missing);
    if Path::new(&operation.destination).parent() != Some(expected_parent.as_path()) {
        return Err(InstallError::InvalidRecord(
            "install destination does not match its bound parent".to_owned(),
        ));
    }
    if !operation.generated.validate()
        || operation
            .previous
            .as_ref()
            .is_some_and(|observed| !observed.image.validate())
        || operation
            .superseded
            .as_ref()
            .is_some_and(|superseded| !superseded.validate())
    {
        return Err(InstallError::InvalidRecord(
            "install operation carries an invalid file image".to_owned(),
        ));
    }
    Ok(())
}

fn validate_generated_content(
    operation: &Operation,
    content: GeneratedContentAuthority,
    common_git: &Path,
) -> Result<(), InstallError> {
    if operation.asset == Asset::Exclusions {
        return Err(InstallError::InvalidRecord(
            "install plan contains the retired version-5 exclusions asset".to_owned(),
        ));
    }
    if content == GeneratedContentAuthority::CurrentBuild
        && let Some(bytes) = fixed_asset_bytes(operation.asset, common_git)?
        && operation.generated != GeneratedImage::new(asset_contract(operation.asset).2, &bytes)
    {
        return Err(InstallError::InvalidRecord(format!(
            "{} differs from this build",
            asset_name(operation.asset)
        )));
    }
    match operation.asset {
        Asset::Binary
            if operation.generated.digest == Digest::compute(Domain::InstallArtifact, &[]) =>
        {
            return Err(InstallError::InvalidRecord(
                "explicit Spice executable is empty".to_owned(),
            ));
        }
        _ => {}
    }
    Ok(())
}

/// Whether one fixed generated asset belongs to another Spice build.
///
/// The binary is deliberately excluded: its selected bytes come from the
/// caller rather than an embedded build asset, and [`super::rebuilt_binary`]
/// compares that explicit input. A retired asset is rejected structurally
/// before this predicate can participate in refresh selection.
pub(super) fn fixed_content_differs_from_current_build(
    operation: &Operation,
    common_git: &Path,
) -> Result<bool, InstallError> {
    let Some(bytes) = fixed_asset_bytes(operation.asset, common_git)? else {
        return Ok(false);
    };
    let expected = GeneratedImage::new(asset_contract(operation.asset).2, &bytes);
    Ok(operation.generated != expected)
}

fn asset_name(asset: Asset) -> &'static str {
    match asset {
        Asset::Exclusions => "retired exclusions asset",
        Asset::GitPreCommit => "Git pre-commit hook",
        Asset::GitCommitMessage => "Git commit-msg hook",
        Asset::GitReferenceTransaction => "Git reference-transaction hook",
        Asset::HookEntry => "entry hook",
        Asset::HookStatic => "static hook",
        Asset::Skill => "generated skill",
        Asset::Binary => "Spice executable",
    }
}

fn validate_introduced_parents(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    let parents = super::path::relative_parents(&operation.destination)?;
    if operation.previous.is_some() && !operation.introduced_parents.is_empty() {
        return Err(InstallError::InvalidRecord(
            "an existing target cannot have introduced parent directories".to_owned(),
        ));
    }
    // A refresh is derived only over a verified-complete install, so every
    // parent it needs is already there and already owned by the plan it
    // supersedes. Claiming to introduce one would hand the same directory to
    // two reversals.
    if operation.superseded.is_some() && !operation.introduced_parents.is_empty() {
        return Err(InstallError::InvalidRecord(
            "a refreshed target cannot have introduced parent directories".to_owned(),
        ));
    }
    let mut minimum_parent = 0;
    for relative in &operation.introduced_parents {
        validate_relative(relative)?;
        let Some(offset) = parents[minimum_parent..]
            .iter()
            .position(|parent| parent == relative)
        else {
            return Err(InstallError::InvalidRecord(
                "introduced directories are not canonical target parents".to_owned(),
            ));
        };
        minimum_parent += offset + 1;
        if !introduced.insert((operation.location, relative.clone())) {
            return Err(InstallError::InvalidRecord(
                "introduced directory is duplicated across operations".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn asset_contract(asset: Asset) -> (Location, &'static str, u32) {
    match asset {
        Asset::Exclusions => (Location::CommonGit, EXCLUDE_PATH, DATA_MODE),
        Asset::GitPreCommit => (Location::CommonGit, GIT_PRE_COMMIT_PATH, EXECUTABLE_MODE),
        Asset::GitCommitMessage => (
            Location::CommonGit,
            GIT_COMMIT_MESSAGE_PATH,
            EXECUTABLE_MODE,
        ),
        Asset::GitReferenceTransaction => (
            Location::CommonGit,
            GIT_REFERENCE_TRANSACTION_PATH,
            EXECUTABLE_MODE,
        ),
        Asset::HookEntry => (Location::CommonGit, ENTRY_PATH, EXECUTABLE_MODE),
        Asset::HookStatic => (Location::CommonGit, STATIC_PATH, EXECUTABLE_MODE),
        Asset::Skill => (Location::CommonGit, SKILL_PATH, DATA_MODE),
        Asset::Binary => (Location::CommonGit, BINARY_PATH, EXECUTABLE_MODE),
    }
}

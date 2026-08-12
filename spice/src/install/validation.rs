use super::*;

pub(super) fn validate_operation(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    let (location, path, expected_mode) = asset_contract(operation.asset);
    validate_operation_contract(operation, location, path, expected_mode)?;
    validate_generated_content(operation, content)?;
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
) -> Result<(), InstallError> {
    match operation.asset {
        Asset::Exclusions => {
            return Err(InstallError::InvalidRecord(
                "install plan contains the retired version-5 exclusions asset".to_owned(),
            ));
        }
        Asset::HookEntry
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated
                    != GeneratedImage::new(
                        EXECUTABLE_MODE,
                        steer::hook::STAGES[0].bytes().as_bytes(),
                    ) =>
        {
            return Err(InstallError::InvalidRecord(
                "entry hook differs from this build".to_owned(),
            ));
        }
        Asset::GitPreCommit
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated
                    != GeneratedImage::new(EXECUTABLE_MODE, GIT_PRE_COMMIT.as_bytes()) =>
        {
            return Err(InstallError::InvalidRecord(
                "Git pre-commit hook differs from this build".to_owned(),
            ));
        }
        Asset::GitCommitMessage
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated
                    != GeneratedImage::new(EXECUTABLE_MODE, GIT_COMMIT_MESSAGE.as_bytes()) =>
        {
            return Err(InstallError::InvalidRecord(
                "Git commit-msg hook differs from this build".to_owned(),
            ));
        }
        Asset::GitReferenceTransaction
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated
                    != GeneratedImage::new(
                        EXECUTABLE_MODE,
                        GIT_REFERENCE_TRANSACTION.as_bytes(),
                    ) =>
        {
            return Err(InstallError::InvalidRecord(
                "Git reference-transaction hook differs from this build".to_owned(),
            ));
        }
        Asset::HookStatic
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated
                    != GeneratedImage::new(
                        EXECUTABLE_MODE,
                        steer::hook::STAGES[1].bytes().as_bytes(),
                    ) =>
        {
            return Err(InstallError::InvalidRecord(
                "static hook differs from this build".to_owned(),
            ));
        }
        Asset::Skill
            if content == GeneratedContentAuthority::CurrentBuild
                && operation.generated != GeneratedImage::new(DATA_MODE, SKILL.as_bytes()) =>
        {
            return Err(InstallError::InvalidRecord(
                "generated skill differs from this build".to_owned(),
            ));
        }
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
pub(super) fn fixed_content_differs_from_current_build(operation: &Operation) -> bool {
    let expected = match operation.asset {
        Asset::GitPreCommit => GeneratedImage::new(EXECUTABLE_MODE, GIT_PRE_COMMIT.as_bytes()),
        Asset::GitCommitMessage => {
            GeneratedImage::new(EXECUTABLE_MODE, GIT_COMMIT_MESSAGE.as_bytes())
        }
        Asset::GitReferenceTransaction => {
            GeneratedImage::new(EXECUTABLE_MODE, GIT_REFERENCE_TRANSACTION.as_bytes())
        }
        Asset::HookEntry => {
            GeneratedImage::new(EXECUTABLE_MODE, steer::hook::STAGES[0].bytes().as_bytes())
        }
        Asset::HookStatic => {
            GeneratedImage::new(EXECUTABLE_MODE, steer::hook::STAGES[1].bytes().as_bytes())
        }
        Asset::Skill => GeneratedImage::new(DATA_MODE, SKILL.as_bytes()),
        Asset::Exclusions | Asset::Binary => return false,
    };
    operation.generated != expected
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

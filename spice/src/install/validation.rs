use super::*;

pub(super) fn validate_plan(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    validate_plan_version(plan)?;
    validate_operation_inventory(plan, content)?;
    validate_retirement_inventory(plan)?;
    validate_plan_destinations(plan, content)
}

fn validate_plan_version(plan: &InstallPlan) -> Result<(), InstallError> {
    if plan.version != PLAN_VERSION {
        return Err(InstallError::InvalidRecord(format!(
            "install plan version {} is not {PLAN_VERSION}",
            plan.version
        )));
    }
    Ok(())
}

fn validate_operation_inventory(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    let expected = [
        Asset::GitPreCommit,
        Asset::GitCommitMessage,
        Asset::GitReferenceTransaction,
        Asset::HookEntry,
        Asset::HookStatic,
        Asset::HookZsh,
        Asset::HookBash,
        Asset::HookWrapperSpice,
        Asset::Skill,
    ];
    if plan.operations.len() < expected.len()
        || plan
            .operations
            .iter()
            .zip(expected)
            .any(|(operation, asset)| operation.asset != asset)
    {
        return Err(InstallError::InvalidRecord(
            "install plan operation order is not canonical".to_owned(),
        ));
    }
    validate_optional_tail(plan, &plan.operations[expected.len()..])?;
    if let Some(activation) = &plan.hook_activation {
        activation.validate(content == GeneratedContentAuthority::HistoricalReceipt)?;
    }
    Ok(())
}

/// The executable and its symbols are the only optional operations, and they
/// close the inventory: `[]`, `[Binary]`, or `[Binary, SymbolsInfo, SymbolsDwarf]`.
fn validate_optional_tail(plan: &InstallPlan, tail: &[Operation]) -> Result<(), InstallError> {
    let tail = tail
        .iter()
        .map(|operation| operation.asset)
        .collect::<Vec<_>>();
    let with_symbols = [Asset::Binary, Asset::SymbolsInfo, Asset::SymbolsDwarf];
    let admitted = tail.is_empty() || tail == [Asset::Binary] || tail == with_symbols;
    if !admitted {
        return Err(InstallError::InvalidRecord(
            "optional executable and symbols do not close the operation inventory".to_owned(),
        ));
    }
    if plan.symbols.is_some() != (tail == with_symbols) {
        return Err(InstallError::InvalidRecord(
            "symbol provenance and retained symbol operations must be planned together".to_owned(),
        ));
    }
    if let Some(symbols) = &plan.symbols {
        symbols.validate()?;
    }
    // The activation and the canvas generation pair with the executable that
    // carries the hooks' shell and links the canvas archive.
    let installed_executable = tail.first() == Some(&Asset::Binary);
    if plan.hook_activation.is_some() != installed_executable {
        return Err(InstallError::InvalidRecord(
            "hook activation and installed executable must be planned together".to_owned(),
        ));
    }
    if plan.app.is_some() != installed_executable {
        return Err(InstallError::InvalidRecord(
            "canvas generation and installed executable must be planned together".to_owned(),
        ));
    }
    Ok(())
}

fn validate_retirement_inventory(plan: &InstallPlan) -> Result<(), InstallError> {
    let fixed = &plan.retirements;
    let mut expected = vec![RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH];
    // Only a plan without symbols may discard a retained bundle.
    if plan.symbols.is_none()
        && fixed
            .get(expected.len()..expected.len() + symbols::DISCARD_PATHS.len())
            .is_some_and(|retirements| {
                retirements
                    .iter()
                    .zip(symbols::DISCARD_PATHS)
                    .all(|(retirement, path)| retirement.path == path)
            })
    {
        expected.extend(symbols::DISCARD_PATHS);
    }
    if fixed.len() != expected.len()
        || fixed
            .iter()
            .zip(expected)
            .any(|(retirement, path)| retirement.path != path)
    {
        return Err(InstallError::InvalidRecord(
            "install retirement inventory is not exact and ordered".to_owned(),
        ));
    }
    Ok(())
}

fn validate_plan_destinations(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    if plan.repository.worktree.is_some() || !plan.repository.common_git.is_absolute() {
        return Err(InstallError::InvalidRecord(
            "install repository identity is not common-directory scoped".to_owned(),
        ));
    }
    validate_relative(&plan.record.destination)?;
    let mut introduced = BTreeSet::new();
    let mut destinations = BTreeSet::new();
    for operation in &plan.operations {
        validate_operation(
            operation,
            &mut introduced,
            content,
            &plan.repository.common_git,
        )?;
        if !destinations.insert((operation.location, operation.destination.clone())) {
            return Err(InstallError::InvalidRecord(
                "install plan duplicates a physical destination".to_owned(),
            ));
        }
    }
    for retirement in &plan.retirements {
        validate_retirement(retirement, &mut introduced)?;
        if !destinations.insert((Location::CommonGit, retirement.destination.clone())) {
            return Err(InstallError::InvalidRecord(
                "install plan duplicates a retirement destination".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_retirement(
    retirement: &Retirement,
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    validate_relative(&retirement.path)?;
    validate_relative(&retirement.destination)?;
    validate_bound_parent(&retirement.destination, &retirement.parent)?;
    if let Some(parent) = &retirement.publication_parent {
        validate_bound_parent(&retirement.destination, parent)?;
    }
    let current_discard = symbols::DISCARD_PATHS.contains(&retirement.path.as_str());
    let invalid_discard = if current_discard {
        !retirement.discard || retirement.previous.is_some() || retirement.observed.is_some()
    } else {
        retirement.discard
    };
    if invalid_discard
        || retirement
            .previous
            .as_ref()
            .is_some_and(|previous| !previous.validate())
        || retirement
            .observed
            .as_ref()
            .is_some_and(|observed| !observed.validate())
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
    validate_introduced_parents(
        Location::CommonGit,
        &retirement.destination,
        retirement.previous.is_some(),
        &retirement.introduced_parents,
        introduced,
    )
}

pub(super) fn validate_operation(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
    content: GeneratedContentAuthority,
    common_git: &Path,
) -> Result<(), InstallError> {
    let (location, path, expected_mode) = asset_contract(operation.asset)?;
    validate_operation_contract(operation, location, &path, expected_mode)?;
    validate_generated_content(operation, content, common_git)?;
    validate_introduced_parents(
        operation.location,
        &operation.destination,
        operation.previous.is_some(),
        &operation.introduced_parents,
        introduced,
    )
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
    validate_bound_parent(&operation.destination, &operation.parent)?;
    if let Some(parent) = &operation.publication_parent {
        validate_bound_parent(&operation.destination, parent)?;
    }
    if !operation.generated.validate()
        || operation
            .previous
            .as_ref()
            .is_some_and(|previous| !previous.validate())
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
        && operation.generated != GeneratedImage::new(asset_contract(operation.asset)?.2, &bytes)
    {
        return Err(InstallError::InvalidRecord(format!(
            "{} differs from this build",
            asset_name(operation.asset)
        )));
    }
    let empty = Digest::compute(Domain::InstallArtifact, &[]);
    match operation.asset {
        Asset::Binary if operation.generated.digest == empty => {
            return Err(InstallError::InvalidRecord(
                "explicit Spice executable is empty".to_owned(),
            ));
        }
        Asset::SymbolsInfo | Asset::SymbolsDwarf if operation.generated.digest == empty => {
            return Err(InstallError::InvalidRecord(
                "retained symbol asset is empty".to_owned(),
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
    let expected = GeneratedImage::new(asset_contract(operation.asset)?.2, &bytes);
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
        Asset::HookZsh => "zsh startup hook",
        Asset::HookBash => "Bash startup hook",
        Asset::HookWrapperSpice => "Spice shell wrapper",
        Asset::Skill => "generated skill",
        Asset::Binary => "Spice executable",
        Asset::SymbolsInfo => "retained dSYM Info.plist",
        Asset::SymbolsDwarf => "retained dSYM DWARF image",
    }
}

fn validate_bound_parent(
    destination: &str,
    parent: &ParentObservation,
) -> Result<(), InstallError> {
    if !parent.ancestor.is_empty() {
        validate_relative(&parent.ancestor)?;
    }
    if !parent.missing.is_empty() {
        validate_relative(&parent.missing)?;
    }
    let expected = Path::new(&parent.ancestor).join(&parent.missing);
    if Path::new(destination).parent() != Some(expected.as_path()) {
        return Err(InstallError::InvalidRecord(
            "install destination does not match its bound parent".to_owned(),
        ));
    }
    Ok(())
}

fn validate_introduced_parents(
    location: Location,
    destination: &str,
    has_previous: bool,
    owned: &[String],
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    let parents = super::path::relative_parents(destination)?;
    if has_previous && !owned.is_empty() {
        return Err(InstallError::InvalidRecord(
            "an existing target cannot own introduced parent directories".to_owned(),
        ));
    }
    let mut minimum_parent = 0;
    for relative in owned {
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
        if !introduced.insert((location, relative.clone())) {
            return Err(InstallError::InvalidRecord(
                "introduced directory is duplicated across reversals".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn asset_contract(asset: Asset) -> Result<(Location, String, u32), InstallError> {
    if let Some(path) = symbols::path(asset) {
        return Ok((Location::CommonGit, path.to_owned(), DATA_MODE));
    }
    let contract = match asset {
        Asset::Exclusions => (Location::CommonGit, EXCLUDE_PATH.to_owned(), DATA_MODE),
        Asset::GitPreCommit => (
            Location::CommonGit,
            GIT_PRE_COMMIT_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::GitCommitMessage => (
            Location::CommonGit,
            GIT_COMMIT_MESSAGE_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::GitReferenceTransaction => (
            Location::CommonGit,
            GIT_REFERENCE_TRANSACTION_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::HookEntry => (Location::CommonGit, ENTRY_PATH.to_owned(), EXECUTABLE_MODE),
        Asset::HookStatic => (Location::CommonGit, STATIC_PATH.to_owned(), EXECUTABLE_MODE),
        Asset::HookZsh => (
            Location::CommonGit,
            ZSH_HOOK_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::HookBash => (
            Location::CommonGit,
            BASH_HOOK_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::HookWrapperSpice => (
            Location::CommonGit,
            WRAPPER_SPICE_PATH.to_owned(),
            EXECUTABLE_MODE,
        ),
        Asset::Skill => (Location::CommonGit, SKILL_PATH.to_owned(), DATA_MODE),
        Asset::Binary => (Location::CommonGit, BINARY_PATH.to_owned(), EXECUTABLE_MODE),
        Asset::SymbolsInfo | Asset::SymbolsDwarf => unreachable!("symbol assets returned above"),
    };
    Ok(contract)
}

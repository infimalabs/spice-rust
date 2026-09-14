use super::*;

pub(super) fn validate_plan(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    let migratable = validate_plan_version(plan, content)?;
    validate_operation_inventory(plan, content)?;
    validate_retirement_inventory(plan, migratable)?;
    validate_plan_destinations(plan, content)
}

fn validate_plan_version(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<bool, InstallError> {
    let historical = content == GeneratedContentAuthority::HistoricalReceipt
        && matches!(
            plan.version,
            MIGRATABLE_RECORD_VERSION
                | FORWARD_MIGRATABLE_RECORD_VERSION
                | INLINE_ROLLBACK_RECORD_VERSION
                | PREVIOUS_RECORD_VERSION
        );
    if plan.version != PLAN_VERSION && !historical {
        return Err(InstallError::InvalidRecord(format!(
            "install plan version {} is not {PLAN_VERSION}",
            plan.version
        )));
    }
    Ok(plan.version == MIGRATABLE_RECORD_VERSION)
}

fn validate_operation_inventory(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<(), InstallError> {
    let mut current = vec![
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
    current.extend(app_operation_inventory(plan)?);
    let previous = [
        Asset::GitPreCommit,
        Asset::GitCommitMessage,
        Asset::GitReferenceTransaction,
        Asset::HookEntry,
        Asset::HookStatic,
        Asset::Skill,
        Asset::Binary,
    ];
    let expected = if !matches!(plan.version, PLAN_VERSION | PREVIOUS_RECORD_VERSION) {
        let length = previous.len()
            - usize::from(
                plan.operations
                    .last()
                    .is_some_and(|operation| operation.asset != Asset::Binary),
            );
        &previous[..length]
    } else {
        &current[..]
    };
    if plan.operations.len() < expected.len()
        || plan
            .operations
            .iter()
            .zip(expected)
            .any(|(operation, asset)| operation.asset != *asset)
    {
        return Err(InstallError::InvalidRecord(
            "install plan operation order is not canonical".to_owned(),
        ));
    }
    validate_optional_tail(plan, &plan.operations[expected.len()..])?;
    validate_app_generations(plan)?;
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
    let admitted = tail.is_empty()
        || tail == [Asset::Binary]
        || (plan.version == PLAN_VERSION && tail == with_symbols);
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
    // Historical inventories place the executable inside the fixed prefix,
    // so the activation pairs with any installed executable, not the tail.
    let installed_executable = plan
        .operations
        .iter()
        .any(|operation| operation.asset == Asset::Binary);
    if plan.hook_activation.is_some() != installed_executable {
        return Err(InstallError::InvalidRecord(
            "hook activation and installed executable must be planned together".to_owned(),
        ));
    }
    Ok(())
}

fn app_operation_inventory(plan: &InstallPlan) -> Result<Vec<Asset>, InstallError> {
    let Some(_) = plan.app else {
        return Ok(Vec::new());
    };
    let files = plan
        .operations
        .iter()
        .filter(|operation| app::file(operation.asset).is_some())
        .count();
    if files == 0 || files % app::ASSETS.len() != 0 {
        return Err(InstallError::InvalidRecord(
            "install plan has an incomplete Canvas generation".to_owned(),
        ));
    }
    let mut expected = Vec::with_capacity(files + 1);
    for _ in 0..files / app::ASSETS.len() {
        expected.extend(app::ASSETS.into_iter().map(|(asset, _)| asset));
    }
    expected.push(Asset::CanvasCurrent);
    Ok(expected)
}

fn validate_app_generations(plan: &InstallPlan) -> Result<(), InstallError> {
    let Some(current) = &plan.app else {
        return Ok(());
    };
    let generations = plan
        .operations
        .iter()
        .filter_map(|operation| app::operation_generation(operation.asset, &operation.path))
        .collect::<Vec<_>>();
    let chunks = generations.chunks_exact(app::ASSETS.len());
    let groups = chunks
        .clone()
        .map(|chunk| chunk[0].clone())
        .collect::<Vec<_>>();
    let prior = &groups[..groups.len().saturating_sub(1)];
    let invalid = !chunks.remainder().is_empty()
        || chunks
            .clone()
            .any(|chunk| chunk.iter().any(|item| item != &chunk[0]))
        || groups.last() != Some(current)
        || prior.windows(2).any(|pair| pair[0] >= pair[1])
        || prior.contains(current);
    if invalid {
        return Err(InstallError::InvalidRecord(
            "install plan Canvas generations are not complete and current-last".to_owned(),
        ));
    }
    Ok(())
}

fn validate_retirement_inventory(plan: &InstallPlan, migratable: bool) -> Result<(), InstallError> {
    if migratable {
        if !plan.retirements.is_empty() || plan.rollback_archive.is_some() {
            return Err(InstallError::InvalidRecord(
                "version-7 install plan carries version-8 migration state".to_owned(),
            ));
        }
    } else {
        let mut expected = vec![RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH];
        if matches!(plan.version, PLAN_VERSION | PREVIOUS_RECORD_VERSION) {
            expected.push(RETIRED_FOLD_STORE_PATH);
        }
        // Only a current plan without symbols may discard a retained bundle.
        if plan.version == PLAN_VERSION
            && plan.symbols.is_none()
            && plan.retirements.len() == expected.len() + symbols::DISCARD_PATHS.len()
        {
            expected.extend(symbols::DISCARD_PATHS);
        }
        if plan.retirements.len() != expected.len()
            || plan
                .retirements
                .iter()
                .zip(expected)
                .any(|(retirement, path)| retirement.path != path)
        {
            return Err(InstallError::InvalidRecord(
                "install retirement inventory is not exact and ordered".to_owned(),
            ));
        }
    }
    if plan
        .retirements
        .iter()
        .any(|retirement| retirement.rollback.is_some())
        && plan.rollback_archive.is_none()
    {
        return Err(InstallError::InvalidRecord(
            "install retirement rollback witness has no rollback archive".to_owned(),
        ));
    }
    if plan.retirements.iter().any(|retirement| {
        matches!(
            (&retirement.rollback, &retirement.superseded),
            (Some(rollback), Some(superseded)) if rollback != superseded
        )
    }) {
        return Err(InstallError::InvalidRecord(
            RETIREMENT_WITNESS_MISMATCH.to_owned(),
        ));
    }
    if let Some(archive) = &plan.rollback_archive {
        archive.validate_structure()?;
        let manifest_version = matches!(plan.version, PLAN_VERSION | PREVIOUS_RECORD_VERSION);
        if manifest_version && archive.is_inline() {
            return Err(InstallError::InvalidRecord(
                "current install plan embeds rollback payload bytes".to_owned(),
            ));
        }
        if !manifest_version && !archive.is_inline() {
            return Err(InstallError::InvalidRecord(
                "historical install plan carries a current rollback manifest".to_owned(),
            ));
        }
    }
    if let Some(rollback) = plan.staged_rollback_state()? {
        plan.validate_rollback_state(&rollback)?;
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
            plan.app.as_ref(),
        )?;
        if !destinations.insert((operation.location, operation.destination.clone())) {
            return Err(InstallError::InvalidRecord(
                "install plan duplicates a physical destination".to_owned(),
            ));
        }
    }
    for retirement in &plan.retirements {
        validate_retirement(retirement, plan.version)?;
        if !destinations.insert((Location::CommonGit, retirement.destination.clone())) {
            return Err(InstallError::InvalidRecord(
                "install plan duplicates a retirement destination".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_retirement(retirement: &Retirement, plan_version: u16) -> Result<(), InstallError> {
    validate_relative(&retirement.path)?;
    validate_relative(&retirement.destination)?;
    let expected_parent = Path::new(&retirement.parent.ancestor).join(&retirement.parent.missing);
    let current_discard = (matches!(plan_version, PLAN_VERSION | PREVIOUS_RECORD_VERSION)
        && retirement.path == RETIRED_FOLD_STORE_PATH)
        || (plan_version == PLAN_VERSION
            && symbols::DISCARD_PATHS.contains(&retirement.path.as_str()));
    let invalid_discard = if current_discard {
        !retirement.discard
            || retirement.previous.is_some()
            || retirement.observed.is_some()
            || retirement.rollback.is_some()
    } else {
        retirement.discard
    };
    if Path::new(&retirement.destination).parent() != Some(expected_parent.as_path())
        || invalid_discard
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
    app: Option<&Digest>,
) -> Result<(), InstallError> {
    let generation = app::operation_generation(operation.asset, &operation.path);
    let (location, path, expected_mode) =
        asset_contract(operation.asset, generation.as_ref().or(app))?;
    validate_operation_contract(operation, location, &path, expected_mode)?;
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
        && operation.generated
            != GeneratedImage::new(asset_contract(operation.asset, None)?.2, &bytes)
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
    app: Option<&Digest>,
) -> Result<bool, InstallError> {
    let Some(bytes) = fixed_asset_bytes(operation.asset, common_git)? else {
        return Ok(false);
    };
    let expected = GeneratedImage::new(asset_contract(operation.asset, app)?.2, &bytes);
    Ok(operation.generated != expected)
}

fn asset_name(asset: Asset) -> &'static str {
    if app::file(asset).is_some() {
        return "Canvas generation asset";
    }
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
        Asset::CanvasCurrent => "Canvas generation pointer",
        Asset::Binary => "Spice executable",
        Asset::SymbolsInfo => "retained dSYM Info.plist",
        Asset::SymbolsDwarf => "retained dSYM DWARF image",
        _ => unreachable!("Canvas generation assets returned above"),
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

pub(super) fn asset_contract(
    asset: Asset,
    app: Option<&Digest>,
) -> Result<(Location, String, u32), InstallError> {
    if let Some(path) = app::operation_path(asset, app)? {
        return Ok((Location::CommonGit, path, DATA_MODE));
    }
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
        _ => unreachable!("Canvas assets returned above"),
    };
    Ok(contract)
}

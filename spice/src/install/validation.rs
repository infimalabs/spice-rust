use std::collections::BTreeMap;

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
    current.extend(app_operation_inventory(plan, content)?);
    let expected = &current[..];
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

fn app_operation_inventory(
    plan: &InstallPlan,
    content: GeneratedContentAuthority,
) -> Result<Vec<Asset>, InstallError> {
    if plan.app_predecessors.len() > RETAINED_APP_PREDECESSORS {
        return Err(InstallError::InvalidRecord(
            "app predecessor inventory exceeds its schema or bound".to_owned(),
        ));
    }
    let Some(current) = &plan.app else {
        if !plan.app_predecessors.is_empty() {
            return Err(InstallError::InvalidRecord(
                "app predecessors have no current generation".to_owned(),
            ));
        }
        return Ok(Vec::new());
    };
    let mut groups = BTreeMap::<&str, BTreeSet<Asset>>::new();
    for operation in &plan.operations {
        if app::file(operation.asset).is_none() {
            continue;
        }
        let generation =
            app::generation_spelling(operation.asset, &operation.path).ok_or_else(|| {
                InstallError::InvalidRecord("app asset has no canonical generation path".to_owned())
            })?;
        if !groups.contains_key(generation) {
            generation.parse::<Digest>().map_err(|_| {
                InstallError::InvalidRecord(
                    "app asset has an invalid generation identity".to_owned(),
                )
            })?;
        }
        if !groups
            .entry(generation)
            .or_default()
            .insert(operation.asset)
        {
            return Err(InstallError::InvalidRecord(
                "app generation repeats an asset".to_owned(),
            ));
        }
    }
    for (generation, assets) in &groups {
        // work.css was introduced without changing the receipt schema. Old
        // generations retain their original inventory, even inside a new plan;
        // only the current generation of a new plan must carry the new file.
        let historical = content == GeneratedContentAuthority::HistoricalReceipt
            || *generation != current.as_str();
        let missing = app::ASSETS
            .iter()
            .any(|(asset, _)| !assets.contains(asset) && !(historical && *asset == Asset::AppWork));
        let renderers = assets
            .iter()
            .filter(|asset| matches!(asset, Asset::AppRenderer(_)))
            .count();
        if missing || renderers > wire::manifest::renderer::MAX_FILES {
            return Err(InstallError::InvalidRecord(
                "app generation is incomplete or exceeds its bound".to_owned(),
            ));
        }
    }
    let current_assets = groups.remove(current.as_str()).ok_or_else(|| {
        InstallError::InvalidRecord("install plan omits its current app generation".to_owned())
    })?;
    // Recency lives only in the ordered inventory; operations keep one
    // canonical digest order, so the inventory must name exactly the retained
    // generations, each once.
    let listed = plan
        .app_predecessors
        .iter()
        .map(Digest::as_str)
        .collect::<BTreeSet<_>>();
    if listed.len() != plan.app_predecessors.len() || !listed.iter().eq(groups.keys()) {
        return Err(InstallError::InvalidRecord(
            "app predecessor inventory does not name exactly its retained generations".to_owned(),
        ));
    }
    let expected = groups
        .into_iter()
        .chain([(current.as_str(), current_assets)])
        .flat_map(|(generation, assets)| assets.into_iter().map(move |asset| (generation, asset)))
        .collect::<Vec<_>>();
    let operations = plan
        .operations
        .iter()
        .filter(|operation| app::file(operation.asset).is_some());
    if operations
        .zip(&expected)
        .any(|(operation, (generation, asset))| {
            operation.asset != *asset
                || app::generation_spelling(operation.asset, &operation.path) != Some(*generation)
        })
    {
        return Err(InstallError::InvalidRecord(
            "app generations are not canonical and current-last".to_owned(),
        ));
    }
    Ok(expected
        .into_iter()
        .map(|(_, asset)| asset)
        .chain([Asset::AppCurrent])
        .collect())
}

fn validate_retirement_inventory(plan: &InstallPlan) -> Result<(), InstallError> {
    let retired_generations = plan
        .retirements
        .iter()
        .rev()
        .take_while(|retirement| app::generation_file(&retirement.path).is_some())
        .count();
    let (fixed, retired) = plan
        .retirements
        .split_at(plan.retirements.len() - retired_generations);
    let mut expected = vec![
        RETIRED_XTASK_PATH,
        RETIRED_XTASK_GUARD_PATH,
        RETIRED_FOLD_STORE_PATH,
    ];
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
    validate_retired_generations(plan, retired)?;
    Ok(())
}

/// Out-of-bound generations retire as discards witnessed by their installed
/// images, in generation then asset order, each bound to withdraw exactly its
/// own generation directory and none that the plan still publishes.
fn validate_retired_generations(
    plan: &InstallPlan,
    retired: &[Retirement],
) -> Result<(), InstallError> {
    let mut prior = None;
    for retirement in retired {
        let (generation, file) =
            app::generation_file(&retirement.path).expect("the tail holds only generation files");
        let asset = app::asset_file(file).expect("a generation file names an asset");
        let published = plan
            .app
            .iter()
            .chain(&plan.app_predecessors)
            .any(|kept| kept.as_str() == generation);
        if published
            || generation.parse::<Digest>().is_err()
            || !retirement.discard
            || retirement.superseded.is_none()
            || retirement.parent.missing != generation
            || prior.is_some_and(|prior| prior >= (generation, asset))
        {
            return Err(InstallError::InvalidRecord(
                "retired app generations are not exact, ordered discards".to_owned(),
            ));
        }
        prior = Some((generation, asset));
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
        validate_retirement(retirement)?;
        if !destinations.insert((Location::CommonGit, retirement.destination.clone())) {
            return Err(InstallError::InvalidRecord(
                "install plan duplicates a retirement destination".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_retirement(retirement: &Retirement) -> Result<(), InstallError> {
    validate_relative(&retirement.path)?;
    validate_relative(&retirement.destination)?;
    let expected_parent = Path::new(&retirement.parent.ancestor).join(&retirement.parent.missing);
    let current_discard = retirement.path == RETIRED_FOLD_STORE_PATH
        || symbols::DISCARD_PATHS.contains(&retirement.path.as_str())
        || app::generation_file(&retirement.path).is_some();
    let invalid_discard = if current_discard {
        !retirement.discard || retirement.previous.is_some() || retirement.observed.is_some()
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
        return "app generation asset";
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
        Asset::AppCurrent => "app generation pointer",
        Asset::Binary => "Spice executable",
        Asset::SymbolsInfo => "retained dSYM Info.plist",
        Asset::SymbolsDwarf => "retained dSYM DWARF image",
        _ => unreachable!("app generation assets returned above"),
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
        _ => unreachable!("app assets returned above"),
    };
    Ok(contract)
}

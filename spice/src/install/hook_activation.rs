//! Repository-wide Git hook activation carried by one install plan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};

use super::{BINARY_PATH, InstallError, InstallPlan, Repository, installed_git_hooks};

const HOOK_KEY: &str = "core.hooksPath";

/// The system Git activation runs, never one found on `PATH`, inside
/// `worktree`.
#[cfg(not(windows))]
fn git_command(_operation: &'static str, worktree: &Path) -> Result<Command, InstallError> {
    let mut command = Command::new("/usr/bin/git");
    command.current_dir(worktree);
    Ok(command)
}

/// Git for Windows' command entry in the system Program Files folder, never
/// one found on `PATH`, inside `worktree`.
#[cfg(windows)]
fn git_command(operation: &'static str, worktree: &Path) -> Result<Command, InstallError> {
    let program_files = sill::program_files().map_err(|source| InstallError::GitLaunch {
        operation,
        worktree: worktree.to_path_buf(),
        source,
    })?;
    let mut command = Command::new(program_files.join(r"Git\cmd\git.exe"));
    command.current_dir(worktree);
    Ok(command)
}

/// Git configuration scope changed by one activation mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookConfigurationScope {
    /// The common repository configuration shared by every linked worktree.
    Local,
    /// One linked worktree's `config.worktree` override.
    Worktree,
}

impl HookConfigurationScope {
    const fn option(self) -> &'static str {
        match self {
            Self::Local => "--local",
            Self::Worktree => "--worktree",
        }
    }
}

/// One exact Git configuration replacement and its reversal value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookConfigurationMutation {
    worktree: String,
    scope: HookConfigurationScope,
    previous: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<Vec<String>>,
    generated: Vec<String>,
}

impl HookConfigurationMutation {
    /// Absolute worktree used to address this Git configuration scope.
    #[must_use]
    pub fn worktree(&self) -> &str {
        &self.worktree
    }

    /// Git configuration scope replaced by this mutation.
    #[must_use]
    pub const fn scope(&self) -> HookConfigurationScope {
        self.scope
    }

    /// Exact values restored by install reversal.
    #[must_use]
    pub fn previous(&self) -> &[String] {
        &self.previous
    }

    /// Exact live values this refresh replaces, when superseding an install.
    #[must_use]
    pub fn superseded(&self) -> Option<&[String]> {
        self.superseded.as_deref()
    }

    /// Exact values installed by activation.
    #[must_use]
    pub fn generated(&self) -> &[String] {
        &self.generated
    }

    /// Whether activation writes a value other than the one it expects in
    /// place, which a refresh over the same hooks path does not.
    #[must_use]
    pub fn changes(&self) -> bool {
        self.superseded.as_deref().unwrap_or(&self.previous) != self.generated
    }

    fn expects_in_place(&self, observed: &[String]) -> bool {
        self.superseded.as_deref().unwrap_or(&self.previous) == observed
    }
}

/// Every effective and worktree-local hook value observed during preflight.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookWorktreeObservation {
    worktree: String,
    effective: Vec<String>,
    worktree_local: Vec<String>,
}

impl HookWorktreeObservation {
    /// Absolute linked-worktree path.
    #[must_use]
    pub fn worktree(&self) -> &str {
        &self.worktree
    }

    /// Every effective `core.hooksPath` value reported across Git scopes.
    #[must_use]
    pub fn effective(&self) -> &[String] {
        &self.effective
    }

    /// Exact worktree-local override values, empty when none exist.
    #[must_use]
    pub fn worktree_local(&self) -> &[String] {
        &self.worktree_local
    }
}

/// Side-effect-free all-worktree hook activation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookActivationPlan {
    hooks_path: String,
    observations: Vec<HookWorktreeObservation>,
    mutations: Vec<HookConfigurationMutation>,
    /// Registered worktrees whose directory is gone, skipped by preflight:
    /// Git applies no hook configuration where there is no directory. This is
    /// preview evidence, never part of the recorded plan or its digest.
    #[serde(skip)]
    skipped: Vec<String>,
}

impl HookActivationPlan {
    /// Absolute installed hook directory selected for every worktree.
    #[must_use]
    pub fn hooks_path(&self) -> &str {
        &self.hooks_path
    }

    /// Complete preflight evidence in canonical worktree order.
    #[must_use]
    pub fn observations(&self) -> &[HookWorktreeObservation] {
        &self.observations
    }

    /// Exact configuration replacements, with the common path switch last.
    #[must_use]
    pub fn mutations(&self) -> &[HookConfigurationMutation] {
        &self.mutations
    }

    /// Registered worktrees whose directory is gone, which `git worktree
    /// prune` clears; preflight leaves them out.
    #[must_use]
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    pub(super) fn validate(&self, historical: bool) -> Result<(), InstallError> {
        valid_value(&self.hooks_path)?;
        if !Path::new(&self.hooks_path).is_absolute() {
            return Err(InstallError::InvalidRecord(
                "hook activation target is not absolute".to_owned(),
            ));
        }
        self.validate_observations()?;
        self.validate_mutations()?;
        self.validate_shape()?;
        self.validate_effective(historical)
    }

    fn validate_observations(&self) -> Result<(), InstallError> {
        if self.observations.is_empty()
            || !self
                .observations
                .windows(2)
                .all(|pair| pair[0].worktree < pair[1].worktree)
        {
            return Err(InstallError::InvalidRecord(
                "hook activation worktrees are not canonical and unique".to_owned(),
            ));
        }
        for observation in &self.observations {
            valid_worktree(&observation.worktree)?;
            valid_values(&observation.effective)?;
            valid_values(&observation.worktree_local)?;
        }
        Ok(())
    }

    fn validate_mutations(&self) -> Result<(), InstallError> {
        if self.mutations.is_empty() {
            return Err(InstallError::InvalidRecord(
                "hook activation omits the common configuration switch".to_owned(),
            ));
        }
        for mutation in &self.mutations {
            valid_worktree(&mutation.worktree)?;
            valid_values(&mutation.previous)?;
            valid_values(&mutation.generated)?;
            if let Some(superseded) = &mutation.superseded {
                valid_values(superseded)?;
            }
            if mutation.scope == HookConfigurationScope::Worktree && !mutation.generated.is_empty()
            {
                return Err(InstallError::InvalidRecord(
                    "worktree hook activation must clear its override".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), InstallError> {
        let common = self.mutations.last().expect("nonempty mutation inventory");
        if common.scope != HookConfigurationScope::Local
            || common.generated != [self.hooks_path.clone()]
            || self.mutations[..self.mutations.len() - 1]
                .iter()
                .any(|mutation| mutation.scope != HookConfigurationScope::Worktree)
        {
            return Err(InstallError::InvalidRecord(
                "hook activation does not place its common path switch last".to_owned(),
            ));
        }
        let worktree_mutations = &self.mutations[..self.mutations.len() - 1];
        if !worktree_mutations
            .windows(2)
            .all(|pair| pair[0].worktree < pair[1].worktree)
            || self.mutations.iter().any(|mutation| {
                !self
                    .observations
                    .iter()
                    .any(|observation| observation.worktree == mutation.worktree)
            })
        {
            return Err(InstallError::InvalidRecord(
                "hook activation mutations do not close over canonical worktree observations"
                    .to_owned(),
            ));
        }
        Ok(())
    }

    fn validate_effective(&self, historical: bool) -> Result<(), InstallError> {
        let common = self.mutations.last().expect("validated mutation inventory");
        if !historical
            && self.observations.iter().any(|observation| {
                !tolerated(&observation.effective, &self.hooks_path)
                    && !common.superseded.as_ref().is_some_and(|superseded| {
                        superseded.len() == 1 && tolerated(&observation.effective, &superseded[0])
                    })
            })
        {
            return Err(InstallError::InvalidRecord(
                "hook activation retains a foreign effective selection".to_owned(),
            ));
        }
        Ok(())
    }

    pub(super) fn rebase_main_worktree(
        &mut self,
        repository: &Repository,
    ) -> Result<(), InstallError> {
        let previous = self
            .observations
            .iter()
            .map(|observation| observation.worktree.clone())
            .collect::<BTreeSet<_>>();
        let current = list_worktrees(repository)?
            .live
            .iter()
            .map(|worktree| worktree_string(worktree))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let retired = previous.difference(&current).collect::<Vec<_>>();
        if retired.is_empty() {
            return Ok(());
        }
        let introduced = current.difference(&previous).collect::<Vec<_>>();
        let ([prior], [current]) = (retired.as_slice(), introduced.as_slice()) else {
            return Err(InstallError::InvalidRecord(
                "relocated hook worktree topology is ambiguous".to_owned(),
            ));
        };
        for observation in &mut self.observations {
            if observation.worktree == **prior {
                observation.worktree.clone_from(current);
            }
        }
        for mutation in &mut self.mutations {
            if mutation.worktree == **prior {
                mutation.worktree.clone_from(current);
            }
        }
        self.observations
            .sort_by(|left, right| left.worktree.cmp(&right.worktree));
        Ok(())
    }
}

pub(super) fn plan(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    migration: bool,
) -> Result<HookActivationPlan, InstallError> {
    let hooks = installed_git_hooks(repository);
    let hooks_path = super::git_hooks::git_spelling(&hooks)?;
    valid_value(&hooks_path).map_err(|_| InstallError::HookPathUnrepresentable {
        path: hooks,
        reason: "contains a line separator",
    })?;

    let Worktrees {
        live: mut worktrees,
        prunable,
    } = list_worktrees(repository)?;
    worktrees.sort();
    worktrees.dedup();
    let skipped = prunable
        .iter()
        .map(|worktree| worktree_string(worktree))
        .collect::<Result<Vec<_>, _>>()?;
    let prior = installed.and_then(InstallPlan::hook_activation);
    let mut observations = Vec::with_capacity(worktrees.len());
    let mut mutations = Vec::new();
    for worktree in worktrees {
        let effective = config_values(&worktree, None)?;
        if !tolerated(&effective, &hooks_path)
            && !prior.is_some_and(|activation| tolerated(&effective, &activation.hooks_path))
        {
            return Err(InstallError::ForeignHookPath {
                worktree,
                observed: effective,
            });
        }
        let worktree_local = worktree_values(&worktree)?;
        let spelling = worktree_string(&worktree)?;
        observations.push(HookWorktreeObservation {
            worktree: spelling.clone(),
            effective,
            worktree_local: worktree_local.clone(),
        });
        let inherited = prior.and_then(|activation| {
            activation.mutations.iter().find(|mutation| {
                mutation.scope == HookConfigurationScope::Worktree && mutation.worktree == spelling
            })
        });
        if !worktree_local.is_empty() || inherited.is_some() {
            mutations.push(HookConfigurationMutation {
                worktree: spelling,
                scope: HookConfigurationScope::Worktree,
                previous: inherited.map_or_else(
                    || worktree_local.clone(),
                    |mutation| {
                        if migration {
                            worktree_local.clone()
                        } else {
                            mutation.previous.clone()
                        }
                    },
                ),
                superseded: inherited.map(|_| worktree_local.clone()),
                generated: Vec::new(),
            });
        }
    }
    let main = observations
        .first()
        .expect("Git returned at least one worktree")
        .worktree
        .clone();
    let local = config_values(Path::new(&main), Some(HookConfigurationScope::Local))?;
    let inherited = prior.and_then(|activation| activation.mutations.last());
    mutations.push(HookConfigurationMutation {
        worktree: main,
        scope: HookConfigurationScope::Local,
        previous: inherited.map_or_else(
            || local.clone(),
            |mutation| {
                if migration {
                    local.clone()
                } else {
                    mutation.previous.clone()
                }
            },
        ),
        superseded: inherited.map(|_| local.clone()),
        generated: vec![hooks_path.clone()],
    });
    let activation = HookActivationPlan {
        hooks_path,
        observations,
        mutations,
        skipped,
    };
    activation.validate(false)?;
    Ok(activation)
}

pub(super) fn topology_changed(
    repository: &Repository,
    installed: &InstallPlan,
) -> Result<bool, InstallError> {
    let Some(prior) = installed.hook_activation() else {
        return Ok(false);
    };
    let current = plan(repository, Some(installed), false)?;
    let same_worktrees = current
        .observations
        .iter()
        .map(|observation| observation.worktree.as_str())
        .eq(prior
            .observations
            .iter()
            .map(|observation| observation.worktree.as_str()));
    let same_mutations = current
        .mutations
        .iter()
        .zip(&prior.mutations)
        .all(|(current, prior)| {
            current.worktree == prior.worktree
                && current.scope == prior.scope
                && current.previous == prior.previous
                && current.generated == prior.generated
                && current.superseded.as_deref() == Some(prior.generated.as_slice())
        })
        && current.mutations.len() == prior.mutations.len();
    Ok(!same_worktrees || !same_mutations)
}

pub(super) fn apply(repository: &Repository, install: &InstallPlan) -> Result<(), InstallError> {
    let activation = install
        .hook_activation()
        .ok_or_else(|| InstallError::InvalidRecord("install has no hook activation".to_owned()))?;
    verify_assets(repository, install)?;
    for mutation in &activation.mutations {
        replace(mutation, false)?;
    }
    verify(activation, true)
}

pub(super) fn restore(activation: &HookActivationPlan) -> Result<(), InstallError> {
    for mutation in activation.mutations.iter().rev() {
        replace(mutation, true)?;
    }
    verify(activation, false)
}

pub(super) fn verify(activation: &HookActivationPlan, generated: bool) -> Result<(), InstallError> {
    for mutation in &activation.mutations {
        let observed = mutation_values(mutation)?;
        let expected = if generated {
            &mutation.generated
        } else {
            &mutation.previous
        };
        if &observed != expected {
            return Err(InstallError::GitConfigurationDrift {
                worktree: PathBuf::from(&mutation.worktree),
                scope: mutation.scope,
                observed,
            });
        }
    }
    Ok(())
}

pub(super) fn verify_transition(activation: &HookActivationPlan) -> Result<(), InstallError> {
    for mutation in &activation.mutations {
        let observed = mutation_values(mutation)?;
        if observed != mutation.previous
            && observed != mutation.generated
            && !transition_state(&observed, &mutation.previous)
            && !transition_state(&observed, &mutation.generated)
        {
            return Err(InstallError::GitConfigurationDrift {
                worktree: PathBuf::from(&mutation.worktree),
                scope: mutation.scope,
                observed,
            });
        }
    }
    Ok(())
}

pub(super) fn verify_pristine(activation: &HookActivationPlan) -> Result<(), InstallError> {
    for mutation in &activation.mutations {
        let observed = mutation_values(mutation)?;
        if !mutation.expects_in_place(&observed) {
            return Err(InstallError::GitConfigurationDrift {
                worktree: PathBuf::from(&mutation.worktree),
                scope: mutation.scope,
                observed,
            });
        }
    }
    Ok(())
}

pub(super) fn verify_refreshable(activation: &HookActivationPlan) -> Result<(), InstallError> {
    for mutation in &activation.mutations {
        let observed = mutation_values(mutation)?;
        if observed != mutation.generated && !tolerated(&observed, &activation.hooks_path) {
            return Err(InstallError::GitConfigurationDrift {
                worktree: PathBuf::from(&mutation.worktree),
                scope: mutation.scope,
                observed,
            });
        }
    }
    Ok(())
}

fn replace(mutation: &HookConfigurationMutation, restoring: bool) -> Result<(), InstallError> {
    let observed = mutation_values(mutation)?;
    let generated = if restoring {
        &mutation.previous
    } else {
        &mutation.generated
    };
    let expected = if restoring {
        &mutation.generated
    } else if let Some(superseded) = &mutation.superseded {
        superseded
    } else {
        &mutation.previous
    };
    if &observed == generated {
        return Ok(());
    }
    if &observed != expected && !transition_state(&observed, generated) {
        return Err(InstallError::GitConfigurationDrift {
            worktree: PathBuf::from(&mutation.worktree),
            scope: mutation.scope,
            observed,
        });
    }
    set_values(mutation, generated)?;
    let observed = mutation_values(mutation)?;
    if &observed != generated {
        return Err(InstallError::GitConfigurationDrift {
            worktree: PathBuf::from(&mutation.worktree),
            scope: mutation.scope,
            observed,
        });
    }
    Ok(())
}

fn set_values(mutation: &HookConfigurationMutation, values: &[String]) -> Result<(), InstallError> {
    let worktree = Path::new(&mutation.worktree);
    let scope = mutation.scope.option();
    let Some((first, rest)) = values.split_first() else {
        let unset = git(
            worktree,
            ["config", scope, "--unset-all", HOOK_KEY],
            "clear Git hook selection",
        );
        return match unset {
            Ok(_)
            | Err(InstallError::GitExit {
                code: Some(1 | 5), ..
            }) => Ok(()),
            Err(error) => Err(error),
        };
    };
    git(
        worktree,
        ["config", scope, "--replace-all", HOOK_KEY, first],
        "replace Git hook selection",
    )?;
    for value in rest {
        git(
            worktree,
            ["config", scope, "--add", HOOK_KEY, value],
            "write Git hook selection",
        )?;
    }
    Ok(())
}

fn transition_state(observed: &[String], target: &[String]) -> bool {
    !target.is_empty() && observed.len() < target.len() && target.starts_with(observed)
}

fn verify_assets(repository: &Repository, plan: &InstallPlan) -> Result<(), InstallError> {
    if !plan
        .operations()
        .iter()
        .any(|operation| operation.path() == BINARY_PATH)
    {
        return Err(InstallError::InvalidRecord(
            "hook activation requires an installed Spice executable".to_owned(),
        ));
    }
    for operation in plan.operations() {
        let observed = super::observe(&operation.destination(repository))?;
        if !super::matches_generated(observed.as_ref(), operation) {
            return Err(super::drift(operation, observed));
        }
    }
    Ok(())
}

fn mutation_values(mutation: &HookConfigurationMutation) -> Result<Vec<String>, InstallError> {
    config_values(Path::new(&mutation.worktree), Some(mutation.scope))
}

/// Registered worktrees, split by whether their directory still exists.
struct Worktrees {
    live: Vec<PathBuf>,
    /// Rows Git marks `prunable`: the directory they name is gone.
    prunable: Vec<PathBuf>,
}

fn list_worktrees(repository: &Repository) -> Result<Worktrees, InstallError> {
    let output = git(
        repository.worktree_root(),
        ["worktree", "list", "--porcelain", "-z"],
        "list Git worktrees",
    )?;
    parse_worktrees(&output.stdout)
}

/// Parse `git worktree list --porcelain -z`: NUL-terminated attributes, each
/// row opened by its `worktree` attribute.
fn parse_worktrees(stdout: &[u8]) -> Result<Worktrees, InstallError> {
    let mut rows: Vec<(PathBuf, bool)> = Vec::new();
    for field in stdout.split(|byte| *byte == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            let path = std::str::from_utf8(path)
                .map_err(|_| InstallError::GitNonUtf8("list Git worktrees"))?;
            rows.push((PathBuf::from(path), false));
        } else if (field == b"prunable" || field.starts_with(b"prunable "))
            && let Some((_, prunable)) = rows.last_mut()
        {
            *prunable = true;
        }
    }
    let (prunable, live): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, prunable)| *prunable);
    if live.is_empty() {
        return Err(InstallError::InvalidRecord(
            "Git returned no worktrees for hook activation".to_owned(),
        ));
    }
    Ok(Worktrees {
        live: live.into_iter().map(|(path, _)| path).collect(),
        prunable: prunable.into_iter().map(|(path, _)| path).collect(),
    })
}

fn worktree_values(worktree: &Path) -> Result<Vec<String>, InstallError> {
    if !worktree_config_enabled(worktree)? {
        return Ok(Vec::new());
    }
    match git(
        worktree,
        ["config", "--worktree", "--get-all", HOOK_KEY],
        "read worktree-local Git hooks",
    ) {
        Ok(output) => values("read worktree-local Git hooks", &output.stdout),
        Err(InstallError::GitExit { code: Some(1), .. }) => Ok(Vec::new()),
        Err(InstallError::GitExit { diagnostic, .. })
            if diagnostic.contains("extension worktreeConfig is enabled") =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

fn worktree_config_enabled(worktree: &Path) -> Result<bool, InstallError> {
    let output = git_command("read Git worktree configuration mode", worktree)?
        .args(["config", "--local", "--get", "extensions.worktreeConfig"])
        .output()
        .map_err(|source| InstallError::GitLaunch {
            operation: "read Git worktree configuration mode",
            worktree: worktree.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        let value = std::str::from_utf8(&output.stdout)
            .map_err(|_| InstallError::GitNonUtf8("read Git worktree configuration mode"))?;
        Ok(value.trim_ascii_end().eq_ignore_ascii_case("true"))
    } else if output.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(InstallError::GitExit {
            operation: "read Git worktree configuration mode",
            code: output.status.code(),
            diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn config_values(
    worktree: &Path,
    scope: Option<HookConfigurationScope>,
) -> Result<Vec<String>, InstallError> {
    let mut command = git_command("read Git hook selection", worktree)?;
    command.arg("config");
    if let Some(scope) = scope {
        command.arg(scope.option());
    }
    let output = command
        .args(["--get-all", HOOK_KEY])
        .output()
        .map_err(|source| InstallError::GitLaunch {
            operation: "read Git hook selection",
            worktree: worktree.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        values("read Git hook selection", &output.stdout)
    } else if output.status.code() == Some(1) {
        Ok(Vec::new())
    } else {
        Err(InstallError::GitExit {
            operation: "read Git hook selection",
            code: output.status.code(),
            diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn values(operation: &'static str, bytes: &[u8]) -> Result<Vec<String>, InstallError> {
    let text = std::str::from_utf8(bytes).map_err(|_| InstallError::GitNonUtf8(operation))?;
    Ok(text.lines().map(str::to_owned).collect())
}

fn tolerated(values: &[String], hooks_path: &str) -> bool {
    let Some(first) = values.first() else {
        return true;
    };
    (first == ".githooks" || first == hooks_path) && values.iter().all(|value| value == first)
}

fn worktree_string(path: &Path) -> Result<String, InstallError> {
    let text = path
        .to_str()
        .ok_or(InstallError::GitNonUtf8("name Git worktree"))?;
    valid_value(text)?;
    if !path.is_absolute() {
        return Err(InstallError::InvalidRecord(
            "Git worktree path is not absolute".to_owned(),
        ));
    }
    Ok(text.to_owned())
}

fn valid_worktree(path: &str) -> Result<(), InstallError> {
    valid_value(path)?;
    if Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(InstallError::InvalidRecord(
            "hook activation worktree is not absolute".to_owned(),
        ))
    }
}

fn valid_values(values: &[String]) -> Result<(), InstallError> {
    for value in values {
        valid_value(value)?;
    }
    Ok(())
}

fn valid_value(value: &str) -> Result<(), InstallError> {
    if value.is_empty() || value.contains(['\0', '\r', '\n']) {
        Err(InstallError::InvalidRecord(
            "hook activation carries an invalid Git configuration value".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn git<const N: usize>(
    root: &Path,
    arguments: [&str; N],
    operation: &'static str,
) -> Result<Output, InstallError> {
    let output = git_command(operation, root)?
        .args(arguments)
        .output()
        .map_err(|source| InstallError::GitLaunch {
            operation,
            worktree: root.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(InstallError::GitExit {
            operation,
            code: output.status.code(),
            diagnostic: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_whose_directory_is_gone_leaves_the_hook_walk() {
        let porcelain = concat!(
            "worktree /repo\0HEAD 1111\0branch refs/heads/main\0\0",
            "worktree /gone\0HEAD 1111\0branch refs/heads/gone\0",
            "prunable gitdir file points to non-existent location\0\0",
            "worktree /kept\0HEAD 1111\0detached\0locked prunable\nreason\0\0",
            "worktree /bare-reason\0HEAD 1111\0detached\0prunable\0\0",
        );

        let worktrees = parse_worktrees(porcelain.as_bytes()).expect("porcelain parses");

        assert_eq!(
            worktrees.live,
            [PathBuf::from("/repo"), PathBuf::from("/kept")]
        );
        assert_eq!(
            worktrees.prunable,
            [PathBuf::from("/gone"), PathBuf::from("/bare-reason")]
        );
    }
}

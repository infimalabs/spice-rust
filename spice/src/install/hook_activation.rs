//! Repository-wide Git hook activation carried by one install plan.
//!
//! Worktree enumeration and the single-file `--local` and `--worktree` hook
//! scopes are read and rewritten in process, addressed exactly as
//! `git worktree list` and `git config --local`/`--worktree` address them.
//! Only the effective selection across every scope runs Git. That read spans
//! the installation, system, global, and command scopes under Git's include
//! rules and uses the same concrete platform executable as gate acquisition:
//! selected from `PATH` before the warned platform default.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{BINARY_PATH, InstallError, InstallPlan, Repository, installed_git_hooks};

const HOOK_KEY: &str = "core.hooksPath";

/// The same platform Git selection used by gate acquisition, inside `worktree`.
fn git_command(
    operation: &'static str,
    worktree: &Path,
    search_path: Option<&OsStr>,
) -> Result<Command, InstallError> {
    let selected =
        gate::git::program(worktree, search_path).map_err(|error| InstallError::GitLaunch {
            operation,
            worktree: worktree.to_path_buf(),
            source: io::Error::other(error),
        })?;
    selected.announce_secondary();
    let mut command = Command::new(selected.path());
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
    const fn source(self) -> gix_config::Source {
        match self {
            Self::Local => gix_config::Source::Local,
            Self::Worktree => gix_config::Source::Worktree,
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
            .into_iter()
            .map(worktree_string)
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
    search_path: Option<&OsStr>,
) -> Result<HookActivationPlan, InstallError> {
    let hooks = installed_git_hooks(repository);
    let hooks_path = super::git_hooks::git_spelling(&hooks)?;
    valid_value(&hooks_path).map_err(|_| InstallError::HookPathUnrepresentable {
        path: hooks,
        reason: "contains a line separator",
    })?;

    let Worktrees { live, prunable } = list_worktrees(repository)?;
    let worktrees = ordered_worktrees(live)?;
    let skipped = prunable
        .into_iter()
        .map(worktree_string)
        .collect::<Result<Vec<_>, _>>()?;
    let prior = installed.and_then(InstallPlan::hook_activation);
    let mut observations = Vec::with_capacity(worktrees.len());
    let mut mutations = Vec::new();
    for spelling in worktrees {
        let worktree = Path::new(&spelling);
        let effective = effective_values(worktree, search_path)?;
        if !tolerated(&effective, &hooks_path)
            && !prior.is_some_and(|activation| tolerated(&effective, &activation.hooks_path))
        {
            return Err(InstallError::ForeignHookPath {
                worktree: worktree.to_path_buf(),
                observed: effective,
            });
        }
        let worktree_local = worktree_values(worktree)?;
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
    let local = scope_values(Path::new(&main), HookConfigurationScope::Local)?;
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
    search_path: Option<&OsStr>,
) -> Result<bool, InstallError> {
    let Some(prior) = installed.hook_activation() else {
        return Ok(false);
    };
    let current = plan(repository, Some(installed), false, search_path)?;
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

/// Rewrite one scope's file to exactly `values` under Git's own `.lock`
/// protocol, leaving every other entry in place. A file already holding
/// `values` is not rewritten, so clearing an absent selection creates nothing.
fn set_values(mutation: &HookConfigurationMutation, values: &[String]) -> Result<(), InstallError> {
    let path = scope_file(Path::new(&mutation.worktree), mutation.scope)?;
    let mut lock = gix::lock::File::acquire_to_update_resource(
        &path,
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|error| InstallError::io("lock Git configuration", &path, io::Error::other(error)))?;
    let mut file = config_file(&path, mutation.scope)?;
    if hook_values(&file)? == values {
        return Ok(());
    }
    replace_hook_values(&mut file, values)
        .map_err(|error| InstallError::io("write Git hook selection", &path, error))?;
    file.write_to(&mut lock)
        .map_err(|source| InstallError::io("write Git configuration", &path, source))?;
    lock.commit()
        .map_err(|error| InstallError::io("replace Git configuration", &path, error.error))?;
    Ok(())
}

/// Remove every `core.hooksPath` entry, then append `values` in order to the
/// last `[core]` section, which is created when none exists.
fn replace_hook_values(file: &mut gix_config::File, values: &[String]) -> io::Result<()> {
    let sections = file
        .sections_and_ids_by_name("core")
        .into_iter()
        .flatten()
        .filter(|(section, _)| section.header().subsection_name().is_none())
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    let (_, key) = HOOK_KEY
        .split_once('.')
        .expect("hook key names its section");
    for id in sections {
        let mut section = file
            .section_mut_by_id(id)
            .expect("a listed section is present");
        while section.remove(key).is_some() {}
    }
    if values.is_empty() {
        return Ok(());
    }
    let mut section = file
        .section_mut_or_create_new("core", None)
        .map_err(io::Error::other)?;
    for value in values {
        section
            .push(key, Some(gix::bstr::BStr::new(value)))
            .map_err(io::Error::other)?;
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
    scope_values(Path::new(&mutation.worktree), mutation.scope)
}

/// Registered worktrees, split by whether their directory still exists.
struct Worktrees {
    live: Vec<PathBuf>,
    /// Rows Git marks `prunable`: the directory they name is gone.
    prunable: Vec<PathBuf>,
}

/// The rows `git worktree list` reports, named as it names them: the main
/// worktree is the real path of the common directory without a trailing
/// `.git`, and each linked worktree is the checkout its `gitdir` file names.
/// A registration whose `gitdir` file is missing or unreadable is left out,
/// and an unlocked one whose checkout is gone is prunable.
fn list_worktrees(repository: &Repository) -> Result<Worktrees, InstallError> {
    let git = open(repository.worktree_root())?;
    let common = std::fs::canonicalize(git.common_dir())
        .map_err(|source| InstallError::io("list Git worktrees", git.common_dir(), source))?;
    let main = match (common.file_name(), common.parent()) {
        (Some(name), Some(parent)) if name == OsStr::new(".git") => parent.to_path_buf(),
        _ => common,
    };
    let mut worktrees = Worktrees {
        live: vec![spelled(&main)?],
        prunable: Vec::new(),
    };
    let linked = git.worktrees().map_err(|source| {
        InstallError::io(
            "list Git worktrees",
            git.common_dir().join("worktrees"),
            source,
        )
    })?;
    for proxy in linked {
        let Ok(base) = proxy.base() else {
            continue;
        };
        // Git resolves a relative `gitdir` to its real path.
        let base = if base
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            std::fs::canonicalize(&base).unwrap_or(base)
        } else {
            base
        };
        let row = spelled(&base)?;
        if proxy.is_locked() || base.join(".git").exists() {
            worktrees.live.push(row);
        } else {
            worktrees.prunable.push(row);
        }
    }
    Ok(worktrees)
}

/// `path` spelled as Git prints worktree paths.
fn spelled(path: &Path) -> Result<PathBuf, InstallError> {
    super::git_hooks::git_spelling(path)
        .map(PathBuf::from)
        .map_err(|_| {
            InstallError::InvalidRecord(format!(
                "Git worktree {} is not a UTF-8 local path",
                path.display()
            ))
        })
}

fn open(worktree: &Path) -> Result<gix::Repository, InstallError> {
    gix::open_opts(worktree, gix::open::Options::isolated())
        .map_err(|error| InstallError::io("open Git repository", worktree, io::Error::other(error)))
}

/// The file `git config --local` or `--worktree` reads and writes from
/// `worktree`: the common `config`, or that worktree's `config.worktree`.
fn scope_file(worktree: &Path, scope: HookConfigurationScope) -> Result<PathBuf, InstallError> {
    let git = open(worktree)?;
    Ok(match scope {
        HookConfigurationScope::Local => git.common_dir().join("config"),
        HookConfigurationScope::Worktree => git.git_dir().join("config.worktree"),
    })
}

/// One scope's configuration file without following its includes, as Git
/// reads a single scope; an absent file holds nothing.
fn config_file(
    path: &Path,
    scope: HookConfigurationScope,
) -> Result<gix_config::File, InstallError> {
    use gix_config::file::init::from_paths::Error;
    match gix_config::File::from_path_no_includes(path.to_path_buf(), scope.source()) {
        Ok(file) => Ok(file),
        Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(
            gix_config::File::new(gix_config::file::Metadata::from(scope.source()).at(path)),
        ),
        Err(Error::Io { source, path }) => {
            Err(InstallError::io("read Git configuration", path, source))
        }
        Err(Error::Init(error)) => Err(InstallError::io(
            "parse Git configuration",
            path,
            io::Error::other(error),
        )),
    }
}

fn hook_values(file: &gix_config::File) -> Result<Vec<String>, InstallError> {
    file.strings(HOOK_KEY)
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            String::from_utf8(value.into())
                .map_err(|_| InstallError::GitNonUtf8("read Git hook selection"))
        })
        .collect()
}

/// Exact values one scope's own file holds.
fn scope_values(
    worktree: &Path,
    scope: HookConfigurationScope,
) -> Result<Vec<String>, InstallError> {
    hook_values(&config_file(&scope_file(worktree, scope)?, scope)?)
}

/// Worktree-local values, which Git consults only once the common
/// configuration enables `extensions.worktreeConfig`.
fn worktree_values(worktree: &Path) -> Result<Vec<String>, InstallError> {
    let local = config_file(
        &scope_file(worktree, HookConfigurationScope::Local)?,
        HookConfigurationScope::Local,
    )?;
    let enabled = local
        .string("extensions.worktreeConfig")
        .is_some_and(|value| value.eq_ignore_ascii_case(b"true"));
    if enabled {
        scope_values(worktree, HookConfigurationScope::Worktree)
    } else {
        Ok(Vec::new())
    }
}

/// Every value Git reports across its scopes from `worktree`, in its order.
fn effective_values(
    worktree: &Path,
    search_path: Option<&OsStr>,
) -> Result<Vec<String>, InstallError> {
    let output = git_command("read Git hook selection", worktree, search_path)?
        .args(["config", "--get-all", HOOK_KEY])
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

/// Receipt keys use string order, which can differ from `Path` component order.
fn ordered_worktrees(paths: Vec<PathBuf>) -> Result<Vec<String>, InstallError> {
    let mut worktrees = paths
        .into_iter()
        .map(worktree_string)
        .collect::<Result<Vec<_>, _>>()?;
    worktrees.sort();
    worktrees.dedup();
    Ok(worktrees)
}

fn worktree_string(path: PathBuf) -> Result<String, InstallError> {
    let text = path
        .into_os_string()
        .into_string()
        .map_err(|_| InstallError::GitNonUtf8("name Git worktree"))?;
    valid_value(&text)?;
    if !Path::new(&text).is_absolute() {
        return Err(InstallError::InvalidRecord(
            "Git worktree path is not absolute".to_owned(),
        ));
    }
    Ok(text)
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-hook-activation-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("scratch directory creates");
        fs::canonicalize(&root).expect("scratch directory resolves")
    }

    fn register(common: &Path, id: &str, checkout: &Path) {
        let admin = common.join("worktrees").join(id);
        fs::create_dir_all(&admin).expect("registration creates");
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", checkout.join(".git").display()),
        )
        .expect("gitdir writes");
    }

    #[test]
    fn registered_worktrees_split_into_live_and_prunable_rows() {
        let scratch = scratch("worktrees");
        let root = scratch.join("repo");
        fs::create_dir_all(&root).expect("main worktree creates");
        crate::test_repository::initialize(&root);
        let common = root.join(".git");
        let live = scratch.join("live");
        fs::create_dir_all(&live).expect("live checkout creates");
        fs::write(live.join(".git"), "gitdir: elsewhere\n").expect("live pointer writes");
        register(&common, "live", &live);
        register(&common, "gone", &scratch.join("gone"));
        register(&common, "locked", &scratch.join("locked"));
        fs::write(common.join("worktrees/locked/locked"), "").expect("lock writes");
        fs::create_dir_all(common.join("worktrees/orphan")).expect("orphan creates");

        let repository = Repository::discover(&root).expect("repository discovers");
        let worktrees = list_worktrees(&repository).expect("worktrees list");
        let mut linked = worktrees.live[1..].to_vec();
        linked.sort();

        let git = |path: &Path| spelled(path).expect("fixture path spells");
        assert_eq!(worktrees.live[0], git(&root));
        assert_eq!(linked, [git(&live), git(&scratch.join("locked"))]);
        assert_eq!(worktrees.prunable, [git(&scratch.join("gone"))]);
        fs::remove_dir_all(scratch).ok();
    }

    #[test]
    fn recorded_worktree_order_keeps_siblings_before_descendants() {
        let base = Path::new(if cfg!(windows) { "C:/work" } else { "/work" });
        let root = base.join("repo");
        let sibling = base.join("repo-a");
        let nested = root.join("child");
        let ordered = ordered_worktrees(vec![nested.clone(), root.clone(), sibling.clone()])
            .expect("absolute worktrees have valid spellings");
        assert_eq!(
            ordered,
            [root, sibling, nested].map(|path| path.into_os_string().into_string().unwrap()),
        );
    }

    #[test]
    fn replacing_a_scope_rewrites_only_its_top_level_hook_values() {
        let scratch = scratch("replace");
        crate::test_repository::initialize(&scratch);
        let config = scratch.join(".git/config");
        let mut text = fs::read_to_string(&config).expect("config reads");
        text.push_str(
            "[core]\n\thooksPath = old\n[core \"sub\"]\n\thooksPath = nested\n\
             [user]\n\tname = kept\n[core]\n\thookspath = older\n",
        );
        fs::write(&config, text).expect("config writes");
        let mutation = |generated: Vec<String>| HookConfigurationMutation {
            worktree: scratch.display().to_string(),
            scope: HookConfigurationScope::Local,
            previous: Vec::new(),
            superseded: None,
            generated,
        };
        let values = vec![" lead".to_owned(), r#"a;b#c"d\e"#.to_owned()];

        set_values(&mutation(values.clone()), &values).expect("values replace");
        assert_eq!(
            scope_values(&scratch, HookConfigurationScope::Local).expect("values read"),
            values
        );
        set_values(&mutation(Vec::new()), &[]).expect("values clear");

        assert!(
            scope_values(&scratch, HookConfigurationScope::Local)
                .expect("values read")
                .is_empty()
        );
        let reread = config_file(&config, HookConfigurationScope::Local).expect("config parses");
        let text = |key: &str| reread.string(key).map(|value| value.to_string());
        assert_eq!(text("user.name").as_deref(), Some("kept"));
        assert_eq!(text("core.sub.hooksPath").as_deref(), Some("nested"));
        fs::remove_dir_all(scratch).ok();
    }

    #[test]
    fn worktree_scope_counts_only_once_the_extension_enables_it() {
        let scratch = scratch("worktree-scope");
        crate::test_repository::initialize(&scratch);
        fs::write(
            scratch.join(".git/config.worktree"),
            "[core]\n\thooksPath = local\n",
        )
        .expect("worktree config writes");

        assert!(worktree_values(&scratch).expect("values read").is_empty());
        let config = scratch.join(".git/config");
        let mut text = fs::read_to_string(&config).expect("config reads");
        text.push_str("[extensions]\n\tworktreeConfig = true\n");
        fs::write(&config, text).expect("config writes");
        assert_eq!(worktree_values(&scratch).expect("values read"), ["local"]);
        fs::remove_dir_all(scratch).ok();
    }
}

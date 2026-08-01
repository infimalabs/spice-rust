use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use sill::{Acquisition, LockFile, LockName};
use spice::repository::Repository;
use wire::{Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

const SKILL_SOURCE: &str = include_str!("../../.agents/skills/spice/SKILL.md");
const SPICE_CONFIG: &str = include_str!("../../spice.toml");
const BASELINE: &str = include_str!("../../baseline.toml");
const SYSTEMD_UNIT: &str = concat!(
    "[Unit]\n",
    "Description=Spice repository resident\n",
    "After=network.target\n\n",
    "[Service]\n",
    "Type=simple\n",
    "ExecStart=spice up\n",
    "ExecStop=spice down\n",
    "Restart=on-failure\n\n",
    "[Install]\n",
    "WantedBy=default.target\n",
);
const LAUNCHD_UNIT: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
    "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    "<plist version=\"1.0\">\n",
    "<dict>\n",
    "  <key>Label</key><string>dev.spice.resident</string>\n",
    "  <key>ProgramArguments</key>\n",
    "  <array><string>spice</string><string>up</string></array>\n",
    "  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n",
    "  <key>ProcessType</key><string>Background</string>\n",
    "</dict>\n",
    "</plist>\n",
);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ArtifactId {
    GitConfig,
    HookEntry,
    HookStatic,
    Skill,
    TrackedConfig,
    Baseline,
    SystemdUnit,
    LaunchdUnit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Disposition {
    Create,
    Replace,
    UpdateMode,
    Unchanged,
    Preserve,
    Restore,
    Remove,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Operation {
    artifact: ArtifactId,
    path: PathBuf,
    disposition: Disposition,
    before: Digest,
    after: Digest,
    executable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct InstallReport {
    action: Action,
    coexistence: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<Digest>,
    operations: Vec<Operation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Preview,
    Applied,
    Unapplied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct State {
    bytes: Vec<u8>,
    mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RecordEntry {
    artifact: ArtifactId,
    prior: Option<State>,
    installed: Digest,
    installed_mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct InstallRecord {
    version: u32,
    entries: Vec<RecordEntry>,
}

#[derive(Clone, Debug)]
struct Artifact {
    id: ArtifactId,
    path: PathBuf,
    prior: Option<State>,
    desired: State,
    write: bool,
}

#[derive(Clone, Debug)]
struct Plan {
    coexistence: bool,
    artifacts: Vec<Artifact>,
    operations: Vec<Operation>,
    digest: Digest,
}

pub(crate) fn run(matches: &ArgMatches) -> Outcome<InstallReport> {
    let current = match crate::working_directory() {
        Ok(path) => path,
        Err(outcome) => return map_outcome(outcome),
    };
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return fault("install::repository-unresolved", &error.to_string()),
    };
    if matches.get_flag("unapply") {
        return unapply(&repository);
    }
    let plan = match Plan::build(&repository) {
        Ok(plan) => plan,
        Err(outcome) => return map_outcome(outcome),
    };
    match matches.get_one::<String>("apply") {
        Some(requested) => apply(&repository, requested),
        None => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: plan.report(Action::Preview),
        }),
    }
}

impl Plan {
    fn build(repository: &Repository) -> Result<Self, Outcome<()>> {
        let (git_config, coexistence) = git_config_artifact(repository)?;
        let mut artifacts = vec![git_config];
        artifacts.extend(content_artifacts(repository)?);
        let operations = artifacts.iter().map(operation).collect::<Vec<_>>();
        let bytes = wire::encode(&(coexistence, &operations))
            .map_err(|error| protocol("install::plan-unencodable", &error.to_string()))?;
        let digest = Digest::compute(Domain::InstallPlan, &bytes);
        Ok(Self {
            coexistence,
            artifacts,
            operations,
            digest,
        })
    }

    fn report(&self, action: Action) -> InstallReport {
        InstallReport {
            action,
            coexistence: self.coexistence,
            digest: Some(self.digest.clone()),
            record: None,
            operations: self.operations.clone(),
        }
    }
}

fn git_config_artifact(repository: &Repository) -> Result<(Artifact, bool), Outcome<()>> {
    let path = repository.common_dir().join("config");
    let prior = read_state(&path)?
        .ok_or_else(|| environment("install::git-config-missing", &path.display().to_string()))?;
    let text = std::str::from_utf8(&prior.bytes).map_err(|error| {
        refusal(
            "install::git-config-non-utf8",
            Some(path.display().to_string()),
            &error.to_string(),
        )
    })?;
    let mut config = gix_config::File::from_str(text).map_err(|error| {
        refusal(
            "install::git-config-malformed",
            Some(path.display().to_string()),
            &error.to_string(),
        )
    })?;
    let coexistence = predecessor_hooks_are_active(repository, &config);
    if !coexistence {
        let hooks = repository.common_dir().join(".spice/hooks");
        let value = hooks.to_str().ok_or_else(|| {
            environment("install::hook-path-non-utf8", &hooks.display().to_string())
        })?;
        config
            .set_raw_value("core.hooksPath", value)
            .map_err(|error| protocol("install::git-config-update", &error.to_string()))?;
    }
    let desired = State {
        bytes: config.to_bstring().to_vec(),
        mode: prior.mode,
    };
    Ok((
        artifact(
            ArtifactId::GitConfig,
            path,
            Some(prior),
            desired,
            !coexistence,
        ),
        coexistence,
    ))
}

fn predecessor_hooks_are_active(repository: &Repository, config: &gix_config::File) -> bool {
    if repository.worktree_root().join(".spice/hooks").is_dir() {
        return true;
    }
    let Ok(value) = config.raw_value("core.hooksPath") else {
        return false;
    };
    let value: &[u8] = value.as_ref();
    value == b".spice/hooks"
}

fn content_artifacts(repository: &Repository) -> Result<Vec<Artifact>, Outcome<()>> {
    let common = repository.common_dir().join(".spice");
    let worktree = repository.worktree_root();
    let skill = generated_skill();
    let specs = [
        (
            ArtifactId::HookEntry,
            common.join("hooks/entry.sh"),
            steer::hook::STAGES[0].bytes().as_bytes(),
            0o755,
            true,
            false,
        ),
        (
            ArtifactId::HookStatic,
            common.join("hooks/static/stage.sh"),
            steer::hook::STAGES[1].bytes().as_bytes(),
            0o755,
            true,
            false,
        ),
        (
            ArtifactId::Skill,
            worktree.join(".agents/skills/spice/SKILL.md"),
            skill.as_bytes(),
            0o644,
            true,
            false,
        ),
        (
            ArtifactId::TrackedConfig,
            worktree.join("spice.toml"),
            SPICE_CONFIG.as_bytes(),
            0o644,
            true,
            true,
        ),
        (
            ArtifactId::Baseline,
            worktree.join("baseline.toml"),
            BASELINE.as_bytes(),
            0o644,
            true,
            true,
        ),
        (
            ArtifactId::SystemdUnit,
            common.join("unit/spice.service"),
            SYSTEMD_UNIT.as_bytes(),
            0o644,
            true,
            false,
        ),
        (
            ArtifactId::LaunchdUnit,
            common.join("unit/dev.spice.resident.plist"),
            LAUNCHD_UNIT.as_bytes(),
            0o644,
            true,
            false,
        ),
    ];
    specs
        .into_iter()
        .map(|(id, path, bytes, mode, write, preserve)| {
            let prior = read_state(&path)?;
            let exists = prior.is_some();
            let desired = if preserve {
                prior.clone().unwrap_or_else(|| state(bytes, mode))
            } else {
                state(bytes, mode)
            };
            Ok(artifact(
                id,
                path,
                prior,
                desired,
                write && !(preserve && exists),
            ))
        })
        .collect()
}

fn artifact(
    id: ArtifactId,
    path: PathBuf,
    prior: Option<State>,
    desired: State,
    write: bool,
) -> Artifact {
    Artifact {
        id,
        path,
        prior,
        desired,
        write,
    }
}

fn state(bytes: &[u8], mode: u32) -> State {
    State {
        bytes: bytes.to_vec(),
        mode,
    }
}

fn operation(artifact: &Artifact) -> Operation {
    let before = artifact
        .prior
        .as_ref()
        .map_or_else(empty_digest, state_digest);
    let after = state_digest(&artifact.desired);
    let disposition = if !artifact.write {
        Disposition::Preserve
    } else {
        match &artifact.prior {
            None => Disposition::Create,
            Some(prior) if prior.bytes != artifact.desired.bytes => Disposition::Replace,
            Some(prior) if prior.mode != artifact.desired.mode => Disposition::UpdateMode,
            Some(_) => Disposition::Unchanged,
        }
    };
    Operation {
        artifact: artifact.id,
        path: artifact.path.clone(),
        disposition,
        before,
        after,
        executable: artifact.desired.mode & 0o111 != 0,
    }
}

fn generated_skill() -> String {
    let mut skill = SKILL_SOURCE.trim_end().to_owned();
    skill.push_str("\n\n## Generated Command Surface\n\n| Syntax | Battery |\n| --- | --- |\n");
    for contract in crate::cli::contracts() {
        let syntax = contract.syntax.replace('|', "\\|");
        skill.push_str(&format!("| `spice {syntax}` | `{}` |\n", contract.battery));
    }
    skill
}

fn apply(repository: &Repository, requested: &str) -> Outcome<InstallReport> {
    let possession = match install_lock(repository) {
        Ok(possession) => possession,
        Err(outcome) => return map_outcome(outcome),
    };
    let plan = match Plan::build(repository) {
        Ok(plan) => plan,
        Err(outcome) => return map_outcome(outcome),
    };
    if requested != plan.digest.as_str() {
        return map_outcome(stale_plan(requested, &plan.digest));
    }
    let old_record = match read_record(repository) {
        Ok(record) => record,
        Err(outcome) => return map_outcome(outcome),
    };
    let record = merged_record(old_record.as_ref(), &plan);
    let record_digest = match publish_plan(repository, &plan, &record) {
        Ok(digest) => digest,
        Err(outcome) => return map_outcome(outcome),
    };
    drop(possession);
    let mut report = plan.report(Action::Applied);
    report.record = Some(record_digest);
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: report,
    })
}

fn install_lock(repository: &Repository) -> Result<sill::Possession, Outcome<()>> {
    let state = repository.common_dir().join(".spice");
    fs::create_dir_all(&state)
        .map_err(|error| environment("install::state-unavailable", &error.to_string()))?;
    let lock = LockFile::new(
        LockName::new("install").expect("install is a valid lock name"),
        state.join("install.lock"),
    );
    match lock.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => Ok(possession),
        Ok(Acquisition::Contended) => Err(refusal(
            "install::busy",
            Some(lock.path().display().to_string()),
            "retry after the current installer exits",
        )),
        Err(error) => Err(environment("install::lock-unavailable", &error.to_string())),
    }
}

fn stale_plan(requested: &str, current: &Digest) -> Outcome<()> {
    refusal(
        "install::plan-moved",
        Some(current.to_string()),
        &format!("preview the current plan and apply its digest; requested {requested}"),
    )
}

fn merged_record(old: Option<&InstallRecord>, plan: &Plan) -> InstallRecord {
    let mut prior = old.map_or_else(BTreeMap::new, |record| {
        record
            .entries
            .iter()
            .map(|entry| (entry.artifact, entry.clone()))
            .collect()
    });
    for artifact in &plan.artifacts {
        let changed = artifact.write && artifact.prior.as_ref() != Some(&artifact.desired);
        if changed || prior.contains_key(&artifact.id) {
            let original = prior
                .get(&artifact.id)
                .map_or_else(|| artifact.prior.clone(), |entry| entry.prior.clone());
            prior.insert(
                artifact.id,
                RecordEntry {
                    artifact: artifact.id,
                    prior: original,
                    installed: state_digest(&artifact.desired),
                    installed_mode: artifact.desired.mode,
                },
            );
        }
    }
    InstallRecord {
        version: 1,
        entries: prior.into_values().collect(),
    }
}

fn publish_plan(
    repository: &Repository,
    plan: &Plan,
    record: &InstallRecord,
) -> Result<Digest, Outcome<()>> {
    let changed = plan
        .artifacts
        .iter()
        .filter(|artifact| artifact.write && artifact.prior.as_ref() != Some(&artifact.desired))
        .collect::<Vec<_>>();
    for (index, artifact) in changed.iter().enumerate() {
        if let Err(outcome) = publish_state(&artifact.path, &artifact.desired) {
            rollback(changed[..index].iter())?;
            return Err(outcome);
        }
    }
    let bytes = wire::encode(record)
        .map_err(|error| protocol("install::record-unencodable", &error.to_string()))?;
    let digest = Digest::compute(Domain::InstallRecord, &bytes);
    let record_path = record_path(repository);
    if let Err(outcome) = publish_bytes(&record_path, &bytes, 0o600) {
        rollback(changed.iter())?;
        return Err(outcome);
    }
    Ok(digest)
}

fn rollback<'a>(
    artifacts: impl DoubleEndedIterator<Item = &'a &'a Artifact>,
) -> Result<(), Outcome<()>> {
    for artifact in artifacts.rev() {
        if let Some(prior) = &artifact.prior {
            publish_state(&artifact.path, prior)?;
        } else {
            remove_file(&artifact.path)?;
        }
    }
    Ok(())
}

fn unapply(repository: &Repository) -> Outcome<InstallReport> {
    let possession = match install_lock(repository) {
        Ok(possession) => possession,
        Err(outcome) => return map_outcome(outcome),
    };
    let record = match read_record(repository) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return map_outcome(refusal(
                "install::record-absent",
                None,
                "apply an installation plan before unapplying it",
            ));
        }
        Err(outcome) => return map_outcome(outcome),
    };
    if let Err(outcome) = verify_installed(repository, &record) {
        return map_outcome(outcome);
    }
    let record_digest = match wire::encode(&record) {
        Ok(bytes) => Digest::compute(Domain::InstallRecord, &bytes),
        Err(error) => {
            return map_outcome(protocol("install::record-unencodable", &error.to_string()));
        }
    };
    let operations = restore_record(repository, &record);
    let operations = match operations {
        Ok(operations) => operations,
        Err(outcome) => return map_outcome(outcome),
    };
    if let Err(outcome) = remove_file(&record_path(repository)) {
        return map_outcome(outcome);
    }
    drop(possession);
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: InstallReport {
            action: Action::Unapplied,
            coexistence: false,
            digest: None,
            record: Some(record_digest),
            operations,
        },
    })
}

fn verify_installed(repository: &Repository, record: &InstallRecord) -> Result<(), Outcome<()>> {
    for entry in &record.entries {
        let path = entry.artifact.path(repository);
        let state = read_state(&path)?.ok_or_else(|| drift(&path, "installed path is absent"))?;
        if state_digest(&state) != entry.installed || state.mode != entry.installed_mode {
            return Err(drift(&path, "installed bytes or mode moved after apply"));
        }
    }
    Ok(())
}

fn restore_record(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<Vec<Operation>, Outcome<()>> {
    let mut operations = Vec::new();
    for entry in record.entries.iter().rev() {
        let path = entry.artifact.path(repository);
        let before = read_state(&path)?.map_or_else(empty_digest, |state| state_digest(&state));
        let (after, disposition) = match &entry.prior {
            Some(prior) => {
                publish_state(&path, prior)?;
                (state_digest(prior), Disposition::Restore)
            }
            None => {
                remove_file(&path)?;
                (empty_digest(), Disposition::Remove)
            }
        };
        operations.push(Operation {
            artifact: entry.artifact,
            path,
            disposition,
            before,
            after,
            executable: entry
                .prior
                .as_ref()
                .is_some_and(|state| state.mode & 0o111 != 0),
        });
    }
    operations.reverse();
    Ok(operations)
}

impl ArtifactId {
    fn path(self, repository: &Repository) -> PathBuf {
        let common = repository.common_dir().join(".spice");
        match self {
            Self::GitConfig => repository.common_dir().join("config"),
            Self::HookEntry => common.join("hooks/entry.sh"),
            Self::HookStatic => common.join("hooks/static/stage.sh"),
            Self::Skill => repository
                .worktree_root()
                .join(".agents/skills/spice/SKILL.md"),
            Self::TrackedConfig => repository.worktree_root().join("spice.toml"),
            Self::Baseline => repository.worktree_root().join("baseline.toml"),
            Self::SystemdUnit => common.join("unit/spice.service"),
            Self::LaunchdUnit => common.join("unit/dev.spice.resident.plist"),
        }
    }
}

fn read_record(repository: &Repository) -> Result<Option<InstallRecord>, Outcome<()>> {
    let Some(state) = read_state(&record_path(repository))? else {
        return Ok(None);
    };
    if state.mode & 0o077 != 0 {
        return Err(refusal(
            "install::record-permissions",
            Some(format!("{:o}", state.mode)),
            "restrict the durable install record to mode 0600",
        ));
    }
    wire::decode(&state.bytes)
        .map(Some)
        .map_err(|error| refusal("install::record-malformed", None, &error.to_string()))
}

fn read_state(path: &Path) -> Result<Option<State>, Outcome<()>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(environment(
                "install::metadata-unavailable",
                &format!("{}: {error}", path.display()),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(refusal(
            "install::path-not-file",
            Some(path.display().to_string()),
            "replace the path explicitly with a regular file before installing",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        environment(
            "install::path-unreadable",
            &format!("{}: {error}", path.display()),
        )
    })?;
    Ok(Some(State {
        bytes,
        mode: metadata.permissions().mode() & 0o777,
    }))
}

fn publish_state(path: &Path, state: &State) -> Result<(), Outcome<()>> {
    publish_bytes(path, &state.bytes, state.mode)
}

fn publish_bytes(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Outcome<()>> {
    let parent = path
        .parent()
        .ok_or_else(|| environment("install::parent-missing", &path.display().to_string()))?;
    fs::create_dir_all(parent).map_err(|error| {
        environment(
            "install::parent-unavailable",
            &format!("{}: {error}", parent.display()),
        )
    })?;
    let destination = atomic::Destination::new(path)
        .map_err(|error| environment("install::publication-unavailable", &error.to_string()))?;
    destination
        .publish(|staged| staged.write_all(bytes))
        .map_err(|error| environment("install::publication-failed", &error.to_string()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        environment(
            "install::permissions-failed",
            &format!("{}: {error}", path.display()),
        )
    })
}

fn remove_file(path: &Path) -> Result<(), Outcome<()>> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(environment(
            "install::removal-failed",
            &format!("{}: {error}", path.display()),
        )),
    }
}

fn record_path(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice/install")
}

fn state_digest(state: &State) -> Digest {
    Digest::compute(Domain::InstallArtifact, &state.bytes)
}

fn empty_digest() -> Digest {
    Digest::compute(Domain::InstallArtifact, &[])
}

fn drift(path: &Path, detail: &str) -> Outcome<()> {
    refusal(
        "install::installed-state-moved",
        Some(path.display().to_string()),
        &format!("restore {detail} or reconcile it manually before unapplying"),
    )
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn environment(reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn protocol(reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn fault<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn map_outcome<B>(outcome: Outcome<()>) -> Outcome<B> {
    match outcome {
        Outcome::Reply(_) => unreachable!("unit helper never returns replies"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

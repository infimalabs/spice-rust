//! Binary-rim translation for release publication and daemonless notes.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::ArgMatches;
use wire::body::{ProofSealed, PublishedTarget, ReleasePublished};
use wire::manifest::ProductVersion;
use wire::{
    Digest, Domain, Fault, FaultClass, Outcome, Refusal, ReleaseNotes, ReleasePlan, Reply,
    ReplyPosition, Seq,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdmittedRelease {
    published: proof::PublishedManifest,
    key: Option<Digest>,
    notes: Digest,
    native: Digest,
    targets: Vec<PublishedTarget>,
    external: Option<dist::ReleasePlan>,
}

impl AdmittedRelease {
    pub(crate) fn key(&self) -> &Digest {
        self.key
            .as_ref()
            .expect("release stage is bound to exactly one external plan")
    }

    pub(crate) const fn manifest(&self) -> &Digest {
        &self.published.digest
    }

    pub(crate) const fn notes(&self) -> &Digest {
        &self.notes
    }

    pub(crate) fn receipts(&self) -> impl Iterator<Item = &Digest> {
        self.targets.iter().map(|target| &target.receipt)
    }

    pub(crate) fn targets(&self) -> &[PublishedTarget] {
        &self.targets
    }

    pub(crate) fn fact(&self) -> ReleasePublished {
        ReleasePublished {
            manifest: self.published.digest.clone(),
            notes: self.notes.clone().into(),
            targets: self.targets.clone(),
        }
    }

    fn plan(&self) -> ReleasePlan {
        let external = self
            .external
            .as_ref()
            .expect("operator preview binds the admitted stage to an external plan");
        ReleasePlan {
            key: self.key().clone(),
            repository: external.repository.clone(),
            action: external.action.as_str().to_owned(),
            requested_version: external.requested_version.clone(),
            source_commit: external.source_commit.clone(),
            release_commit: external.release_commit.clone(),
            manifest: self.published.digest.clone(),
            source: self.published.manifest.source.clone(),
            notes: self.notes.clone(),
            notes_document: String::from_utf8(external.notes.clone())
                .expect("curated release notes were admitted from UTF-8"),
            package: external.package.clone(),
            package_object: external.package_object.clone(),
            installed_artifact: external.installed_artifact.clone(),
            targets: self.targets.clone(),
            operations: external
                .operations
                .iter()
                .map(|operation| operation.as_str().to_owned())
                .collect(),
            observations: wire::ReleasePlanObservations {
                head: external.observations.head.clone(),
                clean: external.observations.clean,
                bump_claimed: external.observations.bump_claimed,
                unrecorded_local_commits: external.observations.unrecorded_local_commits,
                source_version: external.observations.source_version.clone(),
                release_version: external.observations.release_version.clone(),
                credentials_valid: external.observations.credentials_valid,
                source_ref: external.observations.source_ref.clone(),
                package_ref: external.observations.package_ref.clone(),
                tag_ref: external.observations.tag_ref.clone(),
                announcement_ref: external.observations.announcement_ref.clone(),
                registry: external.observations.registry.as_ref().map(|evidence| {
                    wire::ReleaseRegistryEvidence {
                        version: evidence.version.clone(),
                        release_commit: evidence.release_commit.clone(),
                        package: evidence.package.clone(),
                    }
                }),
            },
        }
    }

    fn bind_external(mut self, plan: dist::ReleasePlan) -> Self {
        self.key = Some(plan.key.clone());
        self.external = Some(plan);
        self
    }

    pub(crate) fn with_key(mut self, key: Digest) -> Self {
        self.key = Some(key);
        self
    }

    pub(crate) fn external_plan(&self) -> &dist::ReleasePlan {
        self.external
            .as_ref()
            .expect("operator apply carries the previewed external plan")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationState {
    Fresh { proof: Seq },
    Published { fact: Seq },
}

impl PublicationState {
    pub(crate) const fn observation(self) -> Seq {
        match self {
            Self::Fresh { proof } => proof,
            Self::Published { fact } => fact,
        }
    }
}

pub(crate) enum RunResult {
    Plan(Box<Outcome<ReleasePlan>>),
    Published(Outcome<wire::intent::RecognitionAnswer>),
}

pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> RunResult {
    let prepared = match prepare(matches) {
        Ok(prepared) => prepared,
        Err(outcome) => return RunResult::Plan(outcome),
    };
    let Some(apply) = matches.get_one::<String>("apply") else {
        return RunResult::Plan(Box::new(Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: prepared.release.plan(),
        })));
    };
    let applied = match apply.parse::<Digest>() {
        Ok(applied) => applied,
        Err(error) => {
            return RunResult::Published(Outcome::Refusal(Refusal::bare(
                wire::RefusalDiagnostic::new(
                    "release::apply-digest-refused".to_owned(),
                    Some("publication-plan".to_owned()),
                    Some(prepared.release.key().to_string()),
                    Some(error.to_string()),
                ),
            )));
        }
    };
    if applied != *prepared.release.key() {
        return RunResult::Published(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "release::plan-moved".to_owned(),
                Some("publication-plan".to_owned()),
                Some(prepared.release.key().to_string()),
                Some(applied.to_string()),
            ),
        )));
    }
    if let Err(outcome) = crate::release_execution::execute(
        &prepared.worktree,
        &prepared.operation_journal,
        prepared.release.external_plan(),
        &applied,
    ) {
        return RunResult::Published(outcome);
    }
    let outcome = crate::command_client::execute(
        &prepared.canvas,
        contract,
        crate::command_client::SelectedCommand::ReleasePublish {
            release: &prepared.release,
            state: prepared.state,
        },
    );
    RunResult::Published(recognition_outcome(outcome))
}

struct Prepared {
    release: AdmittedRelease,
    state: PublicationState,
    canvas: PathBuf,
    worktree: PathBuf,
    operation_journal: PathBuf,
}

fn prepare(matches: &ArgMatches) -> Result<Prepared, Box<Outcome<ReleasePlan>>> {
    let current = std::env::current_dir().map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::working-directory-unavailable",
            &error.to_string(),
        ))
    })?;
    let repository = spice::repository::Repository::discover(&current).map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::repository-unresolved",
            &error.to_string(),
        ))
    })?;
    let canonical = crate::release_history::draft(
        repository.worktree_root(),
        matches
            .get_one::<String>("release-commit")
            .map(String::as_str),
        matches.get_one::<String>("version").map(String::as_str),
    )
    .map_err(|error| {
        Box::new(notes_refusal::<ReleasePlan>(
            "release::history-refused",
            &error,
        ))
    })?;
    let candidate = match matches.get_one::<String>("notes-file") {
        Some(path) => fs::read_to_string(path).map_err(|error| {
            Box::new(environment::<ReleasePlan>(
                "release::notes-unreadable",
                &format!("cannot read release notes {path:?}: {error}"),
            ))
        })?,
        None => canonical.document.clone(),
    };
    let notes = dist::curated_release_notes(&candidate, &canonical).map_err(|error| {
        Box::new(notes_refusal::<ReleasePlan>(
            "release::notes-refused",
            &error,
        ))
    })?;
    let external = resolve_external_preconditions(
        repository.worktree_root(),
        matches,
        &canonical.release_commit,
    )
    .map_err(|error| {
        Box::new(refusal::<ReleasePlan>(
            "release::external-plan-refused",
            &error,
        ))
    })?;
    let app = app_digest(repository.worktree_root())
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::app-stage-refused", &error)))?;
    let release = admit_stage(
        &repository.worktree_root().join("dist/stage"),
        &app,
        &notes.identity,
    )
    .map_err(|error| Box::new(refusal::<ReleasePlan>("release::manifest-refused", &error)))?;
    let release = bind_external_plan(
        repository.worktree_root(),
        &canonical.release_commit,
        &notes.document,
        release,
        external,
    )
    .map_err(|error| {
        Box::new(refusal::<ReleasePlan>(
            "release::external-plan-refused",
            &error,
        ))
    })?;
    let state_paths = repository
        .admit_state()
        .map_err(|error| Box::new(crate::state_failure(&error).outcome()))?;
    let reader = journal::JournalReader::open(state_paths.journal()).map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::journal-unavailable",
            &error.to_string(),
        ))
    })?;
    let snapshot = reader.fact_snapshot().map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::journal-unreadable",
            &error.to_string(),
        ))
    })?;
    let state = observe(&snapshot, &release)
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::evidence-refused", &error)))?;
    Ok(Prepared {
        release,
        state,
        canvas: state_paths.canvas().to_path_buf(),
        worktree: repository.worktree_root().to_path_buf(),
        operation_journal: state_paths.data().join("release-operations"),
    })
}

fn recognition_outcome(
    outcome: Outcome<wire::intent::CommandAnswer>,
) -> Outcome<wire::intent::RecognitionAnswer> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            wire::intent::CommandAnswer::Recognition(body) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            _ => Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "release::answer-shape".to_owned(),
                detail: "resident returned a non-recognition release answer".to_owned(),
            }),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

#[cfg(test)]
mod tests {
    use wire::intent::{CommandAnswer, StateAnswer};
    use wire::{ResidentState, Seq, SeqRange};

    use super::*;

    #[test]
    fn publication_wrapper_refuses_an_answer_outside_its_command_binding() {
        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(1).expect("fixture position"),
                last: Seq::new(1).expect("fixture position"),
            }),
            body: CommandAnswer::State(StateAnswer {
                from: ResidentState::Seated,
                to: ResidentState::Dark,
            }),
        });

        let Outcome::Fault(fault) = recognition_outcome(outcome) else {
            panic!("a release reply with another command's answer must fault");
        };
        assert_eq!(fault.class, FaultClass::Protocol);
        assert_eq!(fault.reason, "release::answer-shape");
    }
}

pub(crate) fn app_digest(worktree: &Path) -> Result<Digest, String> {
    canvas::AppSource::resolve(worktree)
        .and_then(|source| source.digest())
        .map_err(|error| error.to_string())
}

pub(crate) fn admit_stage(
    root: &Path,
    app: &Digest,
    notes: &Digest,
) -> Result<AdmittedRelease, ReleaseError> {
    let executable = std::env::current_exe().map_err(ReleaseError::CurrentExecutable)?;
    let bytes = fs::read(&executable).map_err(|source| ReleaseError::ReadExecutable {
        path: executable,
        source,
    })?;
    let native = Digest::compute(Domain::DistArtifact, &bytes);
    let target = proof::compiled_target().map_err(ReleaseError::UnsupportedHost)?;
    let published =
        proof::admit_manifest(root, app, target, &native).map_err(ReleaseError::Manifest)?;
    let targets = published
        .manifest
        .targets
        .iter()
        .map(|target| PublishedTarget {
            target: target.target.to_string(),
            artifact: target.payload.clone(),
            receipt: target.receipt.clone(),
        })
        .collect();
    Ok(AdmittedRelease {
        published,
        key: None,
        notes: notes.clone(),
        native,
        targets,
        external: None,
    })
}

struct ExternalPreconditions {
    requested_version: ProductVersion,
    source_commit: String,
    action: dist::ReleaseAction,
    repository: Digest,
    head: String,
    clean: bool,
    bump_claimed: bool,
    unrecorded_local_commits: u64,
    source_version: ProductVersion,
    release_version: ProductVersion,
    source_ref: Option<String>,
    package_ref: Option<String>,
    tag_ref: Option<String>,
    announcement_ref: Option<String>,
}

struct ReleaseVersions {
    requested: ProductVersion,
    source: ProductVersion,
    release: ProductVersion,
}

struct ReleaseGitState {
    source_commit: String,
    head: String,
    clean: bool,
    bump_claimed: bool,
    unrecorded_local_commits: u64,
}

struct ExternalRefs {
    source_ref: Option<String>,
    package_ref: Option<String>,
    tag_ref: Option<String>,
    announcement_ref: Option<String>,
}

fn resolve_external_preconditions(
    root: &Path,
    matches: &ArgMatches,
    release_commit: &str,
) -> Result<ExternalPreconditions, String> {
    let versions = release_versions(root, matches)?;
    let git = release_git_state(root, release_commit, &versions.source, &versions.requested)?;
    validate_release_git_state(
        &git,
        release_commit,
        &versions.source,
        &versions.release,
        &versions.requested,
    )?;
    let action = release_action(matches)?;
    let authority = dist::current_authority(root).map_err(|error| error.to_string())?;
    let external_refs = resolve_external_refs(
        root,
        &authority,
        &versions.requested,
        &git.source_commit,
        release_commit,
    )?;
    validate_external_refs(&external_refs, release_commit)?;
    Ok(ExternalPreconditions {
        requested_version: versions.requested,
        source_commit: git.source_commit,
        action,
        repository: repository_identity(root, authority.remote())?,
        head: git.head,
        clean: git.clean,
        bump_claimed: git.bump_claimed,
        unrecorded_local_commits: git.unrecorded_local_commits,
        source_version: versions.source,
        release_version: versions.release,
        source_ref: external_refs.source_ref,
        package_ref: external_refs.package_ref,
        tag_ref: external_refs.tag_ref,
        announcement_ref: external_refs.announcement_ref,
    })
}

fn release_versions(root: &Path, matches: &ArgMatches) -> Result<ReleaseVersions, String> {
    let requested = matches
        .get_one::<String>("version")
        .map_or(env!("CARGO_PKG_VERSION"), String::as_str);
    let requested_version = ProductVersion::parse(requested).map_err(|error| error.to_string())?;
    let source_version =
        ProductVersion::parse(env!("CARGO_PKG_VERSION")).map_err(|error| error.to_string())?;
    let release_version_text = fs::read_to_string(root.join("dist/VERSION"))
        .map_err(|error| format!("cannot read dist/VERSION: {error}"))?;
    let release_version =
        ProductVersion::parse(release_version_text.trim()).map_err(|error| error.to_string())?;
    Ok(ReleaseVersions {
        requested: requested_version,
        source: source_version,
        release: release_version,
    })
}

fn release_git_state(
    root: &Path,
    release_commit: &str,
    source_version: &ProductVersion,
    requested_version: &ProductVersion,
) -> Result<ReleaseGitState, String> {
    let head = git_required(root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let release_subject = git_required(root, &["show", "-s", "--format=%s", release_commit])?;
    let bump_claimed = source_version == requested_version
        || release_subject == format!("release: bump to {requested_version}");
    let unrecorded_local_commits = git_required(
        root,
        &["rev-list", "--count", &format!("{release_commit}..HEAD")],
    )?
    .parse::<u64>()
    .map_err(|error| format!("Git returned an invalid local commit count: {error}"))?;
    let source_commit = if release_subject.starts_with("release: bump to ") {
        git_optional(
            root,
            &["rev-parse", "--verify", &format!("{release_commit}^")],
        )?
        .unwrap_or_else(|| release_commit.to_owned())
    } else {
        release_commit.to_owned()
    };
    let clean = tracked_worktree_clean(root)?;
    Ok(ReleaseGitState {
        source_commit,
        head,
        clean,
        bump_claimed,
        unrecorded_local_commits,
    })
}

fn validate_release_git_state(
    state: &ReleaseGitState,
    release_commit: &str,
    source_version: &ProductVersion,
    release_version: &ProductVersion,
    requested_version: &ProductVersion,
) -> Result<(), String> {
    if !state.clean {
        return Err("the release worktree is dirty".to_owned());
    }
    if source_version != release_version && !state.bump_claimed {
        return Err("the observed version bump is not claimed by this release".to_owned());
    }
    if state.unrecorded_local_commits != 0 {
        return Err(format!(
            "{} local commits have not been recorded on the \
             publication baseline",
            state.unrecorded_local_commits
        ));
    }
    if release_version != requested_version {
        return Err(format!(
            "requested version {requested_version} disagrees with observed release version \
             {release_version}"
        ));
    }
    if state.head != release_commit {
        return Err(format!(
            "release commit {release_commit} is not HEAD {}",
            state.head
        ));
    }
    Ok(())
}

fn release_action(matches: &ArgMatches) -> Result<dist::ReleaseAction, String> {
    Ok(
        match matches.get_one::<String>("action").map(String::as_str) {
            None | Some("publish") => dist::ReleaseAction::Publish,
            Some("tag-and-announce") => dist::ReleaseAction::TagAndAnnounce,
            Some(other) => return Err(format!("unsupported release action {other:?}")),
        },
    )
}

fn resolve_external_refs(
    root: &Path,
    authority: &dist::ReleaseAuthority,
    requested_version: &ProductVersion,
    source_commit: &str,
    release_commit: &str,
) -> Result<ExternalRefs, String> {
    let refs = authority.refs(requested_version);
    git_required(root, &["ls-remote", "--refs", authority.remote()])?;
    let source_ref = normalize_source_ref(
        remote_ref(root, authority.remote(), refs.source())?,
        source_commit,
        release_commit,
    );
    let package_ref = remote_ref(root, authority.remote(), refs.package())?;
    let tag_ref = normalize_converged_ref(
        remote_ref(root, authority.remote(), refs.tag())?,
        release_commit,
    );
    let announcement_ref = normalize_converged_ref(
        remote_ref(root, authority.remote(), refs.announcement())?,
        release_commit,
    );
    Ok(ExternalRefs {
        source_ref,
        package_ref,
        tag_ref,
        announcement_ref,
    })
}

fn validate_external_refs(refs: &ExternalRefs, release_commit: &str) -> Result<(), String> {
    ensure_release_ref_absent("source", &refs.source_ref, release_commit)?;
    ensure_release_ref_absent("tag", &refs.tag_ref, release_commit)?;
    ensure_release_ref_absent("announcement", &refs.announcement_ref, release_commit)
}

fn ensure_release_ref_absent(
    label: &str,
    observed: &Option<String>,
    release_commit: &str,
) -> Result<(), String> {
    if let Some(object) = observed {
        return Err(format!(
            "the {label} ref names {object}, not {release_commit}"
        ));
    }
    Ok(())
}

fn bind_external_plan(
    root: &Path,
    release_commit: &str,
    notes: &str,
    release: AdmittedRelease,
    external: ExternalPreconditions,
) -> Result<AdmittedRelease, String> {
    let package = crate::release_package::preview(
        root,
        &external.requested_version,
        release.notes.clone(),
        notes.as_bytes(),
        &release.published.bytes,
    )
    .map_err(|error| error.to_string())?;
    let package_ref = normalize_converged_ref(external.package_ref.clone(), &package.git_tree);
    let registry = if external.action == dist::ReleaseAction::TagAndAnnounce {
        let visible = external.package_ref.ok_or_else(|| {
            format!(
                "package v{} is not visible at the release authority",
                external.requested_version
            )
        })?;
        if visible != package.git_tree {
            return Err(format!(
                "package v{} names {visible}, not {}",
                external.requested_version, package.git_tree
            ));
        }
        Some(dist::RegistryEvidence {
            version: external.requested_version.clone(),
            release_commit: release_commit.to_owned(),
            package: package.identity.clone(),
        })
    } else {
        None
    };
    let observations = dist::ReleaseObservations {
        head: external.head,
        clean: external.clean,
        bump_claimed: external.bump_claimed,
        unrecorded_local_commits: external.unrecorded_local_commits,
        source_version: external.source_version,
        release_version: external.release_version,
        credentials_valid: true,
        source_ref: external.source_ref,
        package_ref,
        tag_ref: external.tag_ref,
        announcement_ref: external.announcement_ref,
        registry,
    };
    let plan = dist::plan_release(dist::PlanInput {
        repository: external.repository,
        action: external.action,
        requested_version: external.requested_version,
        source_commit: external.source_commit,
        release_commit: release_commit.to_owned(),
        notes: notes.as_bytes().to_vec(),
        manifest: release.published.digest.clone(),
        package: package.identity,
        package_object: package.git_tree,
        installed_artifact: release.native.clone(),
        targets: release
            .targets
            .iter()
            .map(|target| dist::SealedTarget {
                target: target.target.clone(),
                artifact: target.artifact.clone(),
                receipt: target.receipt.clone(),
            })
            .collect(),
        observations,
    })
    .map_err(|error| error.to_string())?;
    Ok(release.bind_external(plan))
}

fn tracked_worktree_clean(root: &Path) -> Result<bool, String> {
    Ok(git_required(root, &["status", "--porcelain=v1", "--untracked-files=no"])?.is_empty())
}

pub(super) fn repository_identity(root: &Path, remote: &str) -> Result<Digest, String> {
    let key = format!("remote.{remote}.url");
    let repository = git_optional(root, &["config", "--get", &key])?
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    Ok(Digest::compute(
        Domain::DistSourceCommit,
        repository.as_bytes(),
    ))
}

fn remote_ref(root: &Path, remote: &str, reference: &str) -> Result<Option<String>, String> {
    let output = git_required(root, &["ls-remote", "--refs", remote, reference])?;
    let mut fields = output.split_whitespace();
    Ok(fields.next().map(str::to_owned))
}

fn normalize_converged_ref(observed: Option<String>, release: &str) -> Option<String> {
    observed.filter(|object| object != release)
}

fn normalize_source_ref(
    observed: Option<String>,
    source_commit: &str,
    release_commit: &str,
) -> Option<String> {
    observed.filter(|object| object != source_commit && object != release_commit)
}

fn git_required(root: &Path, arguments: &[&str]) -> Result<String, String> {
    git_output(root, arguments).and_then(|(success, output)| {
        if success {
            Ok(output)
        } else {
            Err(format!("git {} refused: {output}", arguments.join(" ")))
        }
    })
}

fn git_optional(root: &Path, arguments: &[&str]) -> Result<Option<String>, String> {
    git_output(root, arguments).map(|(success, output)| success.then_some(output))
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<(bool, String), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("could not launch git: {error}"))?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("git stdout is not UTF-8: {error}"))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|error| format!("git stderr is not UTF-8: {error}"))?;
    let detail = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((output.status.success(), detail))
}

pub(crate) fn observe(
    snapshot: &journal::FactSnapshot,
    release: &AdmittedRelease,
) -> Result<PublicationState, ObservationError> {
    let mut proof = None;
    let mut published = None;
    let expected = release.fact();
    for envelope in snapshot.facts() {
        if let Some(body) = envelope.fact.read::<ProofSealed>() {
            if body.manifest == *release.manifest() {
                proof = Some(envelope.seq);
            }
        } else if let Some(body) = envelope.fact.read::<ReleasePublished>() {
            if body.manifest != *release.manifest() {
                continue;
            }
            if body != expected {
                return Err(ObservationError::PublishedMismatch(envelope.seq));
            }
            if published.replace(envelope.seq).is_some() {
                return Err(ObservationError::DuplicatePublication);
            }
        }
    }
    if let Some(fact) = published {
        Ok(PublicationState::Published { fact })
    } else {
        proof
            .map(|proof| PublicationState::Fresh { proof })
            .ok_or(ObservationError::MissingProof)
    }
}

#[derive(Debug)]
pub(crate) enum ReleaseError {
    CurrentExecutable(std::io::Error),
    ReadExecutable {
        path: PathBuf,
        source: std::io::Error,
    },
    UnsupportedHost(proof::UnsupportedHost),
    Manifest(proof::ManifestAdmissionError),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(source) => {
                write!(formatter, "current executable is unresolved: {source}")
            }
            Self::ReadExecutable { path, source } => {
                write!(
                    formatter,
                    "current executable {} is unreadable: {source}",
                    path.display()
                )
            }
            Self::UnsupportedHost(source) => source.fmt(formatter),
            Self::Manifest(source) => source.fmt(formatter),
        }
    }
}

impl Error for ReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable(source) => Some(source),
            Self::ReadExecutable { source, .. } => Some(source),
            Self::UnsupportedHost(source) => Some(source),
            Self::Manifest(source) => Some(source),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ObservationError {
    MissingProof,
    PublishedMismatch(Seq),
    DuplicatePublication,
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingProof => {
                formatter.write_str("the current manifest has no durable proof::sealed recognition")
            }
            Self::PublishedMismatch(seq) => write!(
                formatter,
                "release publication at {seq} disagrees with the sealed target identities"
            ),
            Self::DuplicatePublication => {
                formatter.write_str("the sealed manifest was published more than once")
            }
        }
    }
}

impl Error for ObservationError {}

fn refusal<B>(reason: &str, error: &impl fmt::Display) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        reason.to_owned(),
        Some("sealed-release".to_owned()),
        Some("one current complete two-target proof and manifest chain".to_owned()),
        Some(error.to_string()),
    )))
}

fn environment<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(wire::Fault {
        class: wire::FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

pub(crate) fn notes(matches: &clap::ArgMatches) -> Outcome<ReleaseNotes> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            return environment("release::working-directory-unavailable", &error.to_string());
        }
    };
    let repository = match spice::repository::Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            return environment("release::repository-unresolved", &error.to_string());
        }
    };
    match crate::release_history::draft(
        repository.worktree_root(),
        matches
            .get_one::<String>("release-commit")
            .map(String::as_str),
        None,
    ) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => notes_refusal("release::history-refused", &error),
    }
}

fn notes_refusal<B>(reason: &str, error: &impl fmt::Display) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        reason.to_owned(),
        Some("first-parent-release-notes".to_owned()),
        Some(
            "reachable release history and curated highlights above the generated inventory"
                .to_owned(),
        ),
        Some(error.to_string()),
    )))
}

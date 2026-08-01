use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use dist::{CommitSummary, ReleaseNarrative, ReleaseNotes};
use spice::repository::Repository;
use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

const NARRATIVE_PATH: &str = "dist/release.toml";

pub(crate) fn notes() -> Outcome<ReleaseNotes> {
    let current = match crate::working_directory() {
        Ok(current) => current,
        Err(outcome) => return map_outcome(outcome),
    };
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return fault("release::repository-unresolved", &error.to_string()),
    };
    match build_notes(&repository) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(Failure::Refusal(detail)) => Outcome::Refusal(Refusal {
            reason: "release::notes-refused".to_owned(),
            witness: None,
            expected: Some(
                "one sealed release, complete narrative, and exact Git range".to_owned(),
            ),
            observed: Some(detail),
            remedy: "seal the current release inputs and resolve every authored placeholder"
                .to_owned(),
        }),
        Err(Failure::Fault(detail)) => fault("release::notes-unavailable", &detail),
    }
}

fn build_notes(repository: &Repository) -> Result<ReleaseNotes, Failure> {
    let narrative_path = repository.worktree_root().join(NARRATIVE_PATH);
    let narrative_text = read_text(&narrative_path)?;
    let narrative: ReleaseNarrative = toml::from_str(&narrative_text)
        .map_err(|error| Failure::Refusal(format!("{}: {error}", narrative_path.display())))?;
    let manifest_path = relative_path(repository.worktree_root(), &narrative.manifest)?;
    let manifest_bytes = read_regular(&manifest_path)?;
    let admitted = dist::admit_release_manifest(&manifest_bytes)
        .map_err(|error| Failure::Refusal(error.to_string()))?;
    if admitted.manifest().journal_schema != journal::SCHEMA_VERSION {
        return Err(Failure::Refusal(format!(
            "sealed release journal schema {} is not current schema {}",
            admitted.manifest().journal_schema,
            journal::SCHEMA_VERSION
        )));
    }
    let commits = git_range(repository.worktree_root(), admitted.manifest())?;
    dist::release_notes(&admitted, &narrative, commits)
        .map_err(|error| Failure::Refusal(error.to_string()))
}

fn relative_path(root: &Path, text: &str) -> Result<PathBuf, Failure> {
    let path = Path::new(text);
    if path.is_absolute()
        || text.is_empty()
        || text.contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Failure::Refusal(format!(
            "release manifest path is not one safe relative spelling: {text:?}"
        )));
    }
    Ok(root.join(path))
}

fn git_range(
    root: &Path,
    manifest: &wire::manifest::ReleaseManifest,
) -> Result<Vec<CommitSummary>, Failure> {
    let repository = gix::open(root)
        .map_err(|error| Failure::Fault(format!("open Git object store: {error}")))?;
    let from = gix::ObjectId::from_str(&manifest.git_from)
        .map_err(|error| Failure::Refusal(format!("resolve git_from: {error}")))?;
    let to = gix::ObjectId::from_str(&manifest.git_to)
        .map_err(|error| Failure::Refusal(format!("resolve git_to: {error}")))?;
    repository
        .find_commit(from)
        .map_err(|error| Failure::Refusal(format!("git_from is not a commit: {error}")))?;
    repository
        .find_commit(to)
        .map_err(|error| Failure::Refusal(format!("git_to is not a commit: {error}")))?;
    let base = repository
        .merge_base(from, to)
        .map_err(|error| Failure::Refusal(format!("Git range has no merge base: {error}")))?;
    if base.detach() != from {
        return Err(Failure::Refusal(
            "git_from is not an ancestor of git_to".to_owned(),
        ));
    }
    let walk = repository
        .rev_walk([to])
        .with_hidden([from])
        .all()
        .map_err(|error| Failure::Refusal(format!("walk sealed Git range: {error}")))?;
    let mut commits = Vec::new();
    for info in walk {
        let info = info.map_err(|error| Failure::Refusal(format!("walk commit: {error}")))?;
        let commit = info
            .object()
            .map_err(|error| Failure::Refusal(format!("read commit {}: {error}", info.id)))?;
        let raw = commit
            .message_raw()
            .map_err(|error| Failure::Refusal(format!("decode commit {}: {error}", info.id)))?;
        let bytes: &[u8] = raw.as_ref();
        let title = String::from_utf8_lossy(bytes)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        commits.push(CommitSummary {
            id: info.id.to_string(),
            title,
        });
    }
    commits.reverse();
    Ok(commits)
}

fn read_text(path: &Path) -> Result<String, Failure> {
    let bytes = read_regular(path)?;
    String::from_utf8(bytes)
        .map_err(|error| Failure::Refusal(format!("{} is not UTF-8: {error}", path.display())))
}

fn read_regular(path: &Path) -> Result<Vec<u8>, Failure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| Failure::Fault(format!("inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(Failure::Refusal(format!(
            "release input is not a regular file: {}",
            path.display()
        )));
    }
    fs::read(path).map_err(|error| Failure::Fault(format!("read {}: {error}", path.display())))
}

enum Failure {
    Refusal(String),
    Fault(String),
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

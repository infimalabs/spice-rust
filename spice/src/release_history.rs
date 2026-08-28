//! Read-only Git observations for the pure distribution notes fold.

use std::error::Error as StdError;
use std::fmt;
use std::path::Path;
use std::process::Command;

use dist::{Landing, NotesInput};
use wire::ReleaseNotes;

const RECENT_LANDING_LIMIT: &str = "5";

/// Resolve and fold release notes at one repository commit.
pub(crate) fn draft(
    root: &Path,
    revision: Option<&str>,
    requested_version: Option<&str>,
) -> Result<ReleaseNotes, Error> {
    draft_with_release(root, revision, requested_version, None)
}

pub(crate) fn projected_draft(
    root: &Path,
    history_commit: &str,
    requested_version: Option<&str>,
    release_commit: &str,
) -> Result<ReleaseNotes, Error> {
    draft_with_release(
        root,
        Some(history_commit),
        requested_version,
        Some(release_commit),
    )
}

fn draft_with_release(
    root: &Path,
    revision: Option<&str>,
    requested_version: Option<&str>,
    projected_release: Option<&str>,
) -> Result<ReleaseNotes, Error> {
    let history_commit = resolve_commit(root, revision.unwrap_or("HEAD"))?;
    let release_commit = projected_release.unwrap_or(&history_commit).to_owned();
    let release_version = requested_version
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_owned();
    let current_tag = format!("v{release_version}");
    let previous_tag = reachable_tags(root, &history_commit)?
        .into_iter()
        .find(|tag| tag != &current_tag);
    let landings = landings(root, previous_tag.as_deref(), &history_commit)?;
    dist::release_notes(
        NotesInput {
            release_version,
            release_commit,
            previous_tag,
            landings,
        },
        |target, landing| ancestor(root, target, landing),
    )
    .map_err(Error::Fold)
}

fn resolve_commit(root: &Path, revision: &str) -> Result<String, Error> {
    read(
        root,
        &[
            "rev-parse".to_owned(),
            "--verify".to_owned(),
            format!("{revision}^{{commit}}"),
        ],
        "resolve the release commit",
    )
}

fn reachable_tags(root: &Path, release_commit: &str) -> Result<Vec<String>, Error> {
    let raw = read(
        root,
        &[
            "tag".to_owned(),
            "--merged".to_owned(),
            release_commit.to_owned(),
            "--list".to_owned(),
            "v*".to_owned(),
            "--sort=-v:refname".to_owned(),
        ],
        "resolve reachable release tags",
    )?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect())
}

fn landings(
    root: &Path,
    previous_tag: Option<&str>,
    release_commit: &str,
) -> Result<Vec<Landing>, Error> {
    let format = concat!(
        "--format=%H%x1f%P%x1f%s%x1f%(trailers:key=Task-Project,valueonly)",
        "%x1f%(trailers:key=Task-Key,valueonly)",
        "%x1f%(trailers:key=Task-Handle,valueonly)%x1f%b%x1e",
    );
    let mut arguments = vec![
        "log".to_owned(),
        "--first-parent".to_owned(),
        "--reverse".to_owned(),
    ];
    if previous_tag.is_none() {
        arguments.extend(["-n".to_owned(), RECENT_LANDING_LIMIT.to_owned()]);
    }
    arguments.push(format.to_owned());
    arguments.push(previous_tag.map_or_else(
        || release_commit.to_owned(),
        |tag| format!("refs/tags/{tag}..{release_commit}"),
    ));
    let raw = read(root, &arguments, "read first-parent release landings")?;
    let mut landings = Vec::new();
    for raw_record in raw.split('\x1e') {
        let raw_record = raw_record.trim_matches('\n');
        if raw_record.is_empty() {
            continue;
        }
        let mut fields = raw_record.splitn(7, '\x1f');
        let commit = fields.next().unwrap_or_default().trim();
        let parents = fields.next().unwrap_or_default().trim();
        let subject = fields.next().unwrap_or_default().trim();
        let project = fields.next().unwrap_or_default().trim();
        let task_key = fields.next().unwrap_or_default().trim();
        let task_handle = fields.next().unwrap_or_default().trim();
        let body = fields.next().unwrap_or_default().trim_matches('\n');
        let task = normalize_task_key(if task_key.is_empty() {
            task_handle
        } else {
            task_key
        });
        if commit.is_empty() || subject.starts_with("release: bump to ") {
            continue;
        }
        landings.push(Landing {
            commit: commit.to_owned(),
            first_parent: parents.split_whitespace().next().map(str::to_owned),
            subject: subject.to_owned(),
            project: project.to_owned(),
            task: (!task.is_empty()).then(|| task.to_owned()),
            body: body.to_owned(),
        });
    }
    Ok(landings)
}

fn normalize_task_key(value: &str) -> &str {
    value.rsplit_once('-').map_or(value, |(_, key)| key)
}

fn ancestor(root: &Path, candidate: &str, commit: &str) -> Result<bool, String> {
    let output = run(
        root,
        &[
            "merge-base".to_owned(),
            "--is-ancestor".to_owned(),
            candidate.to_owned(),
            commit.to_owned(),
        ],
    )
    .map_err(|error| error.to_string())?;
    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(output.detail("inspect reverted-commit containment")),
    }
}

fn read(root: &Path, arguments: &[String], action: &str) -> Result<String, Error> {
    let output = run(root, arguments)?;
    if output.status == 0 {
        Ok(output.stdout.trim_end_matches('\n').to_owned())
    } else {
        Err(Error::Command(output.detail(action)))
    }
}

fn run(root: &Path, arguments: &[String]) -> Result<GitOutput, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| Error::Launch(error.to_string()))?;
    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| Error::Encoding(format!("Git stdout is not UTF-8: {error}")))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|error| Error::Encoding(format!("Git stderr is not UTF-8: {error}")))?;
    Ok(GitOutput {
        status,
        stdout,
        stderr,
    })
}

struct GitOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

impl GitOutput {
    fn detail(&self, action: &str) -> String {
        let detail = [&self.stdout, &self.stderr]
            .into_iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if detail.is_empty() {
            format!("could not {action} (git exit {})", self.status)
        } else {
            format!("could not {action} (git exit {})\n{detail}", self.status)
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Launch(String),
    Encoding(String),
    Command(String),
    Fold(dist::NotesError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launch(detail) => write!(formatter, "could not launch Git: {detail}"),
            Self::Encoding(detail) | Self::Command(detail) => formatter.write_str(detail),
            Self::Fold(error) => error.fmt(formatter),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Fold(error) => Some(error),
            Self::Launch(_) | Self::Encoding(_) | Self::Command(_) => None,
        }
    }
}

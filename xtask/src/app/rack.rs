//! The spice rack every appliance build links: the staged app generation and
//! the tracked sources of every other namespace, assembled in one place.
//!
//! Release assembly and a development refresh both link what [`assemble`]
//! answers, so the same staged generation and the same tracked sources give
//! byte-identical racks: members are ordered by path, every gzip stream has a
//! fixed header, and no timestamp or host path enters the archive.

use std::path::{Component, Path};

use spice::serve::rack::{Namespace, Rack, RackInput};

use super::AppBuildError;

/// One tracked worktree source of a namespace besides `app`, which the staged
/// generation supplies.
enum Source {
    /// One file, stored as `member` within `namespace`.
    File {
        namespace: Namespace,
        member: &'static str,
        path: &'static str,
    },
    /// Every regular file beneath a directory, stored at its path within it.
    Directory {
        namespace: Namespace,
        path: &'static str,
    },
}

/// The tracked source list, keyed by namespace.
const SOURCES: [Source; 2] = [
    Source::File {
        namespace: Namespace::Skill,
        member: "SKILL.md",
        path: "spice/agent/SKILL.md",
    },
    Source::Directory {
        namespace: Namespace::Doc,
        path: "docs",
    },
];

/// Assemble the rack from the admitted `app` archive staged for its
/// generation and the tracked sources under `root`.
///
/// App members keep their stored gzip streams, so only the other namespaces
/// are deflated here.
pub(super) fn assemble(root: &Path, app: &Rack<'_>) -> Result<Vec<u8>, AppBuildError> {
    let mut tracked = Vec::new();
    for source in &SOURCES {
        match source {
            Source::File {
                namespace,
                member,
                path,
            } => {
                let bytes = super::read_regular(&root.join(path))?;
                if *namespace == Namespace::Skill
                    && bytes.len() > spice::install::SKILL_BYTES_CEILING
                {
                    return Err(AppBuildError::GeneratedStage(format!(
                        "{path} exceeds the {} byte authoritative skill ceiling",
                        spice::install::SKILL_BYTES_CEILING
                    )));
                }
                tracked.push((namespace.member(member), bytes));
            }
            Source::Directory { namespace, path } => {
                let directory = root.join(path);
                let mut files = Vec::new();
                super::source::collect_directory(root, &directory, &mut files)?;
                for file in files {
                    let absolute = root.join(&file);
                    let member = member_path(&absolute, &directory)?;
                    tracked.push((namespace.member(&member), super::read_regular(&absolute)?));
                }
            }
        }
    }
    let app_prefix = Namespace::App.member("");
    Rack::write(
        app.members()
            .filter(|(path, _)| path.starts_with(app_prefix.as_str()))
            .map(|(path, member)| (path, RackInput::Stored(member)))
            .chain(
                tracked
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), RackInput::Bytes(bytes))),
            ),
    )
    .map_err(AppBuildError::Canvas)
}

/// `file`'s path below `directory`, its segments joined by `/` on every host.
fn member_path(file: &Path, directory: &Path) -> Result<String, AppBuildError> {
    let relative = file.strip_prefix(directory).map_err(|_| {
        AppBuildError::SourceIdentity(format!(
            "{} escaped {} while rack sources were collected",
            file.display(),
            directory.display()
        ))
    })?;
    relative
        .components()
        .map(|component| match component {
            Component::Normal(segment) => segment
                .to_str()
                .ok_or_else(|| AppBuildError::NonUtf8(file.to_path_buf())),
            _ => Err(AppBuildError::SourceIdentity(format!(
                "{} is not a plain path below {}",
                file.display(),
                directory.display()
            ))),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|segments| segments.join("/"))
}

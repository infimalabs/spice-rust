//! The source commit's half of the product version.

use std::fs;
use std::path::Path;

use dist::{Build, SOURCE_SECOND_POSITIONS, SourceMinor};
use wire::manifest::ProductVersion;

/// The build environment variable that carries the derived product version
/// into the compiled executable.
pub const SOURCE_VERSION_ENV: &str = wire::SpiceVariable::ProductVersion.as_str();

/// The product version a `build` of the checkout at `root` reports: its
/// `dist/VERSION` applied to its HEAD commit's source minor.
///
/// # Errors
///
/// Refuses an unreadable or malformed `dist/VERSION` and every refusal of
/// [`source_minor`].
pub fn source_version(root: &Path, build: Build) -> Result<ProductVersion, String> {
    let path = root.join("dist/VERSION");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let tracked = dist::admit_tracked_version(&text).map_err(|error| error.to_string())?;
    Ok(tracked.product(Some(source_minor(root)?), build))
}

/// Order HEAD of the checkout at `root` by its committer second and its
/// position among same-second first-parent ancestors.
///
/// # Errors
///
/// Refuses an unreadable repository or commit, a backward clock, more
/// same-second commits than the minor orders, and a shallow boundary inside
/// HEAD's second, whose hidden ancestors could shift the position.
pub fn source_minor(root: &Path) -> Result<SourceMinor, String> {
    let repository = gix::open_opts(root, gix::open::Options::isolated())
        .map_err(|error| format!("repository could not open: {error}"))?;
    let head = repository
        .head_id()
        .map_err(|error| format!("HEAD does not name a commit: {error}"))?
        .detach();
    let shallow = repository
        .shallow_commits()
        .map_err(|error| format!("shallow boundaries could not be read: {error}"))?;
    minor_from_first_parents(first_parents(&repository, head, shallow.as_ref()))
}

/// One first-parent commit's committer second, and whether a declared
/// shallow boundary hides its parents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FirstParent {
    seconds: i64,
    boundary: bool,
}

/// `head` and each first-parent ancestor, read lazily so a caller that stops
/// early never visits the rest of history. A shallow boundary ends the walk
/// even though its commit names a parent object that is intentionally absent.
fn first_parents<'repository>(
    repository: &'repository gix::Repository,
    head: gix::ObjectId,
    shallow: Option<&'repository gix::shallow::Commits>,
) -> impl Iterator<Item = Result<FirstParent, String>> + 'repository {
    let mut next = Some(head);
    std::iter::from_fn(move || {
        let id = next.take()?;
        Some(
            repository
                .find_commit(id)
                .map_err(|error| format!("commit {id} could not be read: {error}"))
                .and_then(|commit| {
                    let boundary =
                        shallow.is_some_and(|commits| commits.binary_search(&id).is_ok());
                    if !boundary {
                        next = commit.parent_ids().next().map(gix::Id::detach);
                    }
                    let seconds = commit
                        .time()
                        .map_err(|error| format!("commit {id} has no committer instant: {error}"))?
                        .seconds;
                    Ok(FirstParent { seconds, boundary })
                }),
        )
    })
}

/// HEAD's committer second and its position in the same-second first-parent
/// chain beneath it. Git stores only seconds, and deterministic task landings
/// inherit the later parent's clock, so each tied descendant takes the next
/// position. Nothing past the first earlier ancestor is read, and that
/// ancestor may be a shallow boundary; a boundary still inside HEAD's second
/// refuses, so every checkout of one commit derives one position.
fn minor_from_first_parents(
    commits: impl IntoIterator<Item = Result<FirstParent, String>>,
) -> Result<SourceMinor, String> {
    let mut commits = commits.into_iter();
    let head = commits
        .next()
        .ok_or_else(|| "HEAD has no committer instant".to_owned())??;
    let mut position = 0;
    let mut tied = head;
    loop {
        if tied.boundary {
            return Err(format!(
                "a shallow boundary inside HEAD's committer second {}s hides ancestors that \
                 order it; fetch history past that second",
                head.seconds
            ));
        }
        let Some(parent) = commits.next().transpose()? else {
            break;
        };
        if head.seconds < parent.seconds {
            return Err(format!(
                "HEAD committer instant {}s is earlier than its first-parent ancestor's {}s",
                head.seconds, parent.seconds
            ));
        }
        if head.seconds > parent.seconds {
            break;
        }
        position += 1;
        if position >= SOURCE_SECOND_POSITIONS {
            break;
        }
        tied = parent;
    }
    SourceMinor::new(head.seconds, position).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_790_000_001;

    fn commit(seconds: i64) -> Result<FirstParent, String> {
        Ok(FirstParent {
            seconds,
            boundary: false,
        })
    }

    fn boundary(seconds: i64) -> Result<FirstParent, String> {
        Ok(FirstParent {
            seconds,
            boundary: true,
        })
    }

    fn minor(
        history: impl IntoIterator<Item = Result<FirstParent, String>>,
    ) -> Result<u64, String> {
        minor_from_first_parents(history).map(SourceMinor::get)
    }

    #[test]
    fn the_minor_is_the_head_second_and_its_same_second_position() {
        assert_eq!(
            minor([commit(SECOND), commit(SECOND - 1)]),
            Ok(17_900_000_010)
        );
        assert_eq!(
            minor([commit(SECOND)]),
            Ok(17_900_000_010),
            "a root has no ties"
        );
        assert_eq!(
            minor([commit(SECOND), commit(SECOND), commit(SECOND - 1)]),
            Ok(17_900_000_011)
        );
        assert_eq!(
            minor([commit(SECOND), commit(SECOND)]),
            Ok(17_900_000_011),
            "a tied root keeps its position"
        );
        let tenth = std::iter::repeat_n(SECOND, 10)
            .chain([SECOND - 1])
            .map(commit);
        assert_eq!(minor(tenth), Ok(17_900_000_019));
    }

    #[test]
    fn an_eleventh_same_second_commit_and_a_backward_clock_refuse() {
        let eleventh = std::iter::repeat_n(SECOND, 11)
            .chain([SECOND - 1])
            .map(commit);
        assert!(minor(eleventh).is_err());
        assert!(minor([commit(SECOND - 1), commit(SECOND)]).is_err());
        assert!(minor([commit(SECOND), commit(SECOND), commit(SECOND + 1)]).is_err());
        assert!(minor([]).is_err());
        assert!(minor([commit(-1)]).is_err());
    }

    #[test]
    fn a_shallow_boundary_ends_history_only_before_the_head_second() {
        assert_eq!(
            minor([commit(SECOND), commit(SECOND), boundary(SECOND - 1)]),
            minor([commit(SECOND), commit(SECOND), commit(SECOND - 1)]),
        );
        assert!(
            minor([boundary(SECOND)]).is_err(),
            "a depth-1 checkout refuses"
        );
        assert!(minor([commit(SECOND), boundary(SECOND)]).is_err());
    }

    /// Only the tied prefix and the first earlier ancestor are read, however
    /// long the history behind them, and an unreadable commit inside that
    /// prefix refuses rather than ordering short.
    #[test]
    fn the_walk_reads_no_history_past_the_first_earlier_ancestor() {
        let mut read = 0;
        let history = [SECOND, SECOND, SECOND - 1]
            .into_iter()
            .chain(std::iter::repeat(SECOND - 2))
            .inspect(|_| read += 1)
            .map(commit);
        assert_eq!(minor(history), Ok(17_900_000_011));
        assert_eq!(read, 3);
        assert_eq!(
            minor([commit(SECOND), Err("commit unreadable".to_owned())]),
            Err("commit unreadable".to_owned())
        );
    }
}

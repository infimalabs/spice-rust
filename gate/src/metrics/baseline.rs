//! The committed bytes a magic-number delta is measured against.
//!
//! A magic-number regression is a comparison-adjacent literal that is in a file
//! now and was not in the same file at `HEAD`. The committed side is therefore
//! half the measurement, not an optimization: a scan handed no baseline reports
//! every literal a file already had as newly introduced, which is why the arm
//! reported the missing input rather than measuring until this existed.
//!
//! The bytes are read once, in the moment the battery source is captured, and
//! retained for the run. `HEAD` moves. A read deferred to the moment the arm
//! runs would compare against a commit the report never names, and two paths in
//! one arm could be compared against two different commits.
//!
//! Only the selected source classes are read. The whole committed tree is
//! walked either way, but a repository's committed prose, fixtures, and images
//! are never a magic-number baseline, and holding them would grow the retained
//! capture without any measurement being able to reach them.

use std::collections::BTreeMap;
use std::path::Path;

use crate::tree::{TreeError, normalized_component, open_repository, verified_object};

/// Git's mode for a directory entry within a tree.
const TREE_MODE: u16 = 0o040000;
/// Git's modes for the regular and executable blobs a source file can be.
const BLOB_MODES: [u16; 2] = [0o100644, 0o100755];

/// The committed baseline a metric capture reached, or why it did not.
///
/// Carried as evidence rather than as a capture failure, so a repository whose
/// committed state cannot be read still produces a battery: the magic arm
/// reports the input it lacks, and the other three arms are unaffected by it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaselineCapture {
    /// The committed texts read from one named commit.
    Reached(CommittedTexts),
    /// The committed state could not be read, and why.
    Unreachable(String),
}

impl BaselineCapture {
    /// Read every selected committed text, or retain why none could be read.
    #[must_use]
    pub fn capture(repository: &Path, suffixes: &[String]) -> Self {
        match CommittedTexts::read(repository, suffixes) {
            Ok(baselines) => Self::Reached(baselines),
            Err(error) => Self::Unreachable(error.to_string()),
        }
    }
}

/// Every selected text captured from one committed `HEAD`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommittedTexts {
    commit: Option<String>,
    texts: BTreeMap<String, String>,
}

impl CommittedTexts {
    /// The committed text for one path, or `None` when `HEAD` does not carry it.
    ///
    /// The two answers are kept apart deliberately. A path absent from `HEAD` is
    /// not a path committed empty: the first has no baseline for a delta to be
    /// taken against, and the second has one that happens to hold no literals.
    #[must_use]
    pub fn text(&self, path: &str) -> Option<&str> {
        self.texts.get(path).map(String::as_str)
    }

    /// The commit these texts were read from, absent on an unborn `HEAD`.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Walk one committed tree, retaining the text of every selected path.
    ///
    /// A repository with no commit yet is read as an empty baseline naming no
    /// commit rather than refused. An unborn `HEAD` is a real repository state,
    /// and every selected path then honestly has no committed bytes behind it.
    pub(crate) fn read(repository: &Path, suffixes: &[String]) -> Result<Self, TreeError> {
        let repo = open_repository(repository)?;
        let head = repo.head().map_err(|source| TreeError::Repository {
            path: repository.to_path_buf(),
            evidence: source.to_string(),
        })?;
        let Some(commit_id) = head.id().map(gix::Id::detach) else {
            return Ok(Self::default());
        };
        let mut texts = BTreeMap::new();
        collect(
            &repo,
            commit_tree(&repo, commit_id)?,
            "",
            suffixes,
            &mut texts,
        )?;
        Ok(Self {
            commit: Some(commit_id.to_string()),
            texts,
        })
    }
}

/// Whether one path is a source class the magic scan is declared over.
///
/// Selection is stated once, here, because two callers depend on the same
/// answer: the walk that retains committed text, and the arm that offers paths
/// to the scanner. Two spellings could disagree, and a path the arm measured
/// but the walk skipped would be reported as having no committed baseline when
/// what really happened is that nobody read it.
#[must_use]
pub fn selects(path: &str, suffixes: &[String]) -> bool {
    let Some(extension) = Path::new(path).extension() else {
        return false;
    };
    let extension = format!(".{}", extension.to_string_lossy().to_ascii_lowercase());
    suffixes
        .iter()
        .any(|suffix| suffix.to_ascii_lowercase() == extension)
}

fn commit_tree(
    repo: &gix::Repository,
    commit_id: gix::ObjectId,
) -> Result<gix::ObjectId, TreeError> {
    let object = verified_object(repo, commit_id, "HEAD")?;
    if object.kind != gix::objs::Kind::Commit {
        return Err(TreeError::RevisionNotCommit {
            revision: "HEAD".to_owned(),
            actual: object.kind.to_string(),
        });
    }
    let commit = object
        .try_into_commit()
        .map_err(|source| TreeError::RevisionNotCommit {
            revision: "HEAD".to_owned(),
            actual: source.actual.to_string(),
        })?;
    commit
        .tree_id()
        .map(gix::Id::detach)
        .map_err(|source| TreeError::MalformedObject {
            object: commit_id.to_string(),
            evidence: source.to_string(),
        })
}

/// Recurse one committed tree, reading the text of every selected blob.
///
/// Entries that are neither a tree nor a blob are passed over rather than
/// refused. A committed symlink or submodule carries no source text of its own,
/// so it has no baseline to offer; refusing here would deny a whole repository
/// its magic measurement over an entry no scan could ever have read.
fn collect(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &str,
    suffixes: &[String],
    texts: &mut BTreeMap<String, String>,
) -> Result<(), TreeError> {
    let label = if prefix.is_empty() { "." } else { prefix };
    let object = verified_object(repo, tree_id, label)?;
    let tree = object
        .try_into_tree()
        .map_err(|source| TreeError::UnexpectedObject {
            path: label.to_owned(),
            object: tree_id.to_string(),
            expected: "tree",
            actual: source.actual.to_string(),
        })?;
    let entries = tree
        .iter()
        .map(|entry| {
            entry
                .map(|entry| (entry.filename().to_vec(), entry.object_id(), entry.mode()))
                .map_err(|source| TreeError::MalformedObject {
                    object: tree_id.to_string(),
                    evidence: source.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (name, id, mode) in entries {
        let name = normalized_component(&name)?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let mode = mode.value();
        if mode == TREE_MODE {
            collect(repo, id, &path, suffixes, texts)?;
        } else if BLOB_MODES.contains(&mode) && selects(&path, suffixes) {
            let text = blob_text(repo, id, &path)?;
            texts.insert(path, text);
        }
    }
    Ok(())
}

/// One committed blob's text.
///
/// Bytes that are not UTF-8 are replaced rather than refused, because the
/// current side of the same comparison is read the same way. A baseline that
/// refused here would leave a file with one stray byte looking like a file with
/// no committed state at all, and every literal it already had would read as
/// newly introduced.
fn blob_text(repo: &gix::Repository, id: gix::ObjectId, path: &str) -> Result<String, TreeError> {
    let object = verified_object(repo, id, path)?;
    if object.kind != gix::objs::Kind::Blob {
        return Err(TreeError::UnexpectedObject {
            path: path.to_owned(),
            object: id.to_string(),
            expected: "blob",
            actual: object.kind.to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&object.data).into_owned())
}

//! The committed bytes a per-file delta is measured against.
//!
//! A regression is content that is in a file now and was not in the same file
//! in the commit the pending one is being written on top of. The committed side
//! is therefore half the measurement, not an optimization: a scan handed no
//! baseline reports everything a file already had as newly introduced, which is
//! why the arm reported the missing input rather than measuring until this
//! existed.
//!
//! The pending commit has as many parents as `HEAD` plus `MERGE_HEAD`, and all
//! of them are read. Comparing against the first parent alone would report every
//! file the second parent brings in as newly written by the merging lane, which
//! is the one thing a merge demonstrably did not do. Each current file is
//! therefore measured against the parent that carries it verbatim when one does,
//! and against the nearest parent that carries the path at all otherwise. If no
//! parent carries the path, the merge base is the last baseline: both sides can
//! delete a file that the merge then restores, and treating that as a new path
//! would erase the delta the shared history still supplies.
//!
//! The guarantee is for files a merge left alone, and it stops there. A file the
//! merging lane edited matches no parent, so it is measured against the nearest
//! one and may report content the other side had already committed. That lane is
//! presenting an edited file as its own work, and answering otherwise would mean
//! asking every parent separately and calling introduced only what none of them
//! held — a wider contract than a merge needs to be able to land.
//!
//! The bytes are read once, in the moment the battery source is captured, and
//! retained for the run. `HEAD` moves. A read deferred to the moment the arm
//! runs would compare against a commit the report never names, and two paths in
//! one arm could be compared against two different commits.
//!
//! Only the selected source classes are read. The whole committed tree is
//! walked either way, but a repository's committed prose, fixtures, and images
//! are never a baseline, and holding them would grow the retained capture
//! without any measurement being able to reach them.

use std::collections::BTreeMap;
use std::fs;
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
    /// The selected committed blobs read from the pending commit's ancestry.
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

/// Every selected blob captured from each parent of the pending commit.
///
/// The parents are held in Git's own order: `HEAD` first, then each
/// `MERGE_HEAD` line. "Nearest" therefore means earliest in this vector, which
/// is the same order `git log` and `git diff` name a merge's parents in. A
/// pending merge also retains its one octopus merge base for the path no parent
/// carries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommittedTexts {
    commit: Option<String>,
    parents: Vec<BTreeMap<String, CommittedText>>,
    merge_base: Option<BTreeMap<String, CommittedText>>,
}

/// One selected blob retained both losslessly and as the text metrics read it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommittedText {
    bytes: Vec<u8>,
    text: String,
}

impl CommittedTexts {
    /// The committed text one current source is measured against.
    ///
    /// A parent carrying `path` byte-identical to `current` is returned outright
    /// and in preference to every other parent, because a pending commit that
    /// changed nothing about a file cannot have introduced anything into it. On
    /// a merge that is what keeps the second parent's content from reading as
    /// newly authored; on an ordinary commit there is one parent, and the
    /// preference resolves to it either way.
    ///
    /// Otherwise the nearest parent that carries the path is the baseline. If
    /// no parent carries it, the pending merge's base is consulted exactly once;
    /// `None` means neither a parent nor that base carries it. The absent answer
    /// is kept apart deliberately from committed empty text: the first has no
    /// baseline for a delta to be taken against, and the second has one that
    /// happens to hold nothing.
    #[must_use]
    pub fn baseline(&self, path: &str, current: &[u8]) -> Option<&str> {
        let mut nearest = None;
        for texts in &self.parents {
            let Some(committed) = texts.get(path) else {
                continue;
            };
            if committed.bytes == current {
                return Some(&committed.text);
            }
            nearest = nearest.or(Some(committed.text.as_str()));
        }
        nearest.or_else(|| {
            self.merge_base
                .as_ref()?
                .get(path)
                .map(|committed| committed.text.as_str())
        })
    }

    /// The commit these texts were read from, absent on an unborn `HEAD`.
    #[must_use]
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// Whether a pending merge left this file byte-for-byte as a parent had it.
    ///
    /// This is an authorship question, not a delta one, and it is asked only of
    /// a merge. An ordinary commit's one parent is the tree it is being written
    /// on top of, so an unchanged file there is still content the committing
    /// lane is presenting as its own and is measured as such. A merge presents
    /// both sides, and the side it did not touch was already measured by the
    /// lane that wrote it: charging it again bills the merging lane for a limit
    /// another lane's commit passed.
    #[must_use]
    pub fn merge_inherits(&self, path: &str, current: &[u8]) -> bool {
        self.parents.len() > 1
            && self
                .parents
                .iter()
                .any(|texts| texts.get(path).is_some_and(|text| text.bytes == current))
    }

    /// Walk each parent and the pending merge base, retaining selected blobs.
    ///
    /// A repository with no commit yet is read as an empty baseline naming no
    /// commit rather than refused. An unborn `HEAD` is a real repository state,
    /// and every selected path then honestly has no committed bytes behind it.
    /// An unborn `HEAD` also cannot be mid-merge, so no `MERGE_HEAD` is sought.
    pub(crate) fn read(repository: &Path, suffixes: &[String]) -> Result<Self, TreeError> {
        let repo = open_repository(repository)?;
        let head = repo.head().map_err(|source| TreeError::Repository {
            path: repository.to_path_buf(),
            evidence: source.to_string(),
        })?;
        let Some(commit_id) = head.id().map(gix::Id::detach) else {
            return Ok(Self::default());
        };
        let merging = merge_heads(&repo)?;
        let merge_base = if merging.is_empty() {
            None
        } else {
            let base = repo
                .merge_base_octopus(std::iter::once(commit_id).chain(merging.iter().copied()))
                .map_err(|source| TreeError::Repository {
                    path: repo.path().join("MERGE_HEAD"),
                    evidence: format!("cannot resolve the pending merge base: {source}"),
                })?
                .detach();
            Some(parent_texts(&repo, base, "merge base", suffixes)?)
        };
        let mut parents = vec![parent_texts(&repo, commit_id, "HEAD", suffixes)?];
        for (index, merging) in merging.into_iter().enumerate() {
            let revision = format!("MERGE_HEAD line {}", index + 1);
            parents.push(parent_texts(&repo, merging, &revision, suffixes)?);
        }
        Ok(Self {
            commit: Some(commit_id.to_string()),
            parents,
            merge_base,
        })
    }
}

/// Every commit `MERGE_HEAD` names, in file order, empty when none is pending.
///
/// The file is read directly rather than resolved as a revision because it is a
/// list: an octopus merge writes one object id per line, and resolving the name
/// would silently answer with only the first. A line that is not an object id
/// refuses the read rather than being passed over, because the alternative is a
/// merge measured against fewer parents than it actually has.
fn merge_heads(repo: &gix::Repository) -> Result<Vec<gix::ObjectId>, TreeError> {
    let path = repo.path().join("MERGE_HEAD");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(TreeError::Repository {
                path,
                evidence: error.to_string(),
            });
        }
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            gix::ObjectId::from_hex(line.as_bytes()).map_err(|source| TreeError::Repository {
                path: path.clone(),
                evidence: format!("MERGE_HEAD names {line:?}, which is not an object id: {source}"),
            })
        })
        .collect()
}

/// One ancestor's selected blobs, read from the commit that names its tree.
fn parent_texts(
    repo: &gix::Repository,
    commit_id: gix::ObjectId,
    revision: &str,
    suffixes: &[String],
) -> Result<BTreeMap<String, CommittedText>, TreeError> {
    let mut texts = BTreeMap::new();
    collect(
        repo,
        commit_tree(repo, commit_id, revision)?,
        "",
        suffixes,
        &mut texts,
    )?;
    Ok(texts)
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
    revision: &str,
) -> Result<gix::ObjectId, TreeError> {
    let object = verified_object(repo, commit_id, revision)?;
    if object.kind != gix::objs::Kind::Commit {
        return Err(TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
            actual: object.kind.to_string(),
        });
    }
    let commit = object
        .try_into_commit()
        .map_err(|source| TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
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
    texts: &mut BTreeMap<String, CommittedText>,
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

/// One committed blob's lossless identity and text projection.
///
/// Bytes that are not UTF-8 are replaced only in the projection the text
/// metrics consume. Exact bytes remain beside it for parent selection and merge
/// inheritance; decoding both sides lossily would let different invalid byte
/// sequences collapse to the same text and exempt a file the merge really
/// changed.
fn blob_text(
    repo: &gix::Repository,
    id: gix::ObjectId,
    path: &str,
) -> Result<CommittedText, TreeError> {
    let object = verified_object(repo, id, path)?;
    if object.kind != gix::objs::Kind::Blob {
        return Err(TreeError::UnexpectedObject {
            path: path.to_owned(),
            object: id.to_string(),
            expected: "blob",
            actual: object.kind.to_string(),
        });
    }
    let bytes = object.data.to_vec();
    Ok(CommittedText {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        bytes,
    })
}

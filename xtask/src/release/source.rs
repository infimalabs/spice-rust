//! Immutable-source guard around release publication.

use super::*;
pub(super) struct SourceGuard {
    pub(super) commit: String,
    pub(super) digest: Digest,
    pub(super) git: PathBuf,
}

impl SourceGuard {
    pub(super) fn capture<E: Executor>(executor: &E, git: &Path) -> Result<Self, TargetBuildError> {
        let commit = capture_commit(executor, git)?;
        verify_checkout(executor, git)?;
        let digest = Digest::compute(Domain::DistSourceCommit, commit.as_bytes());
        Ok(Self {
            commit,
            digest,
            git: git.to_path_buf(),
        })
    }

    pub(super) fn verify<E: Executor>(&self, executor: &E) -> Result<(), TargetBuildError> {
        let observed = capture_commit(executor, &self.git)?;
        if observed != self.commit {
            return Err(TargetBuildError::SourceMoved {
                expected: self.commit.clone(),
                observed,
            });
        }
        verify_checkout(executor, &self.git)
    }
}

fn capture_commit<E: Executor>(executor: &E, git: &Path) -> Result<String, TargetBuildError> {
    let record = successful(executor.execute(CommandSpec::new(
        "source-commit",
        git,
        ["rev-parse", "--verify", "HEAD^{commit}"],
    ))?)?;
    let commit = one_line(&record.stdout, "Git source commit")?;
    if !matches!(commit.len(), 40 | 64)
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TargetBuildError::Observation {
            label: "Git source commit",
            observed: commit,
        });
    }
    Ok(commit)
}

/// The only untracked paths a release build is allowed to have produced.
const RELEASE_OUTPUT_ROOTS: [&str; 2] = ["dist/stage/objects/", "dist/stage/targets/"];
const RELEASE_MANIFEST: &str = "dist/stage/release/manifest.json";

/// The porcelain code and separator an untracked entry is reported under.
const UNTRACKED: &str = "?? ";

pub(super) fn verify_checkout<E: Executor>(
    executor: &E,
    git: &Path,
) -> Result<(), TargetBuildError> {
    // One porcelain status answers both questions, and it is the only spelling
    // that answers them truthfully. `git diff-index` trusts the index stat
    // cache, so a checkout whose files are byte-identical to HEAD still reads
    // as modified whenever that cache is stale -- exactly what a fresh clone
    // observed through another kernel's view of the same bytes produces, which
    // is the sealed proof image reading a mounted host checkout. `git status`
    // refreshes the index before it compares, so it reports content.
    //
    // Release-path only, and cheap enough that its placement never had to be
    // argued: one invocation costs 10ms measured over this worktree, against a
    // target build it guards.
    let status = successful(executor.execute(CommandSpec::new(
        "source-status",
        git,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    ))?)?;
    let status = text(&status.stdout, "source status inventory")?;
    for entry in status.lines() {
        let Some(path) = entry.strip_prefix(UNTRACKED) else {
            return Err(TargetBuildError::SourceDirty(format!(
                "tracked source differs from HEAD: {entry}"
            )));
        };
        if !release_output(path) {
            return Err(TargetBuildError::SourceDirty(format!(
                "untracked source path is outside release output: {path}"
            )));
        }
    }
    // Status trusts the assume-unchanged and skip-worktree index flags, so a
    // tracked file under either can differ from HEAD while status stays
    // silent. No release flow sets them, so an entry carrying one is local
    // state whose bytes status cannot vouch for, and it refuses. Reading the
    // flags writes nothing, leaving the caller's index and configuration as
    // found.
    let flags = successful(executor.execute(CommandSpec::new(
        "source-index-flags",
        git,
        ["ls-files", "-v", "-z"],
    ))?)?;
    let flags = text(&flags.stdout, "source index flags")?;
    for entry in flags.split_terminator('\0') {
        let (tag, path) = entry.split_at_checked(2).unwrap_or((entry, ""));
        let hidden = match tag.as_bytes().first() {
            Some(b'S' | b's') => "skip-worktree",
            Some(byte) if byte.is_ascii_lowercase() => "assume-unchanged",
            _ => continue,
        };
        return Err(TargetBuildError::SourceDirty(format!(
            "index flag {hidden} hides tracked source from status: {path}"
        )));
    }
    Ok(())
}

fn release_output(path: &str) -> bool {
    RELEASE_OUTPUT_ROOTS
        .iter()
        .any(|root| path.starts_with(root))
        || path == RELEASE_MANIFEST
}

pub(super) struct GuardedPublisher<'a, E> {
    pub(super) inner: RootedFiles,
    pub(super) source: &'a SourceGuard,
    pub(super) required: &'a RequiredPins,
    pub(super) programs: &'a Programs,
    pub(super) target: ReleaseTarget,
    pub(super) executor: &'a E,
}

impl<E: Executor> GuardedPublisher<'_, E> {
    /// The source and toolchain pins still match those the build began with.
    fn verify_unmoved(&self) -> Result<(), String> {
        self.source
            .verify(self.executor)
            .map_err(|error| error.to_string())?;
        let observed = observe_pins(self.executor, self.programs, self.target)
            .map_err(|error| error.to_string())?;
        self.required
            .verify(&observed)
            .map_err(|error| error.to_string())
    }
}

impl<E: Executor> RootedPublish for GuardedPublisher<'_, E> {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), atomic::Error> {
        if path.starts_with("targets") && path.file_name() == Some(OsStr::new("payload.json")) {
            self.verify_unmoved().map_err(|reason| {
                atomic::Error::Refused(atomic::Refusal::Withheld {
                    path: path.to_path_buf(),
                    reason,
                })
            })?;
        }
        self.inner.publish(path, mode, bytes)
    }
}

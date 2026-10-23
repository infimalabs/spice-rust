//! Git history transitions bound to task possession and completion.
//!
//! This is the one executable Git path owned by Rust task
//! control. Claiming safely advances a clean lane, capture
//! deliberately binds loose commits without advancing, and
//! completion integrates then publishes before board state may
//! move. Every branch update materializes the target tree before
//! a compare-and-swap ref update. A small per-worktree transaction
//! record makes that ordering restart-safe instead of leaving a
//! half-materialized checkout.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wire::body::GitBoundary;

use crate::repository::Repository;

const PUBLISH_RACE_RETRY_LIMIT: usize = 5;
const TRANSACTION_FILE: &str = "spice-task-boundary";
const MERGE_STATE_FILES: &[&str] = &["ORIG_HEAD", "MERGE_MODE", "MERGE_MSG", "MERGE_HEAD"];
const LOCAL_COMMAND_LIMIT: Duration = Duration::from_secs(10);
const NETWORK_COMMAND_LIMIT: Duration = Duration::from_secs(30);
const OUTPUT_DRAIN_LIMIT: Duration = Duration::from_secs(1);
const PROCESS_GROUP_GRACE: Duration = Duration::from_millis(100);
const PROCESS_GROUP_REAP_LIMIT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
/// One repair direction for work that has not entered a task boundary.
///
/// Every claim path uses this exact vocabulary so repository state, rather
/// than a sequence of refusals, determines the next command.
pub const UNOWNED_WORK_REMEDY: &str = concat!(
    "preserve unowned paths and commits with `spice task capture HANDLE` for an ",
    "existing ready execution task or `spice task capture --title TITLE --project ",
    "PROJECT` for new execution work",
);

/// Task facts used to build one durable integration commit message.
#[derive(Clone, Copy, Debug)]
pub struct Metadata<'a> {
    pub label: &'a str,
    pub title: &'a str,
    pub project: &'a str,
    pub phase: &'a str,
    pub actor: &'a str,
}

/// A loose-commit boundary ready to be attached to a claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub boundary: GitBoundary,
    pub subject: String,
}

/// Whether a failure is repair work or an environment fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    Refusal,
    Fault,
}

/// One boundary failure and its executable recovery direction.
#[derive(Debug)]
pub struct Error {
    class: ErrorClass,
    reason: &'static str,
    detail: String,
    remedy: String,
}

impl Error {
    fn refusal(reason: &'static str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Refusal,
            reason,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    fn fault(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Fault,
            reason,
            detail: detail.into(),
            remedy: concat!(
                "repair the named Git command or repository state ",
                "and retry",
            )
            .to_owned(),
        }
    }

    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        self.class
    }

    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    #[must_use]
    pub fn remedy(&self) -> &str {
        &self.remedy
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl StdError for Error {}

#[derive(Clone, Debug)]
struct Target {
    remote: String,
    baseline: String,
    branch: String,
}

#[derive(Debug)]
struct GitOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

impl GitOutput {
    fn success(&self) -> bool {
        self.status == 0
    }

    fn detail(&self, action: &str) -> String {
        let output = [&self.stdout, &self.stderr]
            .into_iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if output.is_empty() {
            format!("could not {action} (git exit {})", self.status)
        } else {
            format!("could not {action} (git exit {})\n{output}", self.status)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Pending {
    Move {
        expected: String,
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dirty: Option<snapshot::Rollback>,
    },
    Conflict {
        agent: String,
        upstream: String,
        previous: BTreeMap<String, Option<String>>,
    },
    Resolution {
        expected: String,
        target: String,
        tree: String,
    },
    Integration {
        expected: String,
        target: String,
    },
    Snapshot {
        commit: String,
    },
}

/// Inspect loose commits without moving the lane, its branch, or dirty paths.
pub fn capture(repository: &Repository) -> Result<Capture, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    let target = resolve_target(root)?.ok_or_else(|| {
        Error::refusal(
            "git::capture-no-baseline",
            "task capture found no upstream-backed task baseline",
            concat!(
                "configure branch tracking (branch.<lane>.remote and ",
                "branch.<lane>.merge) before capturing loose commits",
            ),
        )
    })?;
    fetch(root, &target)?;
    let upstream_head = required_read(root, &["rev-parse", &target.baseline], "read baseline")?;
    let head = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    let local_commits = ahead_count(root, &target.baseline)?;
    if local_commits == 0 {
        return Err(Error::refusal(
            "git::nothing-to-capture",
            "no local commits are ahead of the task baseline",
            "make and commit the intended change before task capture",
        ));
    }
    let subject = required_read(root, &["log", "-1", "--format=%s"], "read commit subject")?;
    Ok(Capture {
        boundary: boundary(
            head.clone(),
            head,
            Some(&target.baseline),
            Some(upstream_head),
            local_commits,
        ),
        subject,
    })
}

/// Resolve `revision` to the full id of a commit reachable from the lane's
/// HEAD, so a Plan's disposition cites only history that already landed.
pub fn resolve_landed_commit(repository: &Repository, revision: &str) -> Result<String, Error> {
    let root = repository.worktree_root();
    let commit = try_read(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?
    .ok_or_else(|| {
        Error::refusal(
            "git::commit-unknown",
            revision,
            "name an existing task handle or a commit this lane can resolve",
        )
    })?;
    if !is_ancestor(root, &commit, "HEAD")? {
        return Err(Error::refusal(
            "git::commit-unlanded",
            commit,
            "name a commit reachable from this lane's HEAD",
        ));
    }
    Ok(commit)
}

/// Whether the lane has tracked, staged, or non-ignored untracked changes.
pub fn has_dirty_work(repository: &Repository) -> Result<bool, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    required_read(root, &["status", "--porcelain"], "inspect worktree")
        .map(|status| !status.is_empty())
}

/// A release-side Git transition prepared before its board fact commits.
///
/// The journal retains the exact commit identity. If the guarded board write
/// refuses, `rollback` restores that commit to the lane.
#[derive(Debug)]
pub struct ParkedRelease {
    parked: wire::body::TaskParkedCommits,
    root: PathBuf,
    before: String,
    snapshot: Option<snapshot::Captured>,
}

impl ParkedRelease {
    /// Exact durable payload written beside `ClaimFreed`.
    #[must_use]
    pub const fn parked(&self) -> &wire::body::TaskParkedCommits {
        &self.parked
    }

    /// Undo a prepared release after its board write failed.
    pub fn rollback(self) -> Result<(), Error> {
        let current = required_read(&self.root, &["rev-parse", "HEAD"], "read parked lane HEAD")?;
        if current != self.before {
            materialize_and_update(
                &self.root,
                &self.before,
                &current,
                "restore lane after refused task release",
            )?;
        }
        if let Some(snapshot) = &self.snapshot {
            snapshot.restore(&self.root)?;
        }
        Ok(())
    }
}

/// Park unpublished commits and dirty work without attaching preservation refs.
///
/// The lane returns to the newest commit it shares with published history.
/// Dirty work is saved as a separate dangling commit and replayed uncommitted.
/// Open merges and paused replays must be resolved before releasing possession.
pub fn prepare_free(
    repository: &Repository,
    opening: Option<&GitBoundary>,
) -> Result<Option<ParkedRelease>, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    snapshot::require_releasable(root)?;
    let before = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    let snapshot = snapshot::capture(root, &before)?;
    let published = match published_reference(root, opening)? {
        Some(published) => published,
        None if snapshot.is_some() => before.clone(),
        None => return Ok(None),
    };
    let commits = rev_list(root, &published, &before)?;
    if commits.is_empty() && snapshot.is_none() {
        return Ok(None);
    }
    let baseline = required_read(
        root,
        &["merge-base", &published, &before],
        "find the newest published lane commit",
    )?;
    if let Some(snapshot) = &snapshot {
        materialize_release(root, &baseline, &before, snapshot)?;
    } else {
        materialize_and_update(root, &baseline, &before, "return lane to published history")?;
    }
    Ok(Some(ParkedRelease {
        parked: wire::body::TaskParkedCommits {
            baseline,
            head: before.clone(),
            commits,
            snapshot: snapshot
                .as_ref()
                .map(|snapshot| snapshot.commit().to_owned()),
        },
        root: root.to_path_buf(),
        before,
        snapshot,
    }))
}

/// Validate planning history before parking in-flight edits for the next phase.
pub fn prepare_planning_free(
    repository: &Repository,
    phase: &str,
    opening: Option<&GitBoundary>,
) -> Result<Option<ParkedRelease>, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    snapshot::require_releasable(root)?;
    require_planning_opening(phase, false, opening)?;
    let head = required_read(root, &["rev-parse", "HEAD"], "read planning lane HEAD")?;
    prepare_completion_target(root, phase, opening, &head, None)?;
    prepare_free(repository, opening)
}

fn published_reference(
    root: &Path,
    opening: Option<&GitBoundary>,
) -> Result<Option<String>, Error> {
    let Some(target) = resolve_target(root)? else {
        return Ok(opening.map(|opening| opening.head.clone()));
    };
    Ok(Some(required_read(
        root,
        &["rev-parse", &target.baseline],
        "read published upstream",
    )?))
}

fn rev_list(root: &Path, baseline: &str, head: &str) -> Result<Vec<String>, Error> {
    let listed = required_read(
        root,
        &["rev-list", "--reverse", &format!("{baseline}..{head}")],
        "list task commits",
    )?;
    Ok(listed.lines().map(str::to_owned).collect())
}

/// Safely advance a clean lane before allocator possession is minted.
pub fn prepare_claim(repository: &Repository) -> Result<GitBoundary, Error> {
    prepare_claim_observed(repository, LooseCommits::Refuse, || Ok(()), |_| Ok(()))
}

/// Measure a named claim, adopting loose commits or advancing a clean lane.
///
/// Explicit capture and steal use the allocator's opening-boundary transaction,
/// but unlike allocation they may intentionally attach already-committed work.
pub fn prepare_explicit_claim(repository: &Repository) -> Result<ClaimPreparation, Error> {
    let boundary = prepare_claim_observed(repository, LooseCommits::Adopt, || Ok(()), |_| Ok(()))?;
    Ok(ClaimPreparation {
        boundary,
        restore: None,
    })
}
/// Claim-side Git work prepared before the guarded claim fact commits.
#[derive(Debug)]
pub struct ClaimPreparation {
    pub boundary: GitBoundary,
    restore: Option<ParkedRestore>,
}

#[derive(Debug)]
struct ParkedRestore {
    root: PathBuf,
    before: String,
    restored: String,
    report: wire::intent::ClaimTransplant,
}

impl ClaimPreparation {
    /// What the claim did with the task's parked commits, when it had any.
    #[must_use]
    pub fn transplant(&self) -> Option<&wire::intent::ClaimTransplant> {
        self.restore.as_ref().map(|restore| &restore.report)
    }

    /// Return the lane to its pre-claim HEAD after a guarded board refusal.
    pub fn rollback(self) -> Result<(), Error> {
        let Some(restore) = self.restore else {
            return Ok(());
        };
        let current = required_read(
            &restore.root,
            &["rev-parse", "HEAD"],
            "read restored lane HEAD",
        )?;
        if current != restore.restored {
            return Err(Error::fault(
                "git::parked-rollback-head-moved",
                format!(
                    "restored lane moved from {} to {} before claim rollback",
                    restore.restored, current
                ),
            ));
        }
        transplant::abandon(
            &restore.root,
            &restore.before,
            "restore lane after refused parked claim",
        )
    }
}

/// Advance a clean lane and replay one task's parked commits onto it.
///
/// The commits are cherry-picked, oldest first, onto whatever HEAD the claim
/// fast-forwarded to. A conflict does not refuse the claim: Git's cherry-pick
/// stays paused in the worktree for the claimant, and the report says so.
pub fn prepare_task_claim(
    repository: &Repository,
    parked: Option<&wire::body::TaskParkedCommits>,
) -> Result<ClaimPreparation, Error> {
    let root = repository.worktree_root();
    let before = required_read(root, &["rev-parse", "HEAD"], "read pre-claim lane HEAD")?;
    if let Some(parked) = parked {
        for commit in parked.commits.iter().chain(parked.snapshot.iter()) {
            transplant::require_object(root, commit)?;
        }
    }
    let mut boundary = prepare_claim(repository)?;
    let Some(parked) = parked else {
        return Ok(ClaimPreparation {
            boundary,
            restore: None,
        });
    };
    let replayed = match transplant::replay(
        root,
        &parked.commits,
        parked.snapshot.as_deref(),
        &boundary.head,
    ) {
        Ok(replayed) => replayed,
        Err(error) => {
            transplant::abandon(root, &before, "restore lane after failed parked claim")?;
            return Err(error);
        }
    };
    boundary.head = replayed.head.clone();
    boundary.integrated = replayed.head.clone();
    boundary.local_commits = u32::try_from(replayed.report.applied.len()).unwrap_or(u32::MAX);
    Ok(ClaimPreparation {
        boundary,
        restore: Some(ParkedRestore {
            root: root.to_path_buf(),
            before,
            restored: replayed.head,
            report: replayed.report,
        }),
    })
}

/// Safely advance a clean lane while durably observing the exact target first.
///
/// Dirty-handoff recovery uses the observation to journal the only HEAD the
/// ordinary claim boundary may install. The ordinary allocator calls the same
/// implementation with an inert observer, so recovery cannot acquire a subtly
/// different fetch, ancestry, fast-forward, or boundary law. Recovery's
/// preflight rechecks its sealed authority after target resolution and before
/// either the fetch or a no-remote boundary is published.
#[derive(Clone, Copy)]
enum LooseCommits {
    Refuse,
    Adopt,
}

fn prepare_claim_observed(
    repository: &Repository,
    loose_commits: LooseCommits,
    mut preflight: impl FnMut() -> Result<(), Error>,
    mut observe: impl FnMut(&GitBoundary) -> Result<(), Error>,
) -> Result<GitBoundary, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    require_clean_or(root, "task next", UNOWNED_WORK_REMEDY)?;
    let before = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    let target = resolve_target(root)?;
    preflight()?;
    let Some(target) = target else {
        let boundary = boundary(before.clone(), before, None, None, 0);
        observe(&boundary)?;
        return Ok(boundary);
    };
    fetch(root, &target)?;
    let upstream_head = required_read(root, &["rev-parse", &target.baseline], "read baseline")?;
    let ahead = ahead_count(root, &target.baseline)?;
    if ahead != 0 {
        if matches!(loose_commits, LooseCommits::Refuse) {
            return Err(Error::refusal(
                "git::claim-loose-commits",
                format!(
                    concat!(
                        "the lane has {ahead} local commit(s) outside a ",
                        "claimed task boundary",
                    ),
                    ahead = ahead
                ),
                UNOWNED_WORK_REMEDY,
            ));
        }
        let captured = boundary(
            before.clone(),
            before,
            Some(&target.baseline),
            Some(upstream_head),
            ahead,
        );
        observe(&captured)?;
        return Ok(captured);
    }
    let planned = boundary(
        upstream_head.clone(),
        upstream_head.clone(),
        Some(&target.baseline),
        Some(upstream_head.clone()),
        0,
    );
    observe(&planned)?;
    if before != upstream_head {
        if !is_ancestor(root, &before, &upstream_head)? {
            return Err(Error::refusal(
                "git::claim-diverged",
                format!(
                    concat!(
                        "lane HEAD {before} does not fast-forward to ",
                        "{upstream_head}",
                    ),
                    before = before,
                    upstream_head = upstream_head
                ),
                concat!(
                    "publish or capture the local history before ",
                    "claiming new work",
                ),
            ));
        }
        materialize_and_update(
            root,
            &upstream_head,
            &before,
            "advance lane to task baseline",
        )?;
    }
    Ok(planned)
}

/// GIT-04, at agent launch: advance opportunistically and never raise.
///
/// Delegates to the launch module, which documents the rules and every note
/// they can produce. It is spelled out here rather than re-exported because
/// this module is the enumerated git surface: a boundary that is not declared
/// beside its siblings is a boundary a reader has to go looking for.
///
/// Call it before reading the repository's configuration, as the resident's
/// occupant launch does. A fast-forward can change the executable configuration
/// a lane runs under, so advancing afterwards would compose an occupant from
/// configuration the tree no longer has.
///
/// The resident publishes the returned note in the launch transition and in
/// the durable session identity. GIT-04 leaves no failure to propagate, but its
/// safe no-op is still observable rather than becoming an invisible decision.
pub fn advance_at_launch(repository: &Repository) -> LaunchAdvance {
    launch::advance(repository.worktree_root())
}

/// Publish an allocator-authenticated phase, including an owned merge resolution.
pub fn publish_claimed(
    repository: &Repository,
    meta: Metadata<'_>,
    active: &board::Active<'_>,
    opening: Option<&GitBoundary>,
) -> Result<GitBoundary, Error> {
    publish_inner(repository, meta, Some(active), opening)
}

fn publish_inner(
    repository: &Repository,
    meta: Metadata<'_>,
    resolution: Option<&board::Active<'_>>,
    opening: Option<&GitBoundary>,
) -> Result<GitBoundary, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    let open_merge = try_read(root, &["rev-parse", "--verify", "MERGE_HEAD"])?;
    let resolution_state = open_merge.is_some() || resolution::has_state(root)?;
    if resolution_state && resolution.is_none() {
        require_no_open_merge(root, meta.label)?;
        return Err(Error::refusal(
            "git::merge-state-unrecognized",
            "task completion found partial or malformed Git merge state",
            "abort the malformed merge state and retry task completion from a clean lane",
        ));
    }
    if !resolution_state {
        require_clean_or(root, "task completion", &completion_dirty_remedy(meta))?;
    }
    let observed_head = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    require_planning_opening(meta.phase, resolution_state, opening)?;
    let resolution = match (resolution_state, resolution) {
        (true, Some(active)) => {
            let preflight = resolution::Preflight::capture(root)?;
            Some((active, preflight))
        }
        _ => None,
    };
    let open_merge_target = resolution.as_ref().and(open_merge.as_deref());
    let Some((target, upstream_head)) =
        prepare_completion_target(root, meta.phase, opening, &observed_head, open_merge_target)?
    else {
        return Ok(boundary(
            observed_head.clone(),
            observed_head,
            None,
            None,
            0,
        ));
    };
    if let Some((active, preflight)) = resolution {
        let local_commits = ahead_count(root, &upstream_head)?;
        let message = compose_message(meta);
        let integrated = resolution::complete(
            root,
            meta,
            active,
            resolution::Parents {
                agent: &observed_head,
                upstream: &upstream_head,
                preflight,
            },
            &message,
        )?;
        let admission = Admission::resolve(repository, &integrated)?;
        let (integrated, published_upstream) = publish_with_retry(
            root,
            &target,
            integrated.clone(),
            upstream_head,
            Publication {
                repository,
                agent_head: &integrated,
                message: &message,
                label: meta.label,
                admission: &admission,
            },
        )?;
        return Ok(boundary(
            observed_head,
            integrated,
            Some(&target.baseline),
            Some(published_upstream),
            local_commits,
        ));
    }
    let agent_head = if is_ancestor(root, &observed_head, &upstream_head)? {
        recovered_lane_head(root, &observed_head, meta)?.unwrap_or(observed_head)
    } else {
        observed_head
    };
    let local_commits = ahead_count(root, &upstream_head)?;
    let message = compose_message(meta);
    // Bind the authored tree and any non-empty lane command before the merge.
    // An empty lane declaration deliberately defers to each integrated tree.
    let admission = Admission::resolve(repository, &agent_head)?;
    let integrated = integrate(root, &agent_head, &upstream_head, &message, meta.label)?;
    let (integrated, published_upstream) = publish_with_retry(
        root,
        &target,
        integrated,
        upstream_head,
        Publication {
            repository,
            agent_head: &agent_head,
            message: &message,
            label: meta.label,
            admission: &admission,
        },
    )?;
    Ok(boundary(
        agent_head,
        integrated,
        Some(&target.baseline),
        Some(published_upstream),
        local_commits,
    ))
}

/// The holder's routes for dirty completion; planning phases admit no commit.
fn completion_dirty_remedy(meta: Metadata<'_>) -> String {
    if gate::lacks_commit_authority(meta.phase) {
        format!(
            "run spice task free {} to park the reported paths on the task",
            meta.label
        )
    } else {
        format!(
            concat!(
                "commit the reported paths through the pre-commit gate, or run ",
                "spice task free {label} to park them on the task uncommitted",
            ),
            label = meta.label
        )
    }
}

fn prepare_completion_target(
    root: &Path,
    phase: &str,
    opening: Option<&GitBoundary>,
    observed_head: &str,
    open_merge_target: Option<&str>,
) -> Result<Option<(Target, String)>, Error> {
    let Some(target) = resolve_target(root)? else {
        require_planning_synchronization(root, phase, opening, observed_head, None)?;
        return Ok(None);
    };
    let upstream_head = if let Some(upstream) = open_merge_target {
        upstream.to_owned()
    } else {
        fetch(root, &target)?;
        required_read(root, &["rev-parse", &target.baseline], "read baseline")?
    };
    require_planning_synchronization(
        root,
        phase,
        opening,
        observed_head,
        Some((&target, &upstream_head)),
    )?;
    Ok(Some((target, upstream_head)))
}

fn require_planning_opening(
    phase: &str,
    resolution_state: bool,
    opening: Option<&GitBoundary>,
) -> Result<(), Error> {
    if !gate::lacks_commit_authority(phase) {
        return Ok(());
    }
    if resolution_state {
        return Err(planning_implementation_refusal(format!(
            "task completion found merge-resolution state while the active claim is in {phase} phase",
        )));
    }
    let Some(opening) = opening else {
        return Err(planning_implementation_refusal(format!(
            "the active {phase} claim has no recorded opening Git boundary",
        )));
    };
    if opening.local_commits != 0 {
        return Err(planning_implementation_refusal(format!(
            "the {phase} claim opened with {} local commit(s)",
            opening.local_commits,
        )));
    }
    Ok(())
}

fn require_planning_synchronization(
    root: &Path,
    phase: &str,
    opening: Option<&GitBoundary>,
    observed_head: &str,
    target: Option<(&Target, &str)>,
) -> Result<(), Error> {
    if !gate::lacks_commit_authority(phase) {
        return Ok(());
    }
    let opening = opening.expect("planning opening was admitted before target resolution");
    if observed_head == opening.integrated {
        return Ok(());
    }
    let Some((target, upstream_head)) = target else {
        return Err(planning_implementation_refusal(format!(
            "lane HEAD moved from {phase} opening {} to {observed_head} without an upstream",
            opening.integrated,
        )));
    };
    if opening.upstream.as_deref() != Some(target.baseline.as_str())
        || opening.upstream_head.as_deref() != Some(opening.integrated.as_str())
    {
        return Err(planning_implementation_refusal(format!(
            "{phase} opening Git boundary does not match current upstream {}",
            target.baseline,
        )));
    }
    if observed_head != upstream_head || !is_ancestor(root, &opening.integrated, observed_head)? {
        return Err(planning_implementation_refusal(format!(
            "lane HEAD moved from {phase} opening {} to {observed_head}, but current upstream {} is {upstream_head}",
            opening.integrated, target.baseline,
        )));
    }
    let current_head = required_read(root, &["rev-parse", "HEAD"], "recheck planning lane HEAD")?;
    let current_upstream = required_read(
        root,
        &["rev-parse", &target.baseline],
        "recheck planning upstream",
    )?;
    if current_head != observed_head || current_upstream != upstream_head {
        return Err(planning_implementation_refusal(format!(
            "{phase} synchronization moved during admission: HEAD {observed_head} -> {current_head}, upstream {upstream_head} -> {current_upstream}",
        )));
    }
    Ok(())
}

fn planning_implementation_refusal(detail: impl Into<String>) -> Error {
    Error::refusal(
        "git::planning-implementation",
        detail,
        concat!(
            "this phase holds no commit authority: record planning findings with task add, task ",
            "modify, task depends, task note, or task artifact add; restore the lane ",
            "to its clean claim-opening commit and complete the phase, and only a claimed ",
            "implementation phase changes repository history",
        ),
    )
}

fn boundary(
    head: String,
    integrated: String,
    upstream: Option<&str>,
    upstream_head: Option<String>,
    local_commits: u32,
) -> GitBoundary {
    GitBoundary {
        head,
        integrated,
        upstream: upstream.map(str::to_owned),
        upstream_head,
        local_commits,
    }
}

/// The branch the lane's work integrates into: the pairing the lane's own
/// configuration declares, read by [`crate::git_shadow::declared_pairing`] as
/// the reference guard reads it. A remote of `.` names a branch of this
/// repository, so that branch is the baseline and nothing is fetched. A lane
/// that declares no pairing integrates into `origin`'s HEAD branch; a
/// repository with no `origin` then has no target.
fn resolve_target(root: &Path) -> Result<Option<Target>, Error> {
    let lane = required_read(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "resolve current branch",
    )?;
    let declared = crate::git_shadow::declared_pairing(root, &lane)
        .map_err(|error| Error::fault("git::pairing-invalid", error.to_string()))?;
    if let Some(pairing) = declared.as_ref().filter(|pairing| pairing.is_local()) {
        return Ok(Some(Target {
            baseline: pairing.reference(),
            remote: pairing.remote.clone(),
            branch: pairing.branch.clone(),
        }));
    }
    let remote = declared
        .as_ref()
        .map_or_else(|| "origin".to_owned(), |pairing| pairing.remote.clone());
    let remotes = required_read(root, &["remote"], "list Git remotes")?;
    if !remotes.lines().any(|candidate| candidate == remote) {
        return match declared {
            Some(_) => Err(Error::fault(
                "git::remote-invalid",
                format!(
                    "branch {lane} tracks remote {remote}, which this repository does not define"
                ),
            )),
            None => Ok(None),
        };
    }
    let url = run(root, &["remote", "get-url", &remote])?;
    if !url.success() {
        return Err(Error::fault(
            "git::remote-invalid",
            url.detail(&format!("resolve {remote} remote")),
        ));
    }
    let branch = match declared {
        Some(pairing) => pairing.branch,
        None => {
            let head = format!("refs/remotes/{remote}/HEAD");
            let symbolic = required_read(
                root,
                &["symbolic-ref", &head],
                &format!("resolve {remote} HEAD"),
            )?;
            symbolic
                .strip_prefix(&format!("refs/remotes/{remote}/"))
                .ok_or_else(|| Error::fault("git::baseline-invalid", symbolic.clone()))?
                .to_owned()
        }
    };
    Ok(Some(Target {
        baseline: format!("{remote}/{branch}"),
        remote,
        branch,
    }))
}

fn fetch(root: &Path, target: &Target) -> Result<(), Error> {
    if target.remote == "." {
        // The baseline is a branch of this repository: there is nothing to fetch.
        return Ok(());
    }
    // The Git common directory is bare and may name this remote-tracking ref
    // as its own HEAD while linked worktrees carry independent branch HEADs.
    // Fetch owns this one explicit update and ignores configured refspecs that
    // could otherwise use --update-head-ok to target a checked-out lane ref.
    let refspec = format!(
        "+refs/heads/{branch}:refs/remotes/{remote}/{branch}",
        branch = target.branch,
        remote = target.remote,
    );
    let fetched = run(
        root,
        &["fetch", "--update-head-ok", &target.remote, &refspec],
    )?;
    if fetched.success() {
        Ok(())
    } else {
        Err(Error::fault(
            "git::fetch-failed",
            fetched.detail(&format!("fetch {}", target.remote)),
        ))
    }
}

fn ahead_count(root: &Path, baseline: &str) -> Result<u32, Error> {
    let value = required_read(
        root,
        &["rev-list", "--count", &format!("{baseline}..HEAD")],
        "count local commits",
    )?;
    value
        .parse()
        .map_err(|_| Error::fault("git::ahead-invalid", format!("Git returned {value:?}")))
}

fn integrate(
    root: &Path,
    agent_head: &str,
    upstream_head: &str,
    message: &str,
    label: &str,
) -> Result<String, Error> {
    if agent_head == upstream_head {
        return Ok(agent_head.to_owned());
    }
    if is_ancestor(root, agent_head, upstream_head)? {
        let current = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
        if current != upstream_head {
            materialize_and_update(root, upstream_head, &current, "advance integrated baseline")?;
        }
        return Ok(upstream_head.to_owned());
    }
    if is_ancestor(root, upstream_head, agent_head)? {
        if merge_first_parent(root, agent_head)?.as_deref() == Some(upstream_head) {
            // Already shaped as a landing, so nothing below rebuilds it and this
            // is the only place its footprint can still be questioned.
            require_landing_within_footprint(root, label, upstream_head, agent_head, agent_head)?;
            return Ok(agent_head.to_owned());
        }
        return synthesize_and_update(root, agent_head, upstream_head, agent_head, message, label);
    }
    let computed = merge_tree(root, agent_head, upstream_head)?;
    if !computed.conflicts.is_empty() {
        materialize_conflict(
            root,
            &computed.tree,
            &computed.conflicts,
            agent_head,
            upstream_head,
            message,
        )?;
        return Err(conflict_error(label, upstream_head));
    }
    synthesize_and_update(
        root,
        &computed.tree,
        upstream_head,
        agent_head,
        message,
        label,
    )
}

fn synthesize_and_update(
    root: &Path,
    treeish: &str,
    first_parent: &str,
    second_parent: &str,
    message: &str,
    label: &str,
) -> Result<String, Error> {
    let tree = required_read(
        root,
        &["rev-parse", &format!("{treeish}^{{tree}}")],
        "read integrated tree",
    )?;
    let created = create_merge_commit(root, &tree, first_parent, second_parent, message)?;
    if !created.success() || created.stdout.trim().is_empty() {
        return Err(Error::fault(
            "git::commit-tree-failed",
            created.detail("synthesize task merge commit"),
        ));
    }
    let merge_head = created.stdout.trim().to_owned();
    // Before the branch moves: the commit object is still unreferenced here, so
    // refusing leaves nothing published and nothing to unwind.
    require_landing_within_footprint(root, label, first_parent, &merge_head, second_parent)?;
    let expected = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    materialize_and_update_owned(root, &merge_head, &expected, "advance branch to task merge")
        .map_err(|error| with_retry_remedy(error, label))?;
    Ok(merge_head)
}

fn create_merge_commit(
    root: &Path,
    tree: &str,
    first_parent: &str,
    second_parent: &str,
    message: &str,
) -> Result<GitOutput, Error> {
    // A retry over the same immutable parents must name the same commit. Git's
    // default wall clock made an otherwise identical synthesized landing gain
    // a new identity on every task-boundary retry. The later parent clock is
    // equally deterministic and prevents one old integrated-parent timestamp
    // from propagating through every later task landing.
    let date = later_parent_date(root, first_parent, second_parent)?;
    run_with_environment(
        root,
        &[
            "commit-tree",
            tree,
            "-p",
            first_parent,
            "-p",
            second_parent,
            "-m",
            message,
        ],
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    )
}

fn later_parent_date(
    root: &Path,
    first_parent: &str,
    second_parent: &str,
) -> Result<String, Error> {
    let first = commit_date(root, first_parent)?;
    let second = commit_date(root, second_parent)?;
    Ok(if second.0 > first.0 {
        second.1
    } else {
        first.1
    })
}

fn commit_date(root: &Path, commit: &str) -> Result<(i64, String), Error> {
    let observed = required_read(
        root,
        &["show", "-s", "--format=%ct %cI", commit],
        "read task merge parent timestamp",
    )?;
    let Some((seconds, date)) = observed.split_once(' ') else {
        return Err(Error::fault(
            "git::commit-date-invalid",
            format!("commit {commit} returned malformed timestamp {observed:?}"),
        ));
    };
    let seconds = seconds.parse::<i64>().map_err(|error| {
        Error::fault(
            "git::commit-date-invalid",
            format!("commit {commit} returned invalid epoch {seconds:?}: {error}"),
        )
    })?;
    Ok((seconds, date.to_owned()))
}

fn with_retry_remedy(mut error: Error, label: &str) -> Error {
    if error.class == ErrorClass::Refusal {
        error.remedy = format!(
            concat!(
                "inspect git status and HEAD, then retry task ",
                "completion for {label}",
            ),
            label = label
        );
    }
    error
}

struct MergeTree {
    tree: String,
    conflicts: Vec<String>,
}

fn merge_tree(root: &Path, left: &str, right: &str) -> Result<MergeTree, Error> {
    merge_tree_from(root, None, left, right)
}

/// Three-way merge tree over an explicit base, or the merge base when `None`.
fn merge_tree_from(
    root: &Path,
    base: Option<&str>,
    left: &str,
    right: &str,
) -> Result<MergeTree, Error> {
    let base = base.map(|base| format!("--merge-base={base}"));
    let mut args = vec!["merge-tree", "--write-tree", "-z", "--no-messages"];
    args.extend(base.as_deref());
    args.extend([left, right]);
    let merged = run(root, &args)?;
    let mut fields = merged.stdout.split('\0');
    let tree = fields.next().unwrap_or_default().trim().to_owned();
    let conflicts = fields
        .filter(|field| !field.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if merged.status == 1 && !tree.is_empty() && !conflicts.is_empty() {
        return Ok(MergeTree { tree, conflicts });
    }
    if merged.success() && !tree.is_empty() && conflicts.is_empty() {
        return Ok(MergeTree { tree, conflicts });
    }
    Err(Error::fault(
        "git::merge-tree-failed",
        merged.detail("compute task merge tree"),
    ))
}

/// What stays fixed across every attempt to publish one task phase.
#[derive(Clone, Copy)]
struct Publication<'a> {
    repository: &'a Repository,
    agent_head: &'a str,
    message: &'a str,
    label: &'a str,
    admission: &'a Admission,
}

fn publish_with_retry(
    root: &Path,
    target: &Target,
    mut integrated: String,
    mut upstream_head: String,
    publication: Publication<'_>,
) -> Result<(String, String), Error> {
    let Publication {
        repository,
        agent_head,
        message,
        label,
        admission,
    } = publication;
    admission.admit(repository, &integrated, label)?;
    let mut pushed = push(root, target, &integrated, &upstream_head)?;
    if pushed.success() {
        return Ok((integrated, upstream_head));
    }
    if !is_push_race(&pushed) {
        return Err(Error::fault(
            "git::publish-failed",
            pushed.detail(&format!("publish task work to {}", target.baseline)),
        ));
    }
    for _ in 0..PUBLISH_RACE_RETRY_LIMIT {
        fetch(root, target)?;
        let fresh = required_read(
            root,
            &["rev-parse", &target.baseline],
            "read raced baseline",
        )?;
        if fresh == integrated {
            return Ok((integrated, fresh));
        }
        if fresh != upstream_head {
            integrated = integrate_retry(root, &integrated, &fresh, message, label)?;
            upstream_head = fresh;
            admission.admit(repository, &integrated, label)?;
        }
        pushed = push(root, target, &integrated, &upstream_head)?;
        if pushed.success() {
            return Ok((integrated, upstream_head));
        }
        if !is_push_race(&pushed) {
            return Err(Error::fault(
                "git::publish-failed",
                pushed.detail(&format!("publish task work to {}", target.baseline)),
            ));
        }
    }
    Err(Error::refusal(
        "git::publish-race-exhausted",
        pushed.detail(&format!("publish task work to {}", target.baseline)),
        format!(
            concat!(
                "fetch {}, inspect {}, preserve lane commit ",
                "{agent_head}, and retry task completion for ",
                "{label}",
            ),
            target.remote,
            target.baseline,
            agent_head = agent_head,
            label = label,
        ),
    ))
}

fn integrate_retry(
    root: &Path,
    integrated: &str,
    fresh: &str,
    message: &str,
    label: &str,
) -> Result<String, Error> {
    if is_ancestor(root, integrated, fresh)? {
        let current = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
        materialize_and_update(root, fresh, &current, "advance to raced baseline")?;
        return Ok(fresh.to_owned());
    }
    let computed = merge_tree(root, integrated, fresh)?;
    if !computed.conflicts.is_empty() {
        materialize_conflict(
            root,
            &computed.tree,
            &computed.conflicts,
            integrated,
            fresh,
            message,
        )?;
        return Err(conflict_error(label, fresh));
    }
    synthesize_and_update(root, &computed.tree, fresh, integrated, message, label)
}

fn push(root: &Path, target: &Target, commit: &str, expected: &str) -> Result<GitOutput, Error> {
    if !is_ancestor(root, expected, commit)? {
        return Err(Error::refusal(
            "git::publish-non-fast-forward",
            format!("task landing {commit} does not retain expected upstream {expected}"),
            "inspect the task landing parents before retrying completion",
        ));
    }
    run(
        root,
        &[
            "push",
            &format!("--force-with-lease=refs/heads/{}:{expected}", target.branch),
            &target.remote,
            &format!("{commit}:refs/heads/{}", target.branch),
        ],
    )
}

fn is_push_race(output: &GitOutput) -> bool {
    let detail = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    [
        "non-fast-forward",
        "fetch first",
        "stale info",
        "stale-info",
    ]
    .iter()
    .any(|needle| detail.contains(needle))
        || (detail.contains("remote rejected")
            && detail.contains("cannot lock ref")
            && detail.contains("failed to update ref"))
}

mod admission;
use admission::*;
mod landing;
use landing::require_landing_within_footprint;
mod launch;
pub use launch::{LaunchAdvance, LaunchSkip};
mod recovery;
mod resolution;
pub use recovery::{
    Recovery, RecoveryClaim, RestoreOutcome, bind_recovery_claim, pending_recovery,
    quarantine_for_recovery, recovery_claim, replay_recovery, restore_recovery,
    synchronize_recovery,
};
mod transaction;
use transaction::*;
mod command;
use command::*;
mod snapshot;
mod transplant;

#[cfg(test)]
mod tests;

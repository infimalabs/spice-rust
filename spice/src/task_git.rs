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
use std::process::{Command, Stdio};
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
/// How long a descendant still on its way out may take before it is a survivor.
///
/// Git has already exited when this is waited on, so nothing can join the group
/// any more and the wait can only shorten. A descendant that has called exit
/// leaves the group in microseconds, so this is slack for a loaded machine to
/// schedule it, not time any healthy command spends.
const DESCENDANT_EXIT_GRACE: Duration = Duration::from_millis(500);
const PROCESS_GROUP_REAP_LIMIT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

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

/// One failure from the allocator-only resolution evidence runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionEvidenceError {
    class: ErrorClass,
    reason: String,
    detail: String,
    remedy: String,
}

impl ResolutionEvidenceError {
    /// Retain a policy or test refusal without converting it into a Git fault.
    #[must_use]
    pub fn refusal(
        reason: impl Into<String>,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            class: ErrorClass::Refusal,
            reason: reason.into(),
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    /// Retain an environment or protocol failure from the evidence process.
    #[must_use]
    pub fn fault(reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Fault,
            reason: reason.into(),
            detail: detail.into(),
            remedy: "repair the named evidence process and retry task completion".to_owned(),
        }
    }
}

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
}

/// Inspect loose commits without moving the lane or its branch.
pub fn capture(repository: &Repository) -> Result<Capture, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    require_clean(root, "task capture")?;
    let target = resolve_target(root)?.ok_or_else(|| {
        Error::refusal(
            "git::capture-no-baseline",
            "task capture found no origin-backed task baseline",
            concat!(
                "configure origin and branch tracking before ",
                "capturing loose commits",
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

/// Local commits a claim would strand if its holder released it now.
///
/// Deliberately read-only, unlike its siblings: it neither recovers a pending
/// operation, nor requires a clean tree, nor fetches, because deciding whether
/// a release is safe must not itself move the lane or reach the network. The
/// local baseline is the same evidence `prepare_claim` would refuse on later.
/// A lane with no origin-backed baseline has nothing to strand and answers with
/// no commits, which is the inert stance `prepare_claim` takes there too.
pub fn stranded_commits(repository: &Repository) -> Result<Vec<String>, Error> {
    let root = repository.worktree_root();
    let Some(target) = resolve_target(root)? else {
        return Ok(Vec::new());
    };
    let listed = required_read(
        root,
        &[
            "log",
            "--format=%h %s",
            &format!("{}..HEAD", target.baseline),
        ],
        "list local commits",
    )?;
    Ok(listed.lines().map(str::to_owned).collect())
}

/// Refuse a voluntary release that would leave task commits owned by nobody.
///
/// `prepare_claim` already refuses to hand a lane new work while loose commits
/// sit outside a claimed boundary. Releasing a claim is the door that creates
/// those loose commits in the first place, so it refuses on the same evidence
/// and names both the commits and the way out.
pub fn prepare_free(repository: &Repository, task: &str) -> Result<(), Error> {
    let stranded = stranded_commits(repository)?;
    if stranded.is_empty() {
        return Ok(());
    }
    Err(Error::refusal(
        "git::free-loose-commits",
        format!(
            "releasing {task} would strand {count} task-owned local commit(s): {listed}",
            count = stranded.len(),
            listed = stranded.join("; "),
        ),
        format!(
            "run `spice task done {task}` to record those commits under this \
             task, or reset the lane to the task baseline before freeing it",
        ),
    ))
}

/// Safely advance a clean lane before allocator possession is minted.
pub fn prepare_claim(repository: &Repository) -> Result<GitBoundary, Error> {
    prepare_claim_observed(repository, || Ok(()), |_| Ok(()))
}

/// Safely advance a clean lane while durably observing the exact target first.
///
/// Dirty-handoff recovery uses the observation to journal the only HEAD the
/// ordinary claim boundary may install. The ordinary allocator calls the same
/// implementation with an inert observer, so recovery cannot acquire a subtly
/// different fetch, ancestry, fast-forward, or boundary law. Recovery's
/// preflight rechecks its sealed authority after target resolution and before
/// either the fetch or a no-remote boundary is published.
fn prepare_claim_observed(
    repository: &Repository,
    mut preflight: impl FnMut() -> Result<(), Error>,
    mut observe: impl FnMut(&GitBoundary) -> Result<(), Error>,
) -> Result<GitBoundary, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    require_clean(root, "task next")?;
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
        return Err(Error::refusal(
            "git::claim-loose-commits",
            format!(
                concat!(
                    "the lane has {ahead} local commit(s) outside a ",
                    "claimed task boundary",
                ),
                ahead = ahead
            ),
            concat!(
                "run task capture for those commits before ",
                "requesting task next",
            ),
        ));
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
///
/// The evidence callback is reachable only when the repository retains the exact
/// conflict state previously materialized for `active`. Ordinary commits and
/// direct test callers cannot select it.
pub fn publish_claimed(
    repository: &Repository,
    meta: Metadata<'_>,
    active: &board::Active<'_>,
    evidence: impl FnOnce() -> Result<(), ResolutionEvidenceError>,
) -> Result<GitBoundary, Error> {
    publish_inner(repository, meta, Some((active, Box::new(evidence))))
}

type ResolutionEvidence<'a> = Box<dyn FnOnce() -> Result<(), ResolutionEvidenceError> + 'a>;

fn publish_inner<'a>(
    repository: &Repository,
    meta: Metadata<'_>,
    resolution: Option<(&board::Active<'_>, ResolutionEvidence<'a>)>,
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
        require_clean(root, "task completion")?;
    }
    let observed_head = required_read(root, &["rev-parse", "HEAD"], "read lane HEAD")?;
    let Some(target) = resolve_target(root)? else {
        return Ok(boundary(
            observed_head.clone(),
            observed_head,
            None,
            None,
            0,
        ));
    };
    fetch(root, &target)?;
    let upstream_head = required_read(root, &["rev-parse", &target.baseline], "read baseline")?;
    if let Some((active, evidence)) = resolution.filter(|_| resolution_state) {
        let local_commits = ahead_count(root, &target.baseline)?;
        let message = compose_message(meta);
        let integrated = resolution::complete(
            root,
            &target,
            meta,
            active,
            resolution::Parents {
                agent: &observed_head,
                upstream: &upstream_head,
            },
            &message,
            evidence,
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
    let local_commits = ahead_count(root, &target.baseline)?;
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

fn resolve_target(root: &Path) -> Result<Option<Target>, Error> {
    let remotes = required_read(root, &["remote"], "list Git remotes")?;
    if !remotes.lines().any(|remote| remote == "origin") {
        return Ok(None);
    }
    let origin = run(root, &["remote", "get-url", "origin"])?;
    if !origin.success() {
        return Err(Error::fault(
            "git::remote-invalid",
            origin.detail("resolve origin remote"),
        ));
    }
    let branch = required_read(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "resolve current branch",
    )?;
    let configured = try_read(
        root,
        &["config", "--get", &format!("branch.{branch}.merge")],
    )?;
    if let Some(branch) =
        configured.and_then(|value| value.strip_prefix("refs/heads/").map(str::to_owned))
    {
        return Ok(Some(Target {
            baseline: format!("origin/{branch}"),
            remote: "origin".to_owned(),
            branch,
        }));
    }
    let symbolic = required_read(
        root,
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
        "resolve origin HEAD",
    )?;
    let branch = symbolic
        .strip_prefix("refs/remotes/origin/")
        .ok_or_else(|| Error::fault("git::baseline-invalid", symbolic.clone()))?
        .to_owned();
    Ok(Some(Target {
        baseline: format!("origin/{branch}"),
        remote: "origin".to_owned(),
        branch,
    }))
}

fn fetch(root: &Path, target: &Target) -> Result<(), Error> {
    let fetched = run(root, &["fetch", &target.remote])?;
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
    let created = run(
        root,
        &[
            "commit-tree",
            &tree,
            "-p",
            first_parent,
            "-p",
            second_parent,
            "-m",
            message,
        ],
    )?;
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
    materialize_and_update(root, &merge_head, &expected, "advance branch to task merge")
        .map_err(|error| with_retry_remedy(error, label))?;
    Ok(merge_head)
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
    let merged = run(
        root,
        &[
            "merge-tree",
            "--write-tree",
            "-z",
            "--no-messages",
            left,
            right,
        ],
    )?;
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
            // The race produced a tree nothing has read yet. Every other path
            // out of this loop pushes the commit admitted above it.
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
    Recovery, RecoveryClaim, bind_recovery_claim, pending_recovery, quarantine_for_recovery,
    recovery_claim, replay_recovery, restore_recovery, synchronize_recovery,
};
mod transaction;
use transaction::*;
mod command;
use command::*;

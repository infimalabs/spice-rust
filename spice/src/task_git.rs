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
const MERGE_STATE_FILES: &[&str] =
    &["ORIG_HEAD", "MERGE_MODE", "MERGE_MSG", "MERGE_HEAD"];
const LOCAL_COMMAND_LIMIT: Duration = Duration::from_secs(10);
const NETWORK_COMMAND_LIMIT: Duration = Duration::from_secs(30);
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
    fn refusal(
        reason: &'static str,
        detail: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            class: ErrorClass::Refusal,
            reason,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    fn fault(
        reason: &'static str,
        detail: impl Into<String>,
    ) -> Self {
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
            format!(
                "could not {action} (git exit {})\n{output}",
                self.status
            )
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
    let upstream_head = required_read(
        root,
        &["rev-parse", &target.baseline],
        "read baseline",
    )?;
    let head = required_read(
        root,
        &["rev-parse", "HEAD"],
        "read lane HEAD",
    )?;
    let local_commits = ahead_count(root, &target.baseline)?;
    if local_commits == 0 {
        return Err(Error::refusal(
            "git::nothing-to-capture",
            "no local commits are ahead of the task baseline",
            "make and commit the intended change before task capture",
        ));
    }
    let subject = required_read(
        root,
        &["log", "-1", "--format=%s"],
        "read commit subject",
    )?;
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

/// Safely advance a clean lane before allocator possession is minted.
pub fn prepare_claim(
    repository: &Repository,
) -> Result<GitBoundary, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    require_clean(root, "task next")?;
    let before = required_read(
        root,
        &["rev-parse", "HEAD"],
        "read lane HEAD",
    )?;
    let Some(target) = resolve_target(root)? else {
        return Ok(boundary(before.clone(), before, None, None, 0));
    };
    fetch(root, &target)?;
    let upstream_head = required_read(
        root,
        &["rev-parse", &target.baseline],
        "read baseline",
    )?;
    let ahead = ahead_count(root, &target.baseline)?;
    if ahead != 0 {
        return Err(Error::refusal(
            "git::claim-loose-commits",
            format!(concat!(
                "the lane has {ahead} local commit(s) outside a ",
                "claimed task boundary",
            ), ahead = ahead),
            concat!(
                "run task capture for those commits before ",
                "requesting task next",
            ),
        ));
    }
    if before != upstream_head {
        if !is_ancestor(root, &before, &upstream_head)? {
            return Err(Error::refusal(
                "git::claim-diverged",
                format!(concat!(
                    "lane HEAD {before} does not fast-forward to ",
                    "{upstream_head}",
                ), before = before, upstream_head = upstream_head),
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
    Ok(boundary(
        upstream_head.clone(),
        upstream_head.clone(),
        Some(&target.baseline),
        Some(upstream_head),
        0,
    ))
}

/// Publish one task phase before its journal transition commits.
pub fn publish(
    repository: &Repository,
    meta: Metadata<'_>,
) -> Result<GitBoundary, Error> {
    let root = repository.worktree_root();
    recover_pending(root)?;
    require_no_open_merge(root, meta.label)?;
    require_clean(root, "task completion")?;
    let observed_head = required_read(
        root,
        &["rev-parse", "HEAD"],
        "read lane HEAD",
    )?;
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
    let upstream_head = required_read(
        root,
        &["rev-parse", &target.baseline],
        "read baseline",
    )?;
    let agent_head = if is_ancestor(
        root,
        &observed_head,
        &upstream_head,
    )? {
        recovered_lane_head(root, &observed_head, meta)?
            .unwrap_or(observed_head)
    } else {
        observed_head
    };
    let local_commits = ahead_count(root, &target.baseline)?;
    let message = compose_message(meta, &agent_head);
    let integrated = integrate(
        root,
        &agent_head,
        &upstream_head,
        &message,
        meta.label,
    )?;
    let (integrated, published_upstream) = publish_with_retry(
        root,
        &target,
        integrated,
        upstream_head,
        &agent_head,
        &message,
        meta.label,
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
    let remotes = required_read(
        root,
        &["remote"],
        "list Git remotes",
    )?;
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
    if let Some(branch) = configured.and_then(|value| {
        value.strip_prefix("refs/heads/").map(str::to_owned)
    }) {
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
        .ok_or_else(|| {
            Error::fault("git::baseline-invalid", symbolic.clone())
        })?
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
    value.parse().map_err(|_| {
        Error::fault(
            "git::ahead-invalid",
            format!("Git returned {value:?}"),
        )
    })
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
        let current = required_read(
            root,
            &["rev-parse", "HEAD"],
            "read lane HEAD",
        )?;
        if current != upstream_head {
            materialize_and_update(
                root,
                upstream_head,
                &current,
                "advance integrated baseline",
            )?;
        }
        return Ok(upstream_head.to_owned());
    }
    if is_ancestor(root, upstream_head, agent_head)? {
        if merge_first_parent(root, agent_head)?.as_deref()
            == Some(upstream_head)
        {
            return Ok(agent_head.to_owned());
        }
        return synthesize_and_update(
            root,
            agent_head,
            upstream_head,
            agent_head,
            message,
            label,
        );
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
    let expected = required_read(
        root,
        &["rev-parse", "HEAD"],
        "read lane HEAD",
    )?;
    materialize_and_update(
        root,
        &merge_head,
        &expected,
        "advance branch to task merge",
    )
    .map_err(|error| with_retry_remedy(error, label))?;
    Ok(merge_head)
}

fn with_retry_remedy(mut error: Error, label: &str) -> Error {
    if error.class == ErrorClass::Refusal {
        error.remedy = format!(concat!(
            "inspect git status and HEAD, then retry task ",
            "completion for {label}",
        ), label = label);
    }
    error
}

struct MergeTree {
    tree: String,
    conflicts: Vec<String>,
}

fn merge_tree(
    root: &Path,
    left: &str,
    right: &str,
) -> Result<MergeTree, Error> {
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
    if merged.status == 1 && !tree.is_empty() && !conflicts.is_empty()
    {
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

fn publish_with_retry(
    root: &Path,
    target: &Target,
    mut integrated: String,
    mut upstream_head: String,
    agent_head: &str,
    message: &str,
    label: &str,
) -> Result<(String, String), Error> {
    let mut pushed = push(root, target, &integrated, &upstream_head)?;
    if pushed.success() {
        return Ok((integrated, upstream_head));
    }
    if !is_push_race(&pushed) {
        return Err(Error::fault(
            "git::publish-failed",
            pushed.detail(&format!(
                "publish task work to {}",
                target.baseline
            )),
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
            integrated = integrate_retry(
                root,
                &integrated,
                &fresh,
                message,
                label,
            )?;
            upstream_head = fresh;
        }
        pushed = push(root, target, &integrated, &upstream_head)?;
        if pushed.success() {
            return Ok((integrated, upstream_head));
        }
        if !is_push_race(&pushed) {
            return Err(Error::fault(
                "git::publish-failed",
                pushed.detail(&format!(
                    "publish task work to {}",
                    target.baseline
                )),
            ));
        }
    }
    Err(Error::refusal(
        "git::publish-race-exhausted",
        pushed.detail(&format!(
            "publish task work to {}",
            target.baseline
        )),
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
        let current = required_read(
            root,
            &["rev-parse", "HEAD"],
            "read lane HEAD",
        )?;
        materialize_and_update(
            root,
            fresh,
            &current,
            "advance to raced baseline",
        )?;
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
    synthesize_and_update(
        root,
        &computed.tree,
        fresh,
        integrated,
        message,
        label,
    )
}

fn push(
    root: &Path,
    target: &Target,
    commit: &str,
    expected: &str,
) -> Result<GitOutput, Error> {
    run(
        root,
        &[
            "push",
            &format!(
                "--force-with-lease=refs/heads/{}:{expected}",
                target.branch
            ),
            &target.remote,
            &format!("{commit}:refs/heads/{}", target.branch),
        ],
    )
}

fn is_push_race(output: &GitOutput) -> bool {
    let detail = format!("{}\n{}", output.stdout, output.stderr)
        .to_ascii_lowercase();
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

include!("task_git/transaction.rs");
include!("task_git/command.rs");
#[cfg(test)]
include!("task_git/tests.rs");

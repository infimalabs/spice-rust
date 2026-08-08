//! GIT-08, the landing footprint: a task lands only what it wrote.
//!
//! A task lands as a merge whose first parent is the baseline it integrated
//! against. Every path that merge changes against that baseline has to come
//! from one of the task's own commits. Anything else was already on the shared
//! branch before the task started, which makes it a peer's, and overwriting it
//! here is silent: the landing carries no conflict, no warning, and nothing
//! recording that the replaced content was ever there.
//!
//! A machine merge cannot drift, because merging the baseline with the lane
//! changes only what the lane changed. Hand resolution can, and this port
//! reaches hand resolution by design -- [`super::materialize_conflict`] hands a
//! conflicted tree to the agent and takes back whatever it commits. That
//! returning tree is the one landing nothing else inspects, which is what this
//! boundary is for.
//!
//! The refusal names every drifted path and classifies it, because the two
//! classes need opposite recoveries: a path the baseline still holds is put
//! back by restoring its content, and a path the baseline never had is put back
//! by removing it. A recipe that offered only one of those would be wrong half
//! the time.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use super::{Error, required_read, run};

/// Where a drifted path stood at the baseline, which decides how it is put back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Drift {
    /// The baseline holds this path, so the landing rewrote a peer's content.
    RewritesBaseline,
    /// The baseline never held it, so the landing invented it outside the task.
    AddsToBaseline,
}

impl fmt::Display for Drift {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RewritesBaseline => formatter.write_str("rewrites-baseline"),
            Self::AddsToBaseline => formatter.write_str("adds-to-baseline"),
        }
    }
}

/// Refuse a landing whose first-parent diff leaves the task's own footprint.
///
/// `merge_head` is the landing being judged and `agent_head` the lane work it
/// claims to carry; both are compared against `upstream_head`, the baseline the
/// merge takes as its first parent.
pub(super) fn require_landing_within_footprint(
    root: &Path,
    label: &str,
    upstream_head: &str,
    merge_head: &str,
    agent_head: &str,
) -> Result<(), Error> {
    let landed = read_paths(
        root,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            upstream_head,
            merge_head,
        ],
        "read the landing diff",
    )?;
    let footprint = footprint_paths(root, upstream_head, agent_head)?;
    let drifted = landed.difference(&footprint).cloned().collect::<Vec<_>>();
    if drifted.is_empty() {
        return Ok(());
    }
    Err(drift_refusal(
        label,
        upstream_head,
        &classify(root, upstream_head, &drifted)?,
    ))
}

/// The paths of the task's own non-merge commits since the baseline.
///
/// A resolution merge must not count towards the footprint it is being measured
/// against, or the landing would authorize itself: the paths the resolution
/// touched are exactly the set this boundary exists to question.
///
/// `--no-merges` states that, but does not by itself achieve it, and no fixture
/// here can tell the flag apart from its absence. `git log --name-only` already
/// prints a merge with an empty file list, so the paths are excluded whether or
/// not the commit is. The flag is kept because the predecessor names it at
/// `spice/tasks/git/merging.py:336` and because the exclusion is a decision
/// rather than a default worth inheriting silently -- if this listing ever
/// grows `-m` or `--first-parent`, the default reverses and the flag is what
/// holds the meaning still. Measured, not assumed: dropping it leaves all five
/// fixtures green.
fn footprint_paths(
    root: &Path,
    upstream_head: &str,
    agent_head: &str,
) -> Result<BTreeSet<String>, Error> {
    read_paths(
        root,
        &[
            "log",
            "--no-renames",
            "--no-merges",
            "--format=",
            "--name-only",
            &format!("{upstream_head}..{agent_head}"),
        ],
        "read the task footprint",
    )
}

fn read_paths(root: &Path, args: &[&str], action: &str) -> Result<BTreeSet<String>, Error> {
    Ok(required_read(root, args, action)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn classify(
    root: &Path,
    upstream_head: &str,
    drifted: &[String],
) -> Result<Vec<(String, Drift)>, Error> {
    drifted
        .iter()
        .map(|path| {
            let probe = run(
                root,
                &["cat-file", "-e", &format!("{upstream_head}:{path}")],
            )?;
            let drift = if probe.success() {
                Drift::RewritesBaseline
            } else {
                Drift::AddsToBaseline
            };
            Ok((path.clone(), drift))
        })
        .collect()
}

fn drift_refusal(label: &str, upstream_head: &str, classified: &[(String, Drift)]) -> Error {
    let named = classified
        .iter()
        .map(|(path, drift)| format!("\n  {drift} {path}"))
        .collect::<String>();
    Error::refusal(
        "git::landing-out-of-scope",
        format!(
            "landing {label} changes baseline paths outside its own commits:{named}\nthose \
             paths carry work already on the shared branch, and publishing would overwrite \
             it without recording that it was there"
        ),
        remedy(label, upstream_head, classified),
    )
}

/// Both recoveries, because the boundary cannot tell which one is meant.
///
/// Drift is equally consistent with a resolution that reached too far and with
/// one that deliberately took the paths over, and only the author knows which.
/// Naming a single recovery would make the refusal decide the question it exists
/// to raise, so it states the branch each recipe belongs to and stops there.
fn remedy(label: &str, upstream_head: &str, classified: &[(String, Drift)]) -> String {
    let restore = |wanted: Drift| {
        classified
            .iter()
            .filter(|(_, drift)| *drift == wanted)
            .map(|(path, _)| format!(" {path}"))
            .collect::<String>()
    };
    let rewrites = restore(Drift::RewritesBaseline);
    let adds = restore(Drift::AddsToBaseline);
    let mut lines = vec![
        "choose one branch before editing anything".to_owned(),
        "unintended drift, drop the changes: put every path above back as the shared branch \
         has it and commit the restoration"
            .to_owned(),
    ];
    if !rewrites.is_empty() {
        lines.push(format!("git checkout {upstream_head} --{rewrites}"));
    }
    if !adds.is_empty() {
        lines.push(format!("git rm --{adds}"));
    }
    lines.push(format!(
        "deliberate ownership, keep the changes: give every path above a real delta and \
         record them in a new non-merge commit, which is what puts them inside the \
         footprint of {label}"
    ));
    lines.push("then retry task completion".to_owned());
    lines.join("; ")
}

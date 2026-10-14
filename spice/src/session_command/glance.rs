//! Bounded glances a briefing folds beside held work: the board around a
//! lane, the direction it already retired, and the checkout's working state.
//!
//! Each glance reads only what it can bound. The board walks current task
//! heads page by page up to a fixed ceiling and says when it stopped early;
//! the working state is two read-only `git` queries that take no index lock.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use journal::{FactReadBudget, GuardError, JournalRead, RetiredSteering};
use wire::ClaimDisposition;

use super::model::{BoardGlanceView, RepositoryGlanceView, RetiredDirectionView};

/// Task-head pages the board glance walks before it reports itself partial.
const BOARD_PAGES: usize = 16;
/// Retired directions a briefing recalls.
pub(super) const DIRECTION_ROWS: usize = 4;
/// Bytes of one retired direction's text.
const DIRECTION_BYTES: usize = 320;
/// Uncommitted paths the working-state glance keeps.
const DIRTY_ROWS: usize = 24;
/// Newest commits the working-state glance keeps.
const COMMIT_ROWS: usize = 8;

/// Pending public execution work no actor holds, counted by phase.
///
/// Hidden projects (a leading `.`, such as parked oops rows), Design roots,
/// and unauthorized outputs controlled by a Design stay on their dedicated
/// operator-facing surfaces rather than being presented as a lane's next work.
pub(super) fn board(read: &JournalRead<'_>) -> Result<BoardGlanceView, GuardError> {
    let mut unclaimed = BTreeMap::<String, usize>::new();
    let mut controller_designs = BTreeMap::<wire::TaskStamp, bool>::new();
    let mut after = None;
    let mut partial = true;
    for _ in 0..BOARD_PAGES {
        let page = read.task_head_page(after.as_ref(), FactReadBudget::RETAINED)?;
        for heads in &page.rows {
            let statement = heads.statement().body();
            let workflow = heads.workflow().body();
            let held = heads
                .claim()
                .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active);
            if held
                || workflow.status != "pending"
                || statement.project.starts_with('.')
                || speculative_design_work(read, statement, &mut controller_designs)?
            {
                continue;
            }
            *unclaimed
                .entry(workflow.phase.as_str().to_owned())
                .or_default() += 1;
        }
        match page.continuation {
            Some(next) => after = Some(next),
            None => {
                partial = false;
                break;
            }
        }
    }
    Ok(BoardGlanceView {
        unclaimed: unclaimed.into_iter().collect(),
        partial,
    })
}

/// Resolve only the immediate controller a prepared statement names. The
/// journal lookup is primary-key bounded; ordinary rows need no extra read.
fn speculative_design_work(
    read: &JournalRead<'_>,
    statement: &wire::TaskHead,
    controller_designs: &mut BTreeMap<wire::TaskStamp, bool>,
) -> Result<bool, GuardError> {
    if statement.intent == Some(wire::TaskIntent::Design) {
        return Ok(true);
    }
    let controller_is_design = if let Some(preparation) = &statement.preparation {
        if let Some(known) = controller_designs.get(&preparation.controller) {
            *known
        } else {
            let design = read
                .task_head(&preparation.controller)?
                .is_some_and(|head| head.body().intent == Some(wire::TaskIntent::Design));
            controller_designs.insert(preparation.controller.clone(), design);
            design
        }
    } else {
        false
    };
    Ok(board::is_speculative_design_work(
        statement,
        controller_is_design,
    ))
}

/// The newest directions this lane retired, as context rather than authority.
pub(super) fn directions(retired: Vec<RetiredSteering>) -> Vec<RetiredDirectionView> {
    retired
        .into_iter()
        .take(DIRECTION_ROWS)
        .map(|row| RetiredDirectionView {
            key: row.key,
            disposition: format!("{:?}", row.disposition).to_ascii_lowercase(),
            published: row.published.to_string(),
            text: crate::read_page::clip(&row.text, DIRECTION_BYTES),
        })
        .collect()
}

/// Uncommitted paths and the newest commits of one checkout.
pub(super) fn repository(root: &Path) -> RepositoryGlanceView {
    let read = || -> Result<RepositoryGlanceView, String> {
        let status = git(
            root,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?;
        let dirty = status
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let log = git(
            root,
            &["log", "-n", &COMMIT_ROWS.to_string(), "--format=%h %s"],
        )?;
        Ok(RepositoryGlanceView {
            dirty_total: dirty.len(),
            dirty: dirty
                .into_iter()
                .take(DIRTY_ROWS)
                .map(str::to_owned)
                .collect(),
            commits: log.lines().map(str::to_owned).collect(),
            unreadable: None,
        })
    };
    read().unwrap_or_else(|error| RepositoryGlanceView {
        dirty: Vec::new(),
        dirty_total: 0,
        commits: Vec::new(),
        unreadable: Some(error),
    })
}

/// The working-state glance of a checkout no briefing reads.
#[cfg(test)]
pub(super) fn unread(_root: &Path) -> RepositoryGlanceView {
    RepositoryGlanceView {
        dirty: Vec::new(),
        dirty_total: 0,
        commits: Vec::new(),
        unreadable: Some("working state is not read in process".to_owned()),
    }
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(arguments)
        // A glance must never contend with the agent's own index writes.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("git {}: {error}", arguments.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} exited {}: {}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("git {} wrote non-UTF-8: {error}", arguments.join(" ")))
}

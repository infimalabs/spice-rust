//! Exact admission of one allocator-materialized merge resolution.

use std::fs;
use std::path::Path;

use super::landing::require_tree_within_footprint;
use super::transaction::{git_state_path, materialize_resolution};
use super::*;

struct Seal {
    agent: String,
    upstream: String,
    tree: String,
    head_ref: String,
    configuration: String,
    message: String,
}

pub(super) struct Parents<'a> {
    pub(super) agent: &'a str,
    pub(super) upstream: &'a str,
}

pub(super) fn has_state(root: &Path) -> Result<bool, Error> {
    // ORIG_HEAD is durable Git history and commonly survives completed merges;
    // only the three in-progress controls select resolution admission.
    for name in ["MERGE_HEAD", "MERGE_MODE", "MERGE_MSG"] {
        let path = git_state_path(root, name)?;
        match fs::metadata(&path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Error::fault(
                    "git::resolution-state-unreadable",
                    format!("{}: {error}", path.display()),
                ));
            }
        }
    }
    Ok(false)
}

pub(super) fn complete(
    root: &Path,
    target: &Target,
    meta: Metadata<'_>,
    active: &board::Active<'_>,
    parents: Parents<'_>,
    message: &str,
    evidence: ResolutionEvidence<'_>,
) -> Result<String, Error> {
    let Parents { agent, upstream } = parents;
    require_claim(meta, active)?;
    let seal = Seal::capture(root, target, agent, upstream, message)?;
    require_tree_within_footprint(root, meta.label, upstream, &seal.tree, agent)?;
    evidence().map_err(evidence_error)?;
    require_claim(meta, active)?;
    seal.verify(root, target)?;
    let created = run(
        root,
        &[
            "commit-tree",
            &seal.tree,
            "-p",
            &seal.upstream,
            "-p",
            &seal.agent,
            "-m",
            &seal.message,
        ],
    )?;
    if !created.success() || created.stdout.trim().is_empty() {
        return Err(Error::fault(
            "git::resolution-commit-failed",
            created.detail("create admitted task resolution"),
        ));
    }
    let commit = created.stdout.trim().to_owned();
    seal.verify(root, target)?;
    materialize_resolution(root, &commit, &seal.agent, &seal.tree, meta.label)?;
    Ok(commit)
}

impl Seal {
    fn capture(
        root: &Path,
        target: &Target,
        agent: &str,
        upstream: &str,
        message: &str,
    ) -> Result<Self, Error> {
        require_state_file(root, "MERGE_HEAD", &format!("{upstream}\n"))?;
        require_state_file(root, "ORIG_HEAD", &format!("{agent}\n"))?;
        require_state_file(root, "MERGE_MODE", "no-ff\n")?;
        require_state_file(root, "MERGE_MSG", &format!("{message}\n"))?;
        require_resolved_index(root)?;
        let head = required_read(root, &["rev-parse", "HEAD"], "read resolution HEAD")?;
        if head != agent {
            return Err(moved("HEAD", agent, &head));
        }
        let baseline = required_read(
            root,
            &["rev-parse", &target.baseline],
            "read resolution baseline",
        )?;
        if baseline != upstream {
            return Err(moved("baseline", upstream, &baseline));
        }
        let tree = required_read(root, &["write-tree"], "seal resolved index tree")?;
        let head_ref = required_read(
            root,
            &["symbolic-ref", "--quiet", "HEAD"],
            "read resolution HEAD ref",
        )?;
        let configuration = configuration(root)?;
        Ok(Self {
            agent: agent.to_owned(),
            upstream: upstream.to_owned(),
            tree,
            head_ref,
            configuration,
            message: message.to_owned(),
        })
    }

    fn verify(&self, root: &Path, target: &Target) -> Result<(), Error> {
        require_state_file(root, "MERGE_HEAD", &format!("{}\n", self.upstream))?;
        require_state_file(root, "ORIG_HEAD", &format!("{}\n", self.agent))?;
        require_state_file(root, "MERGE_MODE", "no-ff\n")?;
        require_state_file(root, "MERGE_MSG", &format!("{}\n", self.message))?;
        require_resolved_index(root)?;
        require_equal_read(root, &["rev-parse", "HEAD"], "HEAD", &self.agent)?;
        require_equal_read(
            root,
            &["rev-parse", &target.baseline],
            "baseline",
            &self.upstream,
        )?;
        require_equal_read(
            root,
            &["symbolic-ref", "--quiet", "HEAD"],
            "HEAD ref",
            &self.head_ref,
        )?;
        require_equal_read(root, &["write-tree"], "resolved index", &self.tree)?;
        let observed = configuration(root)?;
        if observed != self.configuration {
            return Err(Error::refusal(
                "git::resolution-configuration-moved",
                "Git configuration changed while resolution evidence was running",
                "restore the intended repository configuration and retry task completion",
            ));
        }
        Ok(())
    }
}

fn require_claim(meta: Metadata<'_>, active: &board::Active<'_>) -> Result<(), Error> {
    if active.handle().as_str() == meta.label && active.actor().to_string() == meta.actor {
        return Ok(());
    }
    Err(Error::refusal(
        "git::resolution-claim-mismatch",
        format!(
            "resolution names task {} actor {}, but active proof names task {} actor {}",
            meta.label,
            meta.actor,
            active.handle(),
            active.actor(),
        ),
        "retry through the lane capability that owns this exact active task phase",
    ))
}

fn require_state_file(root: &Path, name: &str, expected: &str) -> Result<(), Error> {
    let path = git_state_path(root, name)?;
    let observed = match fs::read_to_string(&path) {
        Ok(observed) => observed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::refusal(
                "git::resolution-state-missing",
                format!("required {name} is absent at {}", path.display()),
                "restore the allocator-materialized merge state before retrying task completion",
            ));
        }
        Err(error) => {
            return Err(Error::fault(
                "git::resolution-state-unreadable",
                error.to_string(),
            ));
        }
    };
    if observed == expected {
        return Ok(());
    }
    Err(Error::refusal(
        "git::resolution-state-mismatch",
        format!("{name} does not match the allocator-materialized merge state"),
        "do not edit Git merge control files; abort and retry from the current baseline",
    ))
}

fn require_resolved_index(root: &Path) -> Result<(), Error> {
    let unresolved = required_read(
        root,
        &["ls-files", "-u", "-z"],
        "inspect unresolved index stages",
    )?;
    if !unresolved.is_empty() {
        return Err(Error::refusal(
            "git::resolution-unresolved",
            "the task resolution still contains unresolved index stages",
            "resolve every conflicted path, git add each resolution, then retry task completion",
        ));
    }
    let unstaged = run(root, &["diff", "--quiet", "--exit-code"])?;
    if !unstaged.success() {
        return Err(Error::refusal(
            "git::resolution-unstaged",
            "the task resolution contains tracked worktree changes absent from the index",
            "git add every intended resolution before retrying task completion",
        ));
    }
    let untracked = required_read(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        "inspect resolution untracked paths",
    )?;
    if !untracked.is_empty() {
        return Err(Error::refusal(
            "git::resolution-untracked",
            "the task resolution contains untracked paths outside the sealed index",
            "add or remove every untracked path before retrying task completion",
        ));
    }
    Ok(())
}

fn configuration(root: &Path) -> Result<String, Error> {
    required_read(
        root,
        &[
            "config",
            "--null",
            "--show-origin",
            "--show-scope",
            "--list",
        ],
        "capture Git resolution configuration",
    )
}

fn require_equal_read(root: &Path, args: &[&str], name: &str, expected: &str) -> Result<(), Error> {
    let observed = required_read(root, args, &format!("recheck resolution {name}"))?;
    if observed == expected {
        Ok(())
    } else {
        Err(moved(name, expected, &observed))
    }
}

fn moved(name: &str, expected: &str, observed: &str) -> Error {
    Error::refusal(
        "git::resolution-state-moved",
        format!("{name} moved from {expected} to {observed} while evidence was running"),
        "inspect the moved state and retry task completion from a fresh conflict handoff",
    )
}

fn evidence_error(error: ResolutionEvidenceError) -> Error {
    Error {
        class: error.class,
        reason: "git::resolution-evidence-refused",
        detail: format!("{}: {}", error.reason, error.detail),
        remedy: error.remedy,
    }
}

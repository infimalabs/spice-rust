//! Exact admission of one allocator-materialized merge resolution.

use std::fs;
use std::path::Path;

use super::landing::require_tree_within_footprint;
use super::transaction::{git_state_path, materialize_resolution};
use super::*;

#[derive(Debug, Eq, PartialEq)]
struct Seal {
    agent: String,
    upstream: String,
    tree: String,
    head_ref: String,
    configuration: String,
    message: String,
}

pub(super) struct Preflight(Seal);

pub(super) struct Parents<'a> {
    pub(super) agent: &'a str,
    pub(super) upstream: &'a str,
    pub(super) preflight: Preflight,
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
    let Parents {
        agent,
        upstream,
        preflight,
    } = parents;
    require_claim(meta, active)?;
    let seal = preflight.complete(root, target, agent, upstream, message)?;
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

impl Preflight {
    pub(super) fn capture(root: &Path) -> Result<Self, Error> {
        let upstream = read_state_value(root, "MERGE_HEAD")?;
        let agent = read_state_value(root, "ORIG_HEAD")?;
        require_state_file(root, "MERGE_MODE", "no-ff\n")?;
        let message = read_state_text(root, "MERGE_MSG")?
            .strip_suffix('\n')
            .ok_or_else(|| {
                Error::refusal(
                    "git::resolution-state-mismatch",
                    "MERGE_MSG has no canonical final newline",
                    "do not edit Git merge control files; abort and retry from the current baseline",
                )
            })?
            .to_owned();
        require_resolved_index(root)?;
        require_equal_read(root, &["rev-parse", "HEAD"], "HEAD", &agent)?;
        let tree = required_read(root, &["write-tree"], "seal resolved index tree")?;
        let head_ref = required_read(
            root,
            &["symbolic-ref", "--quiet", "HEAD"],
            "read resolution HEAD ref",
        )?;
        let configuration = configuration(root)?;
        Ok(Self(Seal {
            agent,
            upstream,
            tree,
            head_ref,
            configuration,
            message,
        }))
    }

    fn complete(
        self,
        root: &Path,
        target: &Target,
        agent: &str,
        upstream: &str,
        message: &str,
    ) -> Result<Seal, Error> {
        let seal = Seal::capture(root, target, agent, upstream, message)?;
        if seal == self.0 {
            Ok(seal)
        } else {
            Err(Error::refusal(
                "git::resolution-state-moved",
                "resolution controls, index, HEAD, or Git configuration moved while evidence was running",
                "restore the exact sealed resolution and retry task completion",
            ))
        }
    }
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
    let observed = read_state_text(root, name)?;
    if observed == expected {
        return Ok(());
    }
    Err(Error::refusal(
        "git::resolution-state-mismatch",
        format!("{name} does not match the allocator-materialized merge state"),
        "do not edit Git merge control files; abort and retry from the current baseline",
    ))
}

fn read_state_text(root: &Path, name: &str) -> Result<String, Error> {
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
    Ok(observed)
}

fn read_state_value(root: &Path, name: &str) -> Result<String, Error> {
    let text = read_state_text(root, name)?;
    let Some(value) = text.strip_suffix('\n') else {
        return Err(Error::refusal(
            "git::resolution-state-mismatch",
            format!("{name} is not one canonical line"),
            "do not edit Git merge control files; abort and retry from the current baseline",
        ));
    };
    if value.is_empty() || value.contains('\n') {
        return Err(Error::refusal(
            "git::resolution-state-mismatch",
            format!("{name} is not one canonical line"),
            "do not edit Git merge control files; abort and retry from the current baseline",
        ));
    }
    Ok(value.to_owned())
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

pub(super) fn evidence_error(error: ResolutionEvidenceError) -> Error {
    if error.reason == "gate::resolution-in-progress" {
        return Error {
            class: error.class,
            reason: "git::resolution-in-progress",
            detail: error.detail,
            remedy: "retry task completion to read the durable terminal result".to_owned(),
        };
    }
    Error {
        class: error.class,
        reason: "git::resolution-evidence-refused",
        detail: format!("{}: {}", error.reason, error.detail),
        remedy: error.remedy,
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "spice-resolution-preflight-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple(),
            ));
            fs::create_dir(&root).expect("create resolution scratch");
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove resolution scratch");
        }
    }

    fn git(root: &Path, arguments: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(arguments)
            .current_dir(root)
            .output()
            .expect("run Git fixture command");
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout)
            .expect("Git fixture output is UTF-8")
            .trim()
            .to_owned()
    }

    fn preflight_repository() -> (Scratch, String) {
        let scratch = Scratch::new();
        git(&scratch.0, &["init", "--quiet"]);
        git(&scratch.0, &["config", "user.name", "Spice Test"]);
        git(
            &scratch.0,
            &["config", "user.email", "spice-test@example.invalid"],
        );
        fs::write(scratch.0.join("tracked.txt"), b"resolved\n").expect("write tracked fixture");
        git(&scratch.0, &["add", "tracked.txt"]);
        git(&scratch.0, &["commit", "--quiet", "-m", "seed"]);
        let head = git(&scratch.0, &["rev-parse", "HEAD"]);
        for (name, value) in [
            ("MERGE_HEAD", format!("{head}\n")),
            ("ORIG_HEAD", format!("{head}\n")),
            ("MERGE_MODE", "no-ff\n".to_owned()),
            ("MERGE_MSG", "resolved task\n".to_owned()),
        ] {
            fs::write(scratch.0.join(".git").join(name), value).expect("write merge-state fixture");
        }
        (scratch, head)
    }

    #[test]
    fn pending_evidence_preserves_the_operation_identity_for_the_command_reply() {
        let operation = "00000000-0000-4000-8000-000000000051";
        let error = evidence_error(ResolutionEvidenceError::refusal(
            "gate::resolution-in-progress",
            operation,
            "retry",
        ));
        assert_eq!(error.reason(), "git::resolution-in-progress");
        assert_eq!(error.detail(), operation);
        assert_eq!(error.class(), ErrorClass::Refusal);
    }

    #[test]
    fn preflight_refuses_configuration_movement_during_evidence() {
        let (scratch, head) = preflight_repository();
        let preflight = Preflight::capture(&scratch.0).expect("capture exact preflight");
        git(
            &scratch.0,
            &["config", "spice.fixture-moved", "during-evidence"],
        );
        let target = Target {
            remote: "origin".to_owned(),
            baseline: "HEAD".to_owned(),
            branch: "main".to_owned(),
        };

        let error = preflight
            .complete(&scratch.0, &target, &head, &head, "resolved task")
            .expect_err("moved configuration is refused");
        assert_eq!(error.reason(), "git::resolution-state-moved");
    }
}

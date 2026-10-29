//! Native rim for the digest-bound Spice installer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use spice::ambient::Ambient;
use spice::install::{InstallError, InstallPreview, InstallReceipt};
use spice::repository::Repository;
use spice::repository::StateError;
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};
const PROCESS_PATH: &str = "PATH";

pub(crate) enum Answer {
    Plan(Outcome<PlanPreview>),
    Receipt(Outcome<InstallReceipt>),
    Child(ExitStatus),
}

/// An install preview with the exact command that applies its digest.
///
/// A package preview's plan comes from the verified staged executable, which
/// is gone once the preview returns; replaying bare `install --apply` would
/// ask the installed appliance to derive a different plan from its own
/// executable. The command therefore reopens the original package source.
/// The command is display only: the digest still comes from the preview, and
/// apply derives and checks the plan afresh.
pub(crate) struct PlanPreview {
    preview: InstallPreview,
    apply: String,
}

impl PlanPreview {
    fn source(preview: InstallPreview, program: &str) -> Self {
        let apply = source_apply_command(program, preview.digest());
        Self { preview, apply }
    }

    fn package(preview: InstallPreview, program: &str, package: &str) -> Self {
        let apply = package_apply_command(program, package, preview.digest());
        Self { preview, apply }
    }
}

impl serde::Serialize for PlanPreview {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.preview.serialize(serializer)
    }
}

impl fmt::Display for PlanPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.preview.write_text(formatter, &self.apply)
    }
}

pub(crate) fn run(matches: &clap::ArgMatches, ambient: &Ambient) -> Answer {
    let request = if matches.get_flag("unapply") {
        Request::Unapply
    } else if let Some(digest) = matches.get_one::<String>("apply") {
        Request::Apply(digest.clone())
    } else {
        Request::Plan
    };
    let working = match std::env::current_dir() {
        Ok(working) => working,
        Err(error) => {
            return request.fault("install::working-directory-unavailable", error.to_string());
        }
    };
    let repository = match Repository::discover(&working) {
        Ok(repository) => repository,
        Err(error) => {
            return request.fault(spice::repository::ResolveError::REASON, error.to_string());
        }
    };
    if let Some(package) = matches.get_one::<String>("package") {
        if matches!(request, Request::Unapply) {
            return request.package_failure(crate::install_package::Error::input(
                "a package source cannot be combined with --unapply; the installed receipt owns reversal",
            ));
        }
        let apply = match &request {
            Request::Apply(digest) => Some(digest.as_str()),
            Request::Plan => None,
            Request::Unapply => unreachable!("package unapply returned above"),
        };
        let json = matches.get_flag("json");
        if apply.is_none() && !json {
            return match crate::install_package::preview(&repository, Path::new(package)) {
                Ok(outcome) => Answer::Plan(package_preview(package, outcome)),
                Err(error) => request.package_failure(error),
            };
        }
        return match crate::install_package::run(&repository, Path::new(package), apply, json) {
            Ok(status) => Answer::Child(status),
            Err(error) => request.package_failure(error),
        };
    }
    match request {
        Request::Plan => {
            let executable = match executable(ambient) {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Plan(outcome),
            };
            Answer::Plan(result(
                spice::install::preview(&repository, Some(&executable), ambient.get(PROCESS_PATH))
                    .map(|preview| PlanPreview::source(preview, &invoked_program())),
            ))
        }
        Request::Apply(spelling) => {
            let digest = match spelling.parse::<Digest>() {
                Ok(digest) => digest,
                Err(error) => {
                    return Answer::Receipt(Outcome::Refusal(Refusal::bare(
                        wire::RefusalDiagnostic::new(
                            "install::digest-refused".to_owned(),
                            Some("install-plan".to_owned()),
                            Some("canonical install plan digest".to_owned()),
                            Some(spelling),
                        )
                        .with_detail(error.to_string()),
                    )));
                }
            };
            let executable = match executable(ambient) {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Receipt(outcome),
            };
            Answer::Receipt(result(spice::install::apply(
                &repository,
                Some(&executable),
                &digest,
                ambient.get(PROCESS_PATH),
            )))
        }
        Request::Unapply => Answer::Receipt(result(spice::install::unapply(&repository))),
    }
}

enum Request {
    Plan,
    Apply(String),
    Unapply,
}

impl Request {
    fn fault(&self, reason: &str, detail: String) -> Answer {
        let fault = Fault {
            class: FaultClass::Environment,
            reason: reason.to_owned(),
            detail,
        };
        match self {
            Self::Plan => Answer::Plan(Outcome::Fault(fault)),
            Self::Apply(_) | Self::Unapply => Answer::Receipt(Outcome::Fault(fault)),
        }
    }

    fn package_failure(&self, error: crate::install_package::Error) -> Answer {
        match self {
            Self::Plan => Answer::Plan(package_failure(&error)),
            Self::Apply(_) | Self::Unapply => Answer::Receipt(package_failure(&error)),
        }
    }
}

fn package_failure<B>(error: &crate::install_package::Error) -> Outcome<B> {
    use crate::install_package::Error;
    match error {
        Error::Environment { .. } => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::package-environment-failed".to_owned(),
            detail: error.to_string(),
        }),
        Error::Unreadable { .. } => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "install::package-answer-unreadable".to_owned(),
            detail: error.to_string(),
        }),
        Error::Input(_) => Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "install::package-refused".to_owned(),
                Some("sealed-package".to_owned()),
                Some("one exact current-platform release package closure".to_owned()),
                Some(error.to_string()),
            )
            .with_detail(
                "select the explicit local root of a canonical sealed release package and retry; no repository authority changed",
            ),
        )),
    }
}

fn executable<B>(ambient: &Ambient) -> Result<PathBuf, Outcome<B>> {
    #[cfg(debug_assertions)]
    if let Some(executable) = ambient.get(spice::install::TEST_EXECUTABLE) {
        return Ok(PathBuf::from(executable));
    }
    #[cfg(not(debug_assertions))]
    let _ = ambient;
    std::env::current_exe().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::executable-unresolved".to_owned(),
            detail: error.to_string(),
        })
    })
}

/// A sealed package's preview as this front-end renders it: the child's
/// structured preview with the command that applies it.
fn package_preview(package: &str, outcome: Outcome<InstallPreview>) -> Outcome<PlanPreview> {
    match outcome {
        Outcome::Reply(Reply { position, body }) => Outcome::Reply(Reply {
            position,
            body: PlanPreview::package(body, &invoked_program(), package),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn result<B>(result: Result<B, InstallError>) -> Outcome<B> {
    match result {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

pub(crate) fn failure<B>(error: InstallError) -> Outcome<B> {
    match &error {
        InstallError::PathShape(_)
        | InstallError::PathOutsideRoot(_)
        | InstallError::ParentChanged(_)
        | InstallError::GenerationChanged(_)
        | InstallError::BinaryNotRegular(_)
        | InstallError::BinaryEmpty(_)
        | InstallError::OccupantUnrestorable { .. }
        | InstallError::HookPathUnrepresentable { .. }
        | InstallError::SymbolsRefused { .. }
        | InstallError::ForeignHookPath { .. }
        | InstallError::ReferenceStorage { .. }
        | InstallError::GitConfigurationDrift { .. }
        | InstallError::BinaryContextChanged
        | InstallError::DigestMismatch { .. }
        | InstallError::LegacyRecord { .. }
        | InstallError::CrossingRecord(_)
        | InstallError::CanvasRefused { .. }
        | InstallError::RecordMissing
        | InstallError::ApplyInProgress
        | InstallError::UnapplyInProgress
        | InstallError::Drift { .. } => install_refusal(
            &error,
            "install-plan",
            "the exact current Rust install plan and asset state",
        ),
        InstallError::State(
            StateError::Linked { .. }
            | StateError::NotDirectory { .. }
            | StateError::NotRegularFile { .. },
        ) => install_refusal(
            &error,
            "install-state",
            &format!(
                "unlinked current-release Rust state under {}",
                wire::STATE_DIRECTORY
            ),
        ),
        InstallError::Repository(_)
        | InstallError::State(StateError::Filesystem { .. })
        | InstallError::Io { .. }
        | InstallError::GitLaunch { .. }
        | InstallError::GitExit { .. }
        | InstallError::GitNonUtf8(_) => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::environment-failed".to_owned(),
            detail: error.to_string(),
        }),
        InstallError::Encode(_) | InstallError::Decode(_) | InstallError::InvalidRecord(_) => {
            Outcome::Fault(Fault {
                class: FaultClass::Store,
                reason: "install::state-failed".to_owned(),
                detail: error.to_string(),
            })
        }
    }
}

fn install_refusal<B>(error: &InstallError, witness: &str, expected: &str) -> Outcome<B> {
    let observed = error.to_string();
    let detail = format!("{observed}; {}", install_refusal_remedy(error));
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            install_refusal_reason(error).to_owned(),
            Some(witness.to_owned()),
            Some(expected.to_owned()),
            Some(observed),
        )
        .with_detail(detail),
    ))
}

fn install_refusal_reason(error: &InstallError) -> &'static str {
    match error {
        InstallError::DigestMismatch { .. } => "install::digest-moved",
        InstallError::HookPathUnrepresentable { .. } => "install::hook-path-unrepresentable",
        InstallError::SymbolsRefused { .. } => "install::symbols-refused",
        InstallError::OccupantUnrestorable { .. } => "install::occupant-unrestorable",
        InstallError::ForeignHookPath { .. } => "install::foreign-hooks",
        InstallError::ReferenceStorage { .. } => "install::ref-storage-unsupported",
        InstallError::GitConfigurationDrift { .. } => "install::hook-configuration-drift",
        InstallError::LegacyRecord { .. } => "install::release-owned-state",
        InstallError::CrossingRecord(_) => "install::crossing-pending",
        InstallError::CanvasRefused { .. } => "install::canvas-refused",
        InstallError::RecordMissing => "install::record-missing",
        InstallError::ApplyInProgress | InstallError::UnapplyInProgress => {
            "install::transition-incomplete"
        }
        InstallError::Drift { .. } => "install::drift",
        InstallError::PathOutsideRoot(_) => "install::path-outside-root",
        InstallError::ParentChanged(_) => "install::parent-changed",
        InstallError::GenerationChanged(_) => "install::generation-changed",
        InstallError::State(_) => "install::state-refused",
        _ => "install::input-refused",
    }
}

fn install_refusal_remedy(error: &InstallError) -> String {
    if let Some(remedy) = hook_configuration_remedy(error) {
        return remedy;
    }
    if let Some(remedy) = state_remedy(error) {
        return remedy;
    }
    match error {
        InstallError::PathShape(path) => format!(
            "inspect the install target with `{}` and restore it to an unlinked regular file or directory before rerunning this executable's `install` preview",
            inspect_path(path),
        ),
        InstallError::PathOutsideRoot(path) => format!(
            "inspect the escaping install path with `{}` and repair the named symlink or record path so it stays within its permitted Rust root",
            inspect_path(path),
        ),
        InstallError::ParentChanged(path) => format!(
            "inspect the changed destination parent with `{}`; rerun this executable's `install` preview and use only its newly printed digest",
            inspect_path(path),
        ),
        InstallError::GenerationChanged(path) => format!(
            "inspect the concurrently changed install receipt with `{}`; keep the completed direction and rerun this executable's `install` preview before retrying",
            inspect_path(path),
        ),
        InstallError::BinaryNotRegular(path) | InstallError::BinaryEmpty(path) => format!(
            "inspect the selected executable with `{}`; install an appliance that links its canvas archive, built by `spice dev xtask -- install` or taken from a sealed release package",
            inspect_path(path),
        ),
        InstallError::HookPathUnrepresentable { path, .. } => format!(
            "move or recreate the repository at a UTF-8 path with no line separators, then rerun this executable's install preview; the refused Git-common path was {path:?}",
        ),
        InstallError::ReferenceStorage { common, format } => format!(
            "Spice gates commits by reading references through gix, which has no {format} backend; migrate the repository with `{GIT} refs migrate --ref-format=files` in {}, or create it with `{GIT} init --ref-format=files`, then rerun this executable's `install` preview",
            common.display(),
        ),
        InstallError::OccupantUnrestorable { path, .. } => format!(
            "inspect the existing target with `{}`; move it out of the install tree, or restrict it to owner-only access, then preview again",
            inspect_path(path),
        ),
        InstallError::SymbolsRefused { path, .. } => format!(
            "inspect the package symbol input with `{}`; re-acquire the release package so its executable, payload, and `symbols/spice.symbols` come from one sealed build, then rerun `spice install --package`",
            inspect_path(path),
        ),
        InstallError::BinaryContextChanged => format!(
            "the `{INSTALLED_SPICE}` asset was planned from another executable; rerun \
             `spice dev xtask -- install`, which plans and applies one linked build, or preview a \
             sealed release package and pass that preview's digest to the same `install --package`",
        ),
        InstallError::DigestMismatch { .. } => concat!(
            "rerun this executable's `install` preview and pass its newly printed digest to ",
            "the same executable as `install --apply=<digest>`; do not reuse the refused digest",
        )
        .to_owned(),
        InstallError::LegacyRecord { path, owner } => format!(
            "inspect the {owner}-owned record with `{}`; run that retired release's own executable with `install --unapply` before retrying the current preview",
            inspect_path(path),
        ),
        InstallError::CrossingRecord(_) => format!(
            "run this executable's `install` preview and apply its printed digest, which carries the schema-19 `{}/install.json` record forward, then retry",
            wire::STATE_DIRECTORY
        ),
        InstallError::CanvasRefused { path, .. } => format!(
            "inspect the selected executable with `{}`; install an appliance that links its canvas archive, built by `spice dev xtask -- install` or taken from a sealed release package",
            inspect_path(path),
        ),
        InstallError::RecordMissing => format!(
            "the `{}/install.json` record is absent; omit `--unapply` and run this executable's `install` preview instead",
            wire::STATE_DIRECTORY
        ),
        InstallError::ApplyInProgress => format!(
            "the `{}/install.json` record notes an interrupted apply; rerun this executable's `install` preview, finish `install --apply=<printed-digest>`, then retry `--unapply`",
            wire::STATE_DIRECTORY
        ),
        InstallError::UnapplyInProgress => format!(
            "the `{}/install.json` record notes an interrupted reversal; rerun this executable with `install --unapply` to completion before previewing another install",
            wire::STATE_DIRECTORY
        ),
        InstallError::Drift { path, .. } => format!(
            "resolve the asset root with `{GIT} rev-parse --path-format=absolute --git-common-dir`, inspect its install target with `{}`, and preserve the operator replacement or restore the recorded installed image before retrying reversal",
            inspect_path(&Path::new("<printed-common-git>").join(path)),
        ),
        _ => {
            unreachable!("fault-only install errors never construct refusal guidance")
        }
    }
}

fn hook_configuration_remedy(error: &InstallError) -> Option<String> {
    match error {
        InstallError::ForeignHookPath { worktree, .. } => Some(format!(
            "inspect `{GIT} config --show-origin --show-scope --get-all core.hooksPath` in {}; make system, global, and command scopes contribute no value, then deliberately replace only the exact refused common-local or worktree-local value before previewing again",
            worktree.display(),
        )),
        InstallError::GitConfigurationDrift {
            worktree, scope, ..
        } => Some(format!(
            "inspect `{GIT} config --show-origin --show-scope --get-all core.hooksPath` in {}, then restore the exact previewed or installed vector in its {scope:?} scope; system, global, and command scopes must contribute no value",
            worktree.display(),
        )),
        _ => None,
    }
}

fn state_remedy(error: &InstallError) -> Option<String> {
    match error {
        InstallError::State(StateError::Linked { path }) => Some(format!(
            "inspect the Rust state link with `{}` and replace it with the fixed unlinked state component before retrying preview",
            inspect_path(path),
        )),
        InstallError::State(StateError::NotDirectory { path }) => Some(format!(
            "inspect the Rust state parent with `{}` and restore that fixed component as a directory before retrying preview",
            inspect_path(path),
        )),
        InstallError::State(StateError::NotRegularFile { path }) => Some(format!(
            "inspect the Rust state leaf with `{}` and restore that fixed component as a regular file before retrying preview",
            inspect_path(path),
        )),
        _ => None,
    }
}

/// The primary Git command the installer resolves, spelled for the operator's shell.
const GIT: &str = "git";

#[cfg(unix)]
const INSTALLED_SPICE: &str = ".spice-install/bin/spice";
#[cfg(windows)]
const INSTALLED_SPICE: &str = ".spice-install/bin/spice.exe";

#[cfg(unix)]
fn inspect_path(path: &Path) -> String {
    shell_words::join([
        "ls".to_owned(),
        "-ld".to_owned(),
        "--".to_owned(),
        path.to_string_lossy().into_owned(),
    ])
}

#[cfg(windows)]
fn inspect_path(path: &Path) -> String {
    format!("icacls \"{}\"", path.display())
}

/// The program word that reruns this executable as this invocation named it.
///
/// A preview names its plan from this executable's bytes, so a bare `spice`
/// that resolves elsewhere (or nowhere, before `.spice-install/bin` is on
/// `PATH`) would derive another digest; replaying argv[0] reaches the same file.
fn invoked_program() -> String {
    std::env::args_os().next().map_or_else(
        || "spice".to_owned(),
        |program| program.to_string_lossy().into_owned(),
    )
}

#[cfg(unix)]
fn source_apply_command(program: &str, digest: &Digest) -> String {
    format!("{} install --apply={digest}", program_word(program))
}

#[cfg(windows)]
fn source_apply_command(program: &str, digest: &Digest) -> String {
    windows_apply_command(program, None, digest)
}

#[cfg(unix)]
fn program_word(program: &str) -> String {
    shell_words::quote(program).into_owned()
}

/// Render one Windows install replay with a literal PowerShell program word.
#[cfg(any(windows, test))]
fn windows_apply_command(program: &str, package: Option<&str>, digest: &Digest) -> String {
    let apostrophes = program.bytes().filter(|byte| *byte == b'\'').count();
    let package_len = package.map_or(0, |path| " --package \"\"".len() + path.len());
    let mut command = String::with_capacity(
        "& ''".len()
            + program.len()
            + apostrophes
            + " install".len()
            + package_len
            + " --apply=".len()
            + digest.as_str().len(),
    );
    command.push_str("& '");
    for character in program.chars() {
        if character == '\'' {
            command.push_str("''");
        } else {
            command.push(character);
        }
    }
    command.push_str("' install");
    if let Some(package) = package {
        command.push_str(" --package \"");
        command.push_str(package);
        command.push('"');
    }
    command.push_str(" --apply=");
    command.push_str(digest.as_str());
    command
}

/// The command that reopens `package` and applies its previewed `digest`.
#[cfg(unix)]
fn package_apply_command(program: &str, package: &str, digest: &Digest) -> String {
    format!(
        "{} install --package {} --apply={digest}",
        program_word(program),
        shell_words::quote(package)
    )
}

/// The command that reopens `package` and applies its previewed `digest`.
#[cfg(windows)]
fn package_apply_command(program: &str, package: &str, digest: &Digest) -> String {
    windows_apply_command(program, Some(package), digest)
}

#[cfg(test)]
mod tests;

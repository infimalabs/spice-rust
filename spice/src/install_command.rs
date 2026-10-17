//! Native rim for the digest-bound Spice installer.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use spice::install::{InstallError, InstallPreview, InstallReceipt};
use spice::repository::Repository;
use spice::repository::StateError;
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

pub(crate) enum Answer {
    Plan(Outcome<InstallPreview>),
    Receipt(Outcome<InstallReceipt>),
    Child(ExitStatus),
}

pub(crate) fn run(matches: &clap::ArgMatches) -> Answer {
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
        return match crate::install_package::run(
            &repository,
            Path::new(package),
            apply,
            matches.get_flag("json"),
        ) {
            Ok(status) => Answer::Child(status),
            Err(error) => request.package_failure(error),
        };
    }
    match request {
        Request::Plan => {
            let executable = match executable() {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Plan(outcome),
            };
            Answer::Plan(result(
                spice::install::plan(&repository, Some(&executable))
                    .and_then(InstallPreview::from_plan),
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
            let executable = match executable() {
                Ok(executable) => executable,
                Err(outcome) => return Answer::Receipt(outcome),
            };
            Answer::Receipt(result(spice::install::apply(
                &repository,
                Some(&executable),
                &digest,
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
    if error.is_environment() {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::package-environment-failed".to_owned(),
            detail: error.to_string(),
        })
    } else {
        Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "install::package-refused".to_owned(),
                Some("sealed-package".to_owned()),
                Some("one exact current-platform release package closure".to_owned()),
                Some(error.to_string()),
            )
            .with_detail(
                "select the explicit local root of a canonical sealed release package and retry; no repository authority changed",
            ),
        ))
    }
}

fn executable<B>() -> Result<PathBuf, Outcome<B>> {
    #[cfg(debug_assertions)]
    if let Some(executable) = crate::bootstrap_environment(spice::install::TEST_EXECUTABLE) {
        return Ok(PathBuf::from(executable));
    }
    std::env::current_exe().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "install::executable-unresolved".to_owned(),
            detail: error.to_string(),
        })
    })
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
        | InstallError::HookPathUnrepresentable { .. }
        | InstallError::SymbolsRefused { .. }
        | InstallError::ForeignHookPath { .. }
        | InstallError::GitConfigurationDrift { .. }
        | InstallError::BinaryContextChanged
        | InstallError::DigestMismatch { .. }
        | InstallError::LegacyRecord { .. }
        | InstallError::RetainedRecord { .. }
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
            "unlinked current-release Rust state under .spice-rs",
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
        InstallError::ForeignHookPath { .. } => "install::foreign-hooks",
        InstallError::GitConfigurationDrift { .. } => "install::hook-configuration-drift",
        InstallError::LegacyRecord { .. } | InstallError::RetainedRecord { .. } => {
            "install::release-owned-state"
        }
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
            "inspect the Rust-owned install target with `{}` and restore it to an unlinked regular file or directory before rerunning this executable's `install` preview",
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
            "inspect the selected executable with `{}`; from the worktree root, rebuild it with `cargo build --locked --target-dir target -p spice`, then rerun `target/debug/spice install`",
            inspect_path(path),
        ),
        InstallError::HookPathUnrepresentable { path, .. } => format!(
            "move or recreate the repository at a UTF-8 path with no line separators, then rerun this executable's install preview; the refused Git-common path was {path:?}",
        ),
        InstallError::SymbolsRefused { path, .. } => format!(
            "inspect the package symbol input with `{}`; re-acquire the release package so its executable, payload, and `symbols/spice.dSYM.json` come from one sealed build, then rerun `spice install --package`",
            inspect_path(path),
        ),
        InstallError::BinaryContextChanged => concat!(
            "the `.spice-install/bin/spice` asset was planned from another executable; preview ",
            "and apply through the same freshly built executable: from the worktree root, run ",
            "`cargo build --locked --target-dir target -p spice`, rerun `target/debug/spice install`, then pass ",
            "that preview's digest to `target/debug/spice install --apply=<digest>`",
        )
        .to_owned(),
        InstallError::DigestMismatch { .. } => concat!(
            "rerun this executable's `install` preview and pass its newly printed digest to ",
            "the same executable as `install --apply=<digest>`; do not reuse the refused digest",
        )
        .to_owned(),
        InstallError::LegacyRecord { path, owner } => format!(
            "inspect the {owner}-owned record with `{}`; run that retired release's own executable with `install --unapply` before retrying the current preview",
            inspect_path(path),
        ),
        InstallError::RetainedRecord { path, owner, .. } => format!(
            "inspect the schema-18 record with `{}`; a complete one crosses forward ({owner}) only through this executable's `install` preview and `install --apply=<printed-digest>`, so finish or unapply an interrupted one with the schema-18 release's own executable first",
            inspect_path(path),
        ),
        InstallError::CanvasRefused { path, .. } => format!(
            "inspect the selected executable with `{}`; install an appliance that links its canvas archive, built by `spice dev xtask -- install` or taken from a sealed release package",
            inspect_path(path),
        ),
        InstallError::RecordMissing => concat!(
            "the Rust-owned `.spice-rs/install.json` record is absent; omit `--unapply` and ",
            "run this executable's `install` preview instead",
        )
        .to_owned(),
        InstallError::ApplyInProgress => concat!(
            "the Rust-owned `.spice-rs/install.json` records an interrupted apply; rerun this ",
            "executable's `install` preview, finish `install --apply=<printed-digest>`, then retry `--unapply`",
        )
        .to_owned(),
        InstallError::UnapplyInProgress => concat!(
            "the Rust-owned `.spice-rs/install.json` records an interrupted reversal; rerun this ",
            "executable with `install --unapply` to completion before previewing another install",
        )
        .to_owned(),
        InstallError::Drift { path, .. } => format!(
            "resolve the asset root with `/usr/bin/git rev-parse --path-format=absolute --git-common-dir`, inspect its Rust-owned target with `ls -ld -- <printed-common-git>/{path}`, and preserve the operator replacement or restore the recorded installed image before retrying reversal",
        ),
        _ => {
            unreachable!("fault-only install errors never construct refusal guidance")
        }
    }
}

fn hook_configuration_remedy(error: &InstallError) -> Option<String> {
    match error {
        InstallError::ForeignHookPath { worktree, .. } => Some(format!(
            "inspect `/usr/bin/git config --show-origin --show-scope --get-all core.hooksPath` in {}; make system, global, and command scopes contribute no value, then deliberately replace only the exact refused common-local or worktree-local value before previewing again",
            worktree.display(),
        )),
        InstallError::GitConfigurationDrift {
            worktree, scope, ..
        } => Some(format!(
            "inspect `/usr/bin/git config --show-origin --show-scope --get-all core.hooksPath` in {}, then restore the exact previewed or installed vector in its {scope:?} scope; system, global, and command scopes must contribute no value",
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

fn inspect_path(path: &Path) -> String {
    shell_words::join([
        "ls".to_owned(),
        "-ld".to_owned(),
        "--".to_owned(),
        path.to_string_lossy().into_owned(),
    ])
}

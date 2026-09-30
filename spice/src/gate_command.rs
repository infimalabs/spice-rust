//! The journal-free binary rim for the four executable gate leaves.
//!
//! Selection always precedes contract loading. Consequently tracked policy,
//! tracked floors, source bytes, and the root handed to every bounded process
//! all come from one captured Git source rather than from a moving checkout.

use std::env;
use std::io;
use std::path::Path;

use gate::{BatteryReport, CampReport, KillReport, ReportMutationMovement, Selector};
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use spice::repository::Repository;

mod commit_message;
mod pre_commit;
mod prove;
mod reference_transaction;
mod verify_claim;

pub(crate) use prove::prove;

/// Run the ordinary gate against the captured index or one selected commit.
pub(crate) fn run(
    matches: &clap::ArgMatches,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<BatteryReport> {
    let selector = matches
        .get_one::<String>("at")
        .map_or_else(Selector::staged, |revision| {
            Selector::revision(revision.clone())
        });
    let admission = verify_claim::admit_current_worktree(configuration);
    match verify_claim::launch_after_admission(admission, || spice::gate::run(selector)) {
        Ok(Ok(report)) => direct(report),
        Ok(Err(error)) => battery_failure(error),
        Err(failure) => failure.outcome(),
    }
}

/// Run the release-scoped native staged-change gate without command recursion.
pub(crate) fn pre_commit(
    matches: &clap::ArgMatches,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<pre_commit::Report> {
    pre_commit::run(
        matches
            .get_one::<String>("profile-output")
            .map(std::path::PathBuf::from),
        configuration,
    )
}

/// Run the current source repository's strict ordinary change selection.
///
/// This leaf is the policy-free destination for source-development delegates:
/// Cargo may build this product command, but no xtask command or hook owns a
/// second selection implementation.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "evidence")]
pub(crate) enum CurrentSourceSelectionReport {
    /// Full validated evidence for a direct operator invocation.
    Focused {
        report: Box<spice::change_gate::ChangeGateReport>,
    },
    /// Compact success identity for the bounded configured-command supervisor.
    ///
    /// The source gate has already constructed and validated the full report;
    /// its supervisor consumes only the process disposition and exact source.
    Supervised { source: Digest },
}

pub(crate) fn selection() -> Outcome<CurrentSourceSelectionReport> {
    let supervised = configured_selection_progress();
    let _containment = if supervised {
        match gate::measure::inherit_process_group() {
            Ok(containment) => Some(containment),
            Err(detail) => {
                return Outcome::Fault(Fault {
                    class: FaultClass::Environment,
                    reason: "gate::selection-containment-unavailable".to_owned(),
                    detail: detail.to_owned(),
                });
            }
        }
    } else {
        None
    };
    let mut progress = SelectionProgress::new(supervised);
    match spice::change_gate::run_current_source(&mut progress) {
        Ok(report) if report.admits() && supervised => {
            direct(CurrentSourceSelectionReport::Supervised {
                source: report.selection().source().clone(),
            })
        }
        Ok(report) if report.admits() => direct(CurrentSourceSelectionReport::Focused {
            report: Box::new(report),
        }),
        Ok(report) => {
            let terminal = format!("{}selected gate regressed", report.refusal_fold(),);
            Outcome::Refusal(Refusal::evidence(
                "gate::selection-regressed",
                progress.failure_detail(&terminal),
                "repair the selected change before committing it",
            ))
        }
        Err(error) if error.is_fault() => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: error.reason().to_owned(),
            detail: progress.failure_detail(error.detail()),
        }),
        Err(error) => Outcome::Refusal(Refusal::evidence(
            error.reason(),
            progress.failure_detail(error.detail()),
            "repair the named selection input, then rerun the source gate",
        )),
    }
}

struct SelectionProgress {
    buffered: Vec<u8>,
    live: Option<io::Stderr>,
}

impl SelectionProgress {
    fn new(live: bool) -> Self {
        Self {
            buffered: Vec::new(),
            live: live.then(io::stderr),
        }
    }

    fn failure_detail(&self, terminal: &str) -> String {
        if self.live.is_some() {
            return terminal.to_owned();
        }
        selection_detail(&self.buffered, terminal)
    }
}

impl io::Write for SelectionProgress {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(live) = &mut self.live {
            live.write_all(bytes)?;
        }
        self.buffered.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.live.as_mut().map_or(Ok(()), io::Write::flush)
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the configured source-gate leaf detects authority injected by its supervising worker"
)]
fn configured_selection_progress() -> bool {
    env::var_os(wire::SpiceVariable::PreCommitSource.as_str()).is_some()
}

fn selection_detail(progress: &[u8], detail: &str) -> String {
    if progress.is_empty() {
        return detail.to_owned();
    }
    format!("{}{detail}", String::from_utf8_lossy(progress))
}

pub(crate) use commit_message::Run as CommitMessageRun;

/// Apply the current repository commit-message policy to Git's message file.
pub(crate) fn commit_message(
    matches: &clap::ArgMatches,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> CommitMessageRun {
    commit_message::run(matches, configuration)
}

pub(crate) fn pre_commit_worker_entry(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    let arguments = arguments.collect::<Vec<_>>();
    let has_json = arguments
        .iter()
        .skip(1)
        .any(|argument| argument.as_os_str() == std::ffi::OsStr::new("--json"));
    if !has_json && let Some(code) = reference_transaction::entry(arguments.iter().cloned()) {
        return Some(code);
    }
    let reference_arguments = parser_admitted_json_reference_arguments(&arguments);
    reference_arguments
        .and_then(|arguments| reference_transaction::entry(arguments.into_iter()))
        .or_else(|| pre_commit::worker_entry(arguments.into_iter()))
}

fn parser_admitted_json_reference_arguments(
    arguments: &[std::ffi::OsString],
) -> Option<Vec<std::ffi::OsString>> {
    let (program, operands) = arguments.split_first()?;
    let normalized = std::iter::once(program.clone())
        .chain(
            operands
                .iter()
                .filter(|argument| argument.as_os_str() != std::ffi::OsStr::new("--json"))
                .cloned(),
        )
        .collect::<Vec<_>>();
    if normalized.get(1).map(std::ffi::OsString::as_os_str) != Some(std::ffi::OsStr::new("gate"))
        || normalized.get(2).map(std::ffi::OsString::as_os_str)
            != Some(std::ffi::OsStr::new("reference-transaction"))
    {
        return None;
    }
    crate::cli::command()
        .try_get_matches_from(arguments.iter().cloned())
        .ok()?;
    Some(normalized)
}

/// Audit the compiled shared bound without changing a tracked floor.
pub(crate) fn camp() -> Outcome<CampReport> {
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    match selected.contract.audit_camp(selected.root()) {
        Ok(report) => direct(report),
        Err(error) => Failure::run("gate::camp-refused", error).outcome(),
    }
}

/// Run the compiled mutation profile against private copies of the index.
pub(crate) fn kill() -> Outcome<KillReport> {
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    match selected.contract.execute_kill(selected.root()) {
        Ok(run) => kill_report(run.report()),
        Err(error) => Failure::run("gate::kill-refused", error).outcome(),
    }
}

fn kill_report(report: KillReport) -> Outcome<KillReport> {
    let refusal = match report.movement() {
        ReportMutationMovement::Regressed { .. } => Some((
            "gate::kill-regressed",
            "confirmed survivor debt to hold or shrink",
            "remove every confirmed survivor; never raise the survivor floor",
        )),
        ReportMutationMovement::Unconfirmed { .. } => Some((
            "gate::kill-unconfirmed",
            "every declared attempt to confirm each mutant",
            "make the flaky, timed-out, or incomplete attempts repeatable, then run the gate again",
        )),
        ReportMutationMovement::Held { .. }
        | ReportMutationMovement::Shrank { .. }
        | ReportMutationMovement::Unbaselined { .. } => None,
    };
    let Some((reason, _expected, summary)) = refusal else {
        return direct(report);
    };
    Outcome::Refusal(Refusal::evidence(reason, report.render(), summary))
}

fn battery_failure<B>(error: spice::gate::RunError) -> Outcome<B> {
    if error.is_fault() {
        return Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: error.reason().to_owned(),
            detail: error.detail().to_owned(),
        });
    }
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            error.reason().to_owned(),
            None,
            Some("one complete canonical Rust battery capture".to_owned()),
            Some(error.detail().to_owned()),
        )
        .with_detail("repair the named capture input; the Rust battery is the sole execution path"),
    ))
}

fn direct<B>(body: B) -> Outcome<B> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body,
    })
}

/// One immutable selected tree and the current contract loaded from its bytes.
struct SelectedContract {
    tree: gate::SelectedTree,
    contract: gate::Contract,
}

impl SelectedContract {
    fn capture(selector: Selector) -> Result<Self, Failure> {
        let current = std::env::current_dir().map_err(|error| Failure::Fault {
            reason: "gate::working-directory-unavailable",
            detail: error.to_string(),
        })?;
        let repository = Repository::discover(&current).map_err(|error| Failure::Fault {
            reason: spice::repository::ResolveError::REASON,
            detail: error.to_string(),
        })?;
        let tree = selector
            .materialize(repository.worktree_root())
            .map_err(|error| Failure::Refusal {
                reason: "gate::selection-refused",
                expected: "one stable captured index or one immutable commit".to_owned(),
                observed: error.to_string(),
                detail: "resolve the named Git source before running the gate".to_owned(),
            })?;
        let contract = gate::Contract::load(tree.root()).map_err(|error| Failure::Refusal {
            reason: "gate::contract-refused",
            expected: "one complete current tracked gate contract".to_owned(),
            observed: error.to_string(),
            detail: "repair the selected spice-rust.toml, baseline.toml, or compiled tool"
                .to_owned(),
        })?;
        Ok(Self { tree, contract })
    }

    fn root(&self) -> &Path {
        self.tree.root()
    }
}

enum Failure {
    Refusal {
        reason: &'static str,
        expected: String,
        observed: String,
        detail: String,
    },
    Fault {
        reason: &'static str,
        detail: String,
    },
}

impl Failure {
    fn run(reason: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Refusal {
            reason,
            expected: "one complete bounded current gate result".to_owned(),
            observed: error.to_string(),
            detail: "restore the declared inputs and tools, then run the gate again".to_owned(),
        }
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal {
                reason,
                expected,
                observed,
                detail,
            } => Outcome::Refusal(Refusal::bare(
                wire::RefusalDiagnostic::new(
                    reason.to_owned(),
                    None,
                    Some(expected),
                    Some(observed),
                )
                .with_detail(detail),
            )),
            Self::Fault { reason, detail } => Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: reason.to_owned(),
                detail,
            }),
        }
    }
}

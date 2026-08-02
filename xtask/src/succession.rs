//! Canonical succession readiness derived from live and tracked authorities.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use spice::cli::{self, Landing};
use spice::config::{Document, Inputs};

pub(super) const LABEL: &str = "succession readiness";
const REPORT_VERSION: u8 = 2;
const DELETION_DOCTRINE_VERSION: u8 = 1;
const LANDED_THRESHOLD_PERCENT: usize = 95;
const PYTHON_INVENTORY_SCRIPT: &str = concat!(
    "from spice.cli.parser import command_path_registry as registry; ",
    "print('\\n'.join(' '.join(path) for path in sorted(registry())))",
);

/// One machine-readable answer from the canonical verifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct Report {
    version: u8,
    admitted: bool,
    rust: RustReport,
    python: PythonReport,
    tracked_configuration: ConfigurationReport,
}

impl Report {
    pub(super) const fn admitted(&self) -> bool {
        self.admitted
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RustReport {
    total: usize,
    landed: usize,
    gap_owned: usize,
    unaccounted: usize,
    landed_basis_points: usize,
    required_landed_percent: usize,
    duplicate_paths: Vec<String>,
    gap_owned_paths: Vec<OwnedGap>,
    unaccounted_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct OwnedGap {
    path: String,
    owner: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct PythonReport {
    installed_total: usize,
    ledger_total: usize,
    pending_paths: Vec<OwnedGap>,
    installed_duplicates: Vec<String>,
    ledger_duplicates: Vec<String>,
    invalid_ledger_dispositions: Vec<String>,
    unconfirmed_deletions: Vec<String>,
    stale_deletion_confirmations: Vec<String>,
    missing_from_ledger: Vec<String>,
    missing_from_installed: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeletionDoctrine {
    version: u8,
    operator_confirmed_deletions: Vec<ConfirmedDeletion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmedDeletion {
    path: String,
    evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ConfigurationReport {
    admitted: bool,
    diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RustLanding {
    Landed,
    GapOwned(String),
    Unaccounted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RustInput {
    path: String,
    landing: RustLanding,
}

/// Inspect the exact worktree and installed predecessor selected by this run.
pub(super) fn inspect(root: &Path, search_path: Option<&OsStr>) -> Result<Report, Error> {
    let ledger_path = root.join("PORTING.md");
    let ledger = read_text(&ledger_path)?;
    let doctrine_path = root.join("LATER.md");
    let doctrine = read_text(&doctrine_path)?;
    let configuration_path = root.join("spice.toml");
    let configuration = read_text(&configuration_path)?;
    let installed = query_installed_python(root, search_path)?;
    let configuration = admit_tracked_configuration(&configuration);
    evaluate(rust_inputs(), &installed, &ledger, &doctrine, configuration)
}

fn rust_inputs() -> Vec<RustInput> {
    cli::contracts()
        .iter()
        .map(|contract| RustInput {
            path: contract.path.join(" "),
            landing: match cli::landing(contract.path) {
                Landing::Landed => RustLanding::Landed,
                Landing::GapOwned(owner) => RustLanding::GapOwned(owner.to_owned()),
                Landing::Unaccounted => RustLanding::Unaccounted,
            },
        })
        .collect()
}

fn read_text(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn query_installed_python(root: &Path, search_path: Option<&OsStr>) -> Result<String, Error> {
    let launcher = find_program("spice", search_path)?;
    query_installed_python_with(root, &launcher)
}

fn find_program(name: &str, search_path: Option<&OsStr>) -> Result<PathBuf, Error> {
    let search = search_path.ok_or(Error::SearchPathUnavailable)?;
    let candidates = std::env::split_paths(search).map(|directory| directory.join(name));
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(Error::InstalledCommandUnavailable(name.to_owned()))
}

fn query_installed_python_with(root: &Path, launcher: &Path) -> Result<String, Error> {
    let bytes = fs::read(launcher).map_err(|source| Error::Io {
        path: launcher.to_path_buf(),
        source,
    })?;
    let first = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .ok_or_else(|| Error::InstalledLauncher(launcher.to_path_buf()))?;
    let first = std::str::from_utf8(first)
        .map_err(|_| Error::InstalledLauncher(launcher.to_path_buf()))?
        .trim_end_matches('\r');
    let interpreter = first
        .strip_prefix("#!")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| Error::InstalledLauncher(launcher.to_path_buf()))?;
    let output = Command::new(&interpreter)
        .args(["-c", PYTHON_INVENTORY_SCRIPT])
        .current_dir(root)
        .output()
        .map_err(|source| Error::PythonLaunch {
            interpreter: interpreter.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::PythonExit {
            interpreter,
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    if !output.stderr.is_empty() {
        return Err(Error::PythonDiagnostic(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| Error::PythonInventoryNotUtf8)
}

fn admit_tracked_configuration(text: &str) -> Result<(), String> {
    spice::config::resolve(Inputs::new(
        Some(Document::new("spice.toml", text)),
        None,
        &[],
    ))
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn evaluate(
    rust: impl IntoIterator<Item = RustInput>,
    installed: &str,
    ledger: &str,
    doctrine: &str,
    configuration: Result<(), String>,
) -> Result<Report, Error> {
    let mut rust_counts = BTreeMap::<String, usize>::new();
    let mut landed = 0_usize;
    let mut gap_owned_paths = Vec::new();
    let mut unaccounted_paths = Vec::new();
    for row in rust {
        *rust_counts.entry(row.path.clone()).or_default() += 1;
        match row.landing {
            RustLanding::Landed => landed += 1,
            RustLanding::GapOwned(owner) => gap_owned_paths.push(OwnedGap {
                path: row.path,
                owner,
            }),
            RustLanding::Unaccounted => unaccounted_paths.push(row.path),
        }
    }
    gap_owned_paths.sort_by(|left, right| left.path.cmp(&right.path));
    unaccounted_paths.sort();
    let duplicate_paths = duplicates(&rust_counts);
    let total: usize = rust_counts.values().sum();
    let landed_basis_points = landed
        .saturating_mul(10_000)
        .checked_div(total)
        .unwrap_or(0);
    let landed_threshold_met =
        total > 0 && landed.saturating_mul(100) >= total.saturating_mul(LANDED_THRESHOLD_PERCENT);

    let installed = parse_inventory(installed);
    let ledger = parse_python_ledger(ledger)?;
    let confirmed_deletions = parse_deletion_doctrine(doctrine)?;
    let unconfirmed_deletions = ledger
        .deletions
        .difference(&confirmed_deletions)
        .cloned()
        .collect::<Vec<_>>();
    let stale_deletion_confirmations = confirmed_deletions
        .difference(&ledger.deletions)
        .cloned()
        .collect::<Vec<_>>();
    let missing_from_ledger = installed
        .paths
        .difference(&ledger.paths)
        .cloned()
        .collect::<Vec<_>>();
    let missing_from_installed = ledger
        .paths
        .difference(&installed.paths)
        .cloned()
        .collect::<Vec<_>>();
    let configuration = match configuration {
        Ok(()) => ConfigurationReport {
            admitted: true,
            diagnostic: None,
        },
        Err(diagnostic) => ConfigurationReport {
            admitted: false,
            diagnostic: Some(diagnostic),
        },
    };

    let rust = RustReport {
        total,
        landed,
        gap_owned: gap_owned_paths.len(),
        unaccounted: unaccounted_paths.len(),
        landed_basis_points,
        required_landed_percent: LANDED_THRESHOLD_PERCENT,
        duplicate_paths,
        gap_owned_paths,
        unaccounted_paths,
    };
    let python = PythonReport {
        installed_total: installed.total,
        ledger_total: ledger.total,
        pending_paths: ledger.pending,
        installed_duplicates: installed.duplicates,
        ledger_duplicates: ledger.duplicates,
        invalid_ledger_dispositions: ledger.invalid,
        unconfirmed_deletions,
        stale_deletion_confirmations,
        missing_from_ledger,
        missing_from_installed,
    };
    let admitted = rust.duplicate_paths.is_empty()
        && rust.unaccounted == 0
        && landed_threshold_met
        && python.installed_duplicates.is_empty()
        && python.ledger_duplicates.is_empty()
        && python.invalid_ledger_dispositions.is_empty()
        && python.unconfirmed_deletions.is_empty()
        && python.stale_deletion_confirmations.is_empty()
        && python.missing_from_ledger.is_empty()
        && python.missing_from_installed.is_empty()
        && configuration.admitted;

    Ok(Report {
        version: REPORT_VERSION,
        admitted,
        rust,
        python,
        tracked_configuration: configuration,
    })
}

struct Inventory {
    total: usize,
    paths: BTreeSet<String>,
    duplicates: Vec<String>,
}

fn parse_inventory(document: &str) -> Inventory {
    let mut counts = BTreeMap::<String, usize>::new();
    for line in document.lines().filter(|line| !line.is_empty()) {
        *counts.entry(line.to_owned()).or_default() += 1;
    }
    Inventory {
        total: counts.values().sum(),
        paths: counts.keys().cloned().collect(),
        duplicates: duplicates(&counts),
    }
}

struct LedgerInventory {
    total: usize,
    paths: BTreeSet<String>,
    duplicates: Vec<String>,
    invalid: Vec<String>,
    deletions: BTreeSet<String>,
    pending: Vec<OwnedGap>,
}

fn parse_python_ledger(document: &str) -> Result<LedgerInventory, Error> {
    let inventory = document
        .split_once("## Installed Python paths")
        .and_then(|(_, suffix)| suffix.split_once("## Reproduce the audit"))
        .map(|(inventory, _)| inventory)
        .ok_or(Error::PythonLedgerBoundary)?;
    let mut counts = BTreeMap::<String, usize>::new();
    let mut invalid = Vec::new();
    let mut deletions = BTreeSet::new();
    let mut pending = Vec::new();
    for line in inventory.lines() {
        let Some(row) = line.strip_prefix("- `") else {
            continue;
        };
        let Some((path, suffix)) = row.split_once('`') else {
            invalid.push(row.to_owned());
            continue;
        };
        *counts.entry(path.to_owned()).or_default() += 1;
        let disposition = suffix.strip_prefix(": ");
        let valid = disposition.is_some_and(|suffix| {
            matches!(suffix, "R." | "D." | "X.")
                || ["R -> ", "D -> ", "X -> ", "P -> `"]
                    .iter()
                    .any(|prefix| suffix.starts_with(prefix))
        });
        if !valid {
            invalid.push(path.to_owned());
        } else if disposition.is_some_and(|suffix| suffix.starts_with('X')) {
            deletions.insert(path.to_owned());
        } else if let Some(owner) = disposition
            .and_then(|suffix| suffix.strip_prefix("P -> `"))
            .and_then(|suffix| suffix.split_once('`').map(|(owner, _)| owner))
        {
            if valid_task_handle(owner) {
                pending.push(OwnedGap {
                    path: path.to_owned(),
                    owner: owner.to_owned(),
                });
            } else {
                invalid.push(path.to_owned());
            }
        }
    }
    invalid.sort();
    pending.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(LedgerInventory {
        total: counts.values().sum(),
        paths: counts.keys().cloned().collect(),
        duplicates: duplicates(&counts),
        invalid,
        deletions,
        pending,
    })
}

fn valid_task_handle(value: &str) -> bool {
    let Some((prefix, suffix)) = value.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix
            .chars()
            .all(|character| character.is_ascii_uppercase())
        && suffix.len() == 8
        && suffix
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

fn parse_deletion_doctrine(document: &str) -> Result<BTreeSet<String>, Error> {
    let section = document
        .split_once("## Operator-confirmed deletions")
        .and_then(|(_, suffix)| suffix.split_once("```json"))
        .and_then(|(_, suffix)| suffix.split_once("```"))
        .map(|(json, _)| json.trim())
        .ok_or_else(|| Error::DeletionDoctrine("missing bounded JSON registry".to_owned()))?;
    let doctrine = serde_json::from_str::<DeletionDoctrine>(section)
        .map_err(|error| Error::DeletionDoctrine(error.to_string()))?;
    if doctrine.version != DELETION_DOCTRINE_VERSION {
        return Err(Error::DeletionDoctrine(format!(
            "version {} is not {}",
            doctrine.version, DELETION_DOCTRINE_VERSION,
        )));
    }

    let mut paths = BTreeSet::new();
    for row in doctrine.operator_confirmed_deletions {
        if row.path.trim() != row.path || row.path.is_empty() {
            return Err(Error::DeletionDoctrine(
                "a confirmed deletion has an empty or non-canonical path".to_owned(),
            ));
        }
        if !valid_ack_evidence(&row.evidence) {
            return Err(Error::DeletionDoctrine(format!(
                "{} has no acknowledgment evidence",
                row.path,
            )));
        }
        if !paths.insert(row.path.clone()) {
            return Err(Error::DeletionDoctrine(format!(
                "{} is confirmed more than once",
                row.path,
            )));
        }
    }
    Ok(paths)
}

fn valid_ack_evidence(value: &str) -> bool {
    value.strip_prefix("ack:").is_some_and(|key| {
        key.len() == 8
            && key
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    })
}

fn duplicates(counts: &BTreeMap<String, usize>) -> Vec<String> {
    counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(path, _)| path.clone())
        .collect()
}

#[cfg(test)]
pub(super) fn fixture_report(admitted: bool) -> Report {
    Report {
        version: REPORT_VERSION,
        admitted,
        rust: RustReport {
            total: 20,
            landed: 20,
            gap_owned: 0,
            unaccounted: 0,
            landed_basis_points: 10_000,
            required_landed_percent: LANDED_THRESHOLD_PERCENT,
            duplicate_paths: Vec::new(),
            gap_owned_paths: Vec::new(),
            unaccounted_paths: Vec::new(),
        },
        python: PythonReport {
            installed_total: 1,
            ledger_total: 1,
            pending_paths: Vec::new(),
            installed_duplicates: Vec::new(),
            ledger_duplicates: Vec::new(),
            invalid_ledger_dispositions: Vec::new(),
            unconfirmed_deletions: Vec::new(),
            stale_deletion_confirmations: Vec::new(),
            missing_from_ledger: Vec::new(),
            missing_from_installed: Vec::new(),
        },
        tracked_configuration: ConfigurationReport {
            admitted: true,
            diagnostic: None,
        },
    }
}

/// Failure to acquire one exact readiness input.
#[derive(Debug)]
pub(super) enum Error {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    SearchPathUnavailable,
    InstalledCommandUnavailable(String),
    InstalledLauncher(PathBuf),
    PythonLaunch {
        interpreter: PathBuf,
        source: io::Error,
    },
    PythonExit {
        interpreter: PathBuf,
        code: Option<i32>,
        stderr: String,
    },
    PythonDiagnostic(String),
    PythonInventoryNotUtf8,
    PythonLedgerBoundary,
    DeletionDoctrine(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::SearchPathUnavailable => {
                formatter.write_str("PATH is unavailable for installed spice discovery")
            }
            Self::InstalledCommandUnavailable(name) => {
                write!(formatter, "installed {name} is absent from PATH")
            }
            Self::InstalledLauncher(path) => write!(
                formatter,
                "installed launcher has no absolute interpreter shebang: {}",
                path.display()
            ),
            Self::PythonLaunch {
                interpreter,
                source,
            } => write!(
                formatter,
                "installed Python interpreter did not launch ({}): {source}",
                interpreter.display()
            ),
            Self::PythonExit {
                interpreter,
                code,
                stderr,
            } => write!(
                formatter,
                "installed Python inventory failed ({}; exit {code:?}): {stderr}",
                interpreter.display()
            ),
            Self::PythonDiagnostic(diagnostic) => {
                write!(
                    formatter,
                    "installed Python inventory wrote stderr: {diagnostic}"
                )
            }
            Self::PythonInventoryNotUtf8 => {
                formatter.write_str("installed Python inventory is not UTF-8")
            }
            Self::PythonLedgerBoundary => {
                formatter.write_str("PORTING.md has no bounded installed Python inventory")
            }
            Self::DeletionDoctrine(diagnostic) => {
                write!(
                    formatter,
                    "LATER.md deletion doctrine is invalid: {diagnostic}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::PythonLaunch { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[cfg(unix)]
    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn exact_ninety_five_percent_is_admitted_and_one_less_refuses() {
        let exact = evaluate(
            rows(19, 1, 0),
            "agent\n",
            &ledger(&["agent"]),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("exact threshold evaluates");
        assert!(exact.admitted);
        assert_eq!(exact.rust.landed_basis_points, 9_500);

        let below = evaluate(
            rows(18, 2, 0),
            "agent\n",
            &ledger(&["agent"]),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("below threshold evaluates");
        assert!(!below.admitted);
        assert_eq!(below.rust.landed_basis_points, 9_000);
    }

    #[test]
    fn current_registries_and_tracked_documents_produce_one_admitted_report() {
        let configuration = include_str!("../../spice.toml");
        let report = evaluate(
            rust_inputs(),
            include_str!("../../spice/tests/fixtures/python-command-registry.txt"),
            include_str!("../../PORTING.md"),
            include_str!("../../LATER.md"),
            admit_tracked_configuration(configuration),
        )
        .expect("current authorities evaluate");

        assert!(report.admitted);
        assert_eq!(
            (report.rust.total, report.rust.landed),
            (cli::contracts().len(), cli::contracts().len())
        );
        assert_eq!(
            (report.python.installed_total, report.python.ledger_total),
            (123, 123)
        );
        assert_eq!(
            report.python.pending_paths.len(),
            0,
            "recovery and demo successors must leave no pending installed Python path",
        );
        assert!(
            report
                .python
                .pending_paths
                .iter()
                .all(|path| path.path != "task capture" && !path.path.starts_with("task artifact"))
        );
        assert!(report.python.unconfirmed_deletions.is_empty());
        assert!(report.python.stale_deletion_confirmations.is_empty());
        let encoded = serde_json::to_value(&report).expect("report encodes");
        assert_eq!(encoded["version"], REPORT_VERSION);
        assert_eq!(encoded["admitted"], true);
    }

    #[test]
    fn rust_report_distinguishes_owned_terminal_refusal_and_unaccounted_dispatch() {
        let report = evaluate(
            rows(18, 1, 1),
            "agent\n",
            &ledger(&["agent"]),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("Rust classes evaluate");

        assert!(!report.admitted);
        assert_eq!((report.rust.total, report.rust.landed), (20, 18));
        assert_eq!(report.rust.gap_owned, 1);
        assert_eq!(report.rust.unaccounted, 1);
        assert_eq!(report.rust.gap_owned_paths[0].owner, "GAP-owner0000");
        assert_eq!(report.rust.unaccounted_paths, ["command-19"]);
    }

    #[test]
    fn duplicate_rust_and_python_paths_are_refused_by_name() {
        let mut rust = rows(19, 1, 0);
        rust.push(rust[0].clone());
        let report = evaluate(
            rust,
            "agent\nagent\n",
            &ledger(&["agent", "agent"]),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("duplicate inputs evaluate");

        assert!(!report.admitted);
        assert_eq!(report.rust.duplicate_paths, ["command-0"]);
        assert_eq!(report.python.installed_duplicates, ["agent"]);
        assert_eq!(report.python.ledger_duplicates, ["agent"]);
    }

    #[test]
    fn python_ledger_drift_is_reported_in_both_directions() {
        let report = evaluate(
            rows(20, 0, 0),
            "agent\ninstalled-only\n",
            &ledger(&["agent", "ledger-only"]),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("drift evaluates");

        assert!(!report.admitted);
        assert_eq!(report.python.missing_from_ledger, ["installed-only"]);
        assert_eq!(report.python.missing_from_installed, ["ledger-only"]);
    }

    #[test]
    fn invalid_python_ledger_disposition_is_refused_by_path() {
        let report = evaluate(
            rows(20, 0, 0),
            "agent\n",
            concat!(
                "# Ledger\n## Installed Python paths\n",
                "- `agent`: Rubbish.\n## Reproduce the audit\n",
            ),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("invalid disposition evaluates");

        assert!(!report.admitted);
        assert_eq!(report.python.invalid_ledger_dispositions, ["agent"]);
        assert!(report.python.missing_from_ledger.is_empty());
        assert!(report.python.missing_from_installed.is_empty());
    }

    #[test]
    fn pending_python_path_requires_a_concrete_task_handle() {
        let report = evaluate(
            rows(20, 0, 0),
            "agent\n",
            concat!(
                "# Ledger\n## Installed Python paths\n",
                "- `agent`: P -> `not-a-task` pending.\n## Reproduce the audit\n",
            ),
            &doctrine(&[]),
            Ok(()),
        )
        .expect("invalid pending owner evaluates");

        assert!(!report.admitted);
        assert_eq!(report.python.invalid_ledger_dispositions, ["agent"]);
        assert!(report.python.pending_paths.is_empty());
    }

    #[test]
    fn live_python_deletion_requires_exact_operator_confirmation() {
        let ledger = concat!(
            "# Ledger\n## Installed Python paths\n",
            "- `agent`: X -> clipped.\n## Reproduce the audit\n",
        );
        let unconfirmed = evaluate(rows(20, 0, 0), "agent\n", ledger, &doctrine(&[]), Ok(()))
            .expect("unconfirmed deletion evaluates");
        assert!(!unconfirmed.admitted);
        assert_eq!(unconfirmed.python.unconfirmed_deletions, ["agent"]);

        let confirmed = evaluate(
            rows(20, 0, 0),
            "agent\n",
            ledger,
            &doctrine(&[("agent", "ack:1kJWMcnL")]),
            Ok(()),
        )
        .expect("confirmed deletion evaluates");
        assert!(confirmed.admitted);
        assert!(confirmed.python.unconfirmed_deletions.is_empty());
        assert!(confirmed.python.stale_deletion_confirmations.is_empty());
    }

    #[test]
    fn deletion_confirmation_cannot_outlive_its_exact_ledger_path() {
        let report = evaluate(
            rows(20, 0, 0),
            "agent\n",
            &ledger(&["agent"]),
            &doctrine(&[("agent", "ack:1kJWMcnL")]),
            Ok(()),
        )
        .expect("stale deletion confirmation evaluates");

        assert!(!report.admitted);
        assert_eq!(report.python.stale_deletion_confirmations, ["agent"]);
    }

    #[test]
    fn malformed_acknowledgment_cannot_enter_deletion_doctrine() {
        let result = evaluate(
            rows(20, 0, 0),
            "agent\n",
            concat!(
                "# Ledger\n## Installed Python paths\n",
                "- `agent`: X -> clipped.\n## Reproduce the audit\n",
            ),
            &doctrine(&[("agent", "ack:short")]),
            Ok(()),
        );

        assert!(matches!(result, Err(Error::DeletionDoctrine(_))));
    }

    #[test]
    fn tracked_configuration_uses_the_current_typed_admission() {
        let current = include_str!("../../spice.toml");
        assert!(admit_tracked_configuration(current).is_ok());
        assert!(admit_tracked_configuration("[unknown]\nvalue = true\n").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn installed_inventory_runs_the_interpreter_named_by_the_launcher() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = Scratch::new();
        let interpreter = root.path.join("python-fixture");
        fs::write(&interpreter, b"#!/bin/sh\nprintf 'agent\\ntask add\\n'\n")
            .expect("fixture interpreter writes");
        fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o755))
            .expect("fixture interpreter is executable");
        let launcher = root.path.join("spice");
        fs::write(&launcher, format!("#!{}\n", interpreter.display()))
            .expect("fixture launcher writes");

        assert_eq!(
            query_installed_python_with(&root.path, &launcher).expect("inventory runs"),
            "agent\ntask add\n"
        );
    }

    fn rows(landed: usize, gaps: usize, unaccounted: usize) -> Vec<RustInput> {
        let mut rows = Vec::new();
        for index in 0..landed {
            rows.push(RustInput {
                path: format!("command-{index}"),
                landing: RustLanding::Landed,
            });
        }
        for offset in 0..gaps {
            let index = landed + offset;
            rows.push(RustInput {
                path: format!("command-{index}"),
                landing: RustLanding::GapOwned(format!("GAP-owner{offset:04}")),
            });
        }
        for offset in 0..unaccounted {
            let index = landed + gaps + offset;
            rows.push(RustInput {
                path: format!("command-{index}"),
                landing: RustLanding::Unaccounted,
            });
        }
        rows
    }

    fn ledger(paths: &[&str]) -> String {
        format!(
            "# Ledger\n## Installed Python paths\n{}## Reproduce the audit\n",
            paths
                .iter()
                .map(|path| format!("- `{path}`: R -> current.\n"))
                .collect::<String>()
        )
    }

    fn doctrine(rows: &[(&str, &str)]) -> String {
        let rows = rows
            .iter()
            .map(|(path, evidence)| serde_json::json!({"path": path, "evidence": evidence}))
            .collect::<Vec<_>>();
        format!(
            "# LATER\n## Operator-confirmed deletions\n```json\n{}\n```\n",
            serde_json::json!({
                "version": DELETION_DOCTRINE_VERSION,
                "operator_confirmed_deletions": rows,
            })
        )
    }

    #[cfg(unix)]
    struct Scratch {
        path: PathBuf,
    }

    #[cfg(unix)]
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "spice-xtask-succession-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("fixture directory creates");
            Self { path }
        }
    }

    #[cfg(unix)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).expect("fixture directory removes");
        }
    }
}

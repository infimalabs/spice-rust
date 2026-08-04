//! Canonical contract landing derived from the tracked authorities.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use spice::cli::{self, Landing};
use spice::config::{Document, Inputs};

pub(super) const LABEL: &str = "contract landing";
const REPORT_VERSION: u8 = 3;
const LANDED_THRESHOLD_PERCENT: usize = 95;

/// One machine-readable answer from the canonical verifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct Report {
    version: u8,
    admitted: bool,
    rust: RustReport,
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

/// Inspect the exact worktree this run was invoked from.
pub(super) fn inspect() -> Result<Report, Error> {
    let root = std::env::current_dir().map_err(|source| Error {
        path: PathBuf::from("."),
        source,
    })?;
    let configuration_path = root.join("spice.toml");
    let configuration = read_text(&configuration_path)?;
    Ok(evaluate(
        rust_inputs(),
        admit_tracked_configuration(&configuration),
    ))
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
    fs::read_to_string(path).map_err(|source| Error {
        path: path.to_path_buf(),
        source,
    })
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
    configuration: Result<(), String>,
) -> Report {
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
    let admitted = rust.duplicate_paths.is_empty()
        && rust.unaccounted == 0
        && landed_threshold_met
        && configuration.admitted;

    Report {
        version: REPORT_VERSION,
        admitted,
        rust,
        tracked_configuration: configuration,
    }
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
        tracked_configuration: ConfigurationReport {
            admitted: true,
            diagnostic: None,
        },
    }
}

/// One acquisition failure, built by the exact reader production uses rather
/// than assembled by hand, so the verifier's refusal path is proved against a
/// shape that can really reach it.
#[cfg(test)]
pub(super) fn fixture_error() -> Error {
    read_text(Path::new("landing-input-that-is-absent"))
        .expect_err("an absent landing input refuses")
}

/// Failure to acquire the one landing input this verifier reads.
#[derive(Debug)]
pub(super) struct Error {
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.source)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_ninety_five_percent_is_admitted_and_one_less_refuses() {
        let exact = evaluate(rows(19, 1, 0), Ok(()));
        assert!(exact.admitted);
        assert_eq!(exact.rust.landed_basis_points, 9_500);

        let below = evaluate(rows(18, 2, 0), Ok(()));
        assert!(!below.admitted);
        assert_eq!(below.rust.landed_basis_points, 9_000);
    }

    #[test]
    fn current_registries_and_tracked_documents_produce_one_admitted_report() {
        let configuration = include_str!("../../spice.toml");
        let report = evaluate(rust_inputs(), admit_tracked_configuration(configuration));

        assert!(report.admitted);
        assert_eq!(
            (report.rust.total, report.rust.landed),
            (cli::contracts().len(), cli::contracts().len())
        );
        let encoded = serde_json::to_value(&report).expect("report encodes");
        assert_eq!(encoded["version"], REPORT_VERSION);
        assert_eq!(encoded["admitted"], true);
    }

    #[test]
    fn rust_report_distinguishes_owned_terminal_refusal_and_unaccounted_dispatch() {
        let report = evaluate(rows(18, 1, 1), Ok(()));

        assert!(!report.admitted);
        assert_eq!((report.rust.total, report.rust.landed), (20, 18));
        assert_eq!(report.rust.gap_owned, 1);
        assert_eq!(report.rust.unaccounted, 1);
        assert_eq!(report.rust.gap_owned_paths[0].owner, "GAP-owner0000");
        assert_eq!(report.rust.unaccounted_paths, ["command-19"]);
    }

    #[test]
    fn duplicate_contract_paths_are_refused_by_name() {
        let mut rust = rows(19, 1, 0);
        rust.push(rust[0].clone());
        let report = evaluate(rust, Ok(()));

        assert!(!report.admitted);
        assert_eq!(report.rust.duplicate_paths, ["command-0"]);
    }

    #[test]
    fn tracked_configuration_uses_the_current_typed_admission() {
        let current = include_str!("../../spice.toml");
        assert!(admit_tracked_configuration(current).is_ok());
        assert!(admit_tracked_configuration("[unknown]\nvalue = true\n").is_err());

        let refused = evaluate(rows(20, 0, 0), Err("unknown key".to_owned()));
        assert!(!refused.admitted);
        assert_eq!(
            refused.tracked_configuration.diagnostic.as_deref(),
            Some("unknown key")
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
}

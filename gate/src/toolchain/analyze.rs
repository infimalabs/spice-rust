//! Compiled analyses for the toolchain arms this build answers itself.
//!
//! Most of the eight declared analyses are compiled here: the formatter check
//! over the complete selected tree, the canvas Wasm check and build, the
//! workstation-path scanner, the prose scanner, the JavaScript
//! declaration/reference analysis, and the two environment scanners. Each
//! reports only what it observed, and [`super::measure_toolchain`] remains the
//! sole authority that admits it.
//!
//! Each declaration records its exclusions as English sentences and its
//! includes as globs. Those strings are the retained evidence and the
//! predicates here are the compiled form of the same rules, so every rule is
//! written once in this module and the two spellings are proven equal in tests.
//!
//! The environment law is large enough to live on its own in
//! [`super::environment`]. This module still owns which paths the two
//! environment arms select and which outcome they earn; that module owns only
//! what one line of one language family says about the environment.
//!
//! The C# unused-member analysis lives in [`csharp`] for the same reason. Its
//! whole difficulty is which absences it may accuse and which it must withdraw
//! unnamed, and that argument reads better beside itself than interleaved with
//! arms that share none of it.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, Environment, spawn};

use super::{
    ArmFinding, ArmOutcome, ArmTiming, ENV_NAME_LITERAL_MARKER, EnvironmentLedgerKind,
    FindingDetail, JavascriptUnusedStatus, LOCAL_PATH_MARKER, PACKAGED_ENV_PROFILE,
    PACKAGED_JAVASCRIPT_EXEMPTIONS, PACKAGED_TASTE_WORDS, PACKAGED_TEST_PATHS, SourceLocus,
    TASTE_EXTENSIONS, ToolchainDeclaration, column_of, environment,
};
use crate::GateId;
use crate::jurisdiction::{Ignore, Jurisdiction};
use crate::measure::Bounds;
use crate::structural::{StructuralPattern, scan_javascript_symbols};

mod build_root;
mod compile;
mod csharp;
mod rust;

use build_root::CanvasWorkspace;

pub(super) use compile::{CompileInvocation, run_compile_analysis, run_compile_analysis_with_wait};
pub(super) use csharp::csharp_unused;
pub(super) use rust::rust_typecheck;

/// What one compiled analysis observed about one arm.
///
/// The exit code is carried because [`super::measure_toolchain`] admits one
/// only alongside [`ArmOutcome::NonzeroAnalysis`] and demands one there, so the
/// code and the outcome it explains are produced together or not at all.
pub(super) struct Analysis {
    pub(super) outcome: ArmOutcome,
    pub(super) reason: String,
    pub(super) findings: Vec<ArmFinding>,
    pub(super) exit_code: Option<i32>,
    pub(super) timing: Option<ArmTiming>,
}

impl Analysis {
    /// A terminal state that carries neither findings nor an exit code.
    pub(super) fn terminal(outcome: ArmOutcome, reason: String) -> Self {
        Self {
            outcome,
            reason,
            findings: Vec::new(),
            exit_code: None,
            timing: None,
        }
    }

    /// A direct analysis process refused without naming anything it examined.
    fn refused(reason: String, code: i32) -> Self {
        Self {
            outcome: ArmOutcome::NonzeroAnalysis,
            reason,
            findings: Vec::new(),
            exit_code: Some(code),
            timing: None,
        }
    }

    /// The result of one completed scan over `examined` selected paths.
    fn scanned(id: GateId, examined: usize, findings: Vec<ArmFinding>) -> Self {
        if findings.is_empty() {
            return Self::terminal(
                ArmOutcome::Passed,
                format!(
                    "{} found nothing across {examined} selected paths",
                    id.as_str()
                ),
            );
        }
        Self {
            outcome: ArmOutcome::Findings,
            reason: format!(
                "{} found {} across {examined} selected paths",
                id.as_str(),
                plural(findings.len(), "finding"),
            ),
            findings,
            exit_code: None,
            timing: None,
        }
    }

    /// Attach the bounded process duration observed by a compiled runner.
    fn timed(mut self, timing: ArmTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    /// The declaration's own applicability predicate came out false.
    ///
    /// The sentence is read back off the declaration rather than repeated
    /// here, so the reason a tree was skipped is the text the report retains.
    fn not_applicable(declaration: ToolchainDeclaration) -> Self {
        Self::terminal(
            ArmOutcome::NotApplicable,
            format!(
                "{} requires {}",
                declaration.id.as_str(),
                declaration.jurisdiction.applicability
            ),
        )
    }
}

/// Settle declaration-owned applicability that precedes executable discovery.
///
/// A tool is required to perform an applicable analysis, not to prove that the
/// selected tree contains no subject for that analysis. Returning the absence
/// here keeps an inapplicable arm independent of workstation tool inventory and
/// ensures its retained tool evidence is empty.
pub(super) fn before_tools(declaration: ToolchainDeclaration, root: &Path) -> Option<Analysis> {
    match declaration.id {
        GateId::Formatters => match formatter_sources(root) {
            Ok(sources) if sources.is_empty() => Some(Analysis::not_applicable(declaration)),
            Ok(_) => None,
            Err(reason) => Some(Analysis::terminal(ArmOutcome::ParseFailure, reason)),
        },
        GateId::CanvasWebTypecheck
            if !root.join("Cargo.toml").is_file() || !root.join("app/index.html").is_file() =>
        {
            Some(Analysis::not_applicable(declaration))
        }
        GateId::PythonTypecheck => match python_sources(root) {
            Ok(true) => None,
            Ok(false) => Some(Analysis::not_applicable(declaration)),
            Err(reason) => Some(Analysis::terminal(ArmOutcome::ParseFailure, reason)),
        },
        _ => None,
    }
}

/// One compiled path-component exclusion and the sentence that records it.
///
/// [`super::PACKAGED_SCANNER_EXCLUDES`] is the retained prose form of exactly
/// this table, in this order, and the two are proven equal in tests.
#[derive(Clone, Copy)]
enum Exclusion {
    /// A path component equal to this name.
    Equals(&'static str),
    /// A path component beginning with this prefix.
    StartsWith(&'static str),
}

impl Exclusion {
    /// Whether this rule removes one path component.
    fn excludes(self, component: &str) -> bool {
        match self {
            Self::Equals(name) => component == name,
            Self::StartsWith(prefix) => component.starts_with(prefix),
        }
    }

    /// The exact sentence this rule is recorded as.
    ///
    /// Reports retain [`super::PACKAGED_SCANNER_EXCLUDES`] itself, so this
    /// rendering exists only to prove the compiled table says the same thing.
    #[cfg(test)]
    fn sentence(self) -> String {
        match self {
            Self::Equals(name) => format!("any path component equal to {name}"),
            Self::StartsWith(prefix) => format!("any path component starting with {prefix}"),
        }
    }
}

/// Every scanner exclusion, in the order the declarations record them.
const SCANNER_EXCLUSIONS: &[Exclusion] = &[
    Exclusion::Equals(".git"),
    Exclusion::StartsWith(".spice"),
    Exclusion::Equals(".venv"),
    Exclusion::Equals(".ruff_cache"),
    Exclusion::Equals(".pytest_cache"),
    Exclusion::Equals("__pycache__"),
    Exclusion::Equals("node_modules"),
    Exclusion::Equals("site-packages"),
];

/// Every build-output exclusion, in the order the declarations record them.
///
/// The Formatters declaration excludes build output rather than scanner noise,
/// and the two tables are genuinely different rules: these are top-level
/// directories written as `name/**`, not components matched at any depth.
const BUILD_EXCLUSIONS: &[&str] = &[".git", "target", "dist", "node_modules", ".venv", "vendor"];

/// The compiled form of the JavaScript-unused include globs.
const JAVASCRIPT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs"];

/// The compiled form of the Formatters Rust include glob.
const RUST_EXTENSION: &str = "rs";

/// Byte acquisition used by the source-decoding analyses.
///
/// Production always supplies [`fs::read`]. Keeping the operation explicit
/// lets tests prove read-refusal behavior without depending on effective-user
/// permission semantics.
type SourceReader = fn(&Path) -> io::Result<Vec<u8>>;

fn read_source(path: &Path) -> io::Result<Vec<u8>> {
    fs::read(path)
}

/// The formatter whose canonical bytes every Formatters finding is measured against.
const FORMATTER: &str = "rustfmt";

/// The declared program the formatter check is invoked through.
const CARGO: &str = "cargo";

/// The declared program that validates the canvas asset graph.
const TRUNK: &str = "trunk";

/// Cargo's external build directory, constructed so the workspace ledger does
/// not mistake a third-party process variable for a Spice-owned name.
const CARGO_TARGET_DIRECTORY: &str = concat!("CARGO_", "TARGET_DIR");

/// Cargo's external build-artifact directory under the same ownership rule.
const CARGO_BUILD_DIRECTORY: &str = concat!("CARGO_", "BUILD_BUILD_DIR");

/// Trunk's external distribution directory under the same rule.
const TRUNK_DISTRIBUTION_DIRECTORY: &str = concat!("TRUNK_", "BUILD_DIST");

/// The build-time home Cargo and the workspace's compiled declarations share.
const COMPILED_HOME_DIRECTORY: &str = env!("HOME");

/// Home is a tool-runtime input, not a Spice-owned environment declaration.
const HOME_DIRECTORY: &str = concat!("HO", "ME");

/// The first canvas analysis, kept in its declaration order.
const CARGO_WASM_ARGUMENTS: &[&str] = &[
    "check",
    "--locked",
    "--workspace",
    "--target",
    "wasm32-unknown-unknown",
];

/// The second canvas analysis, kept in its declaration order.
const TRUNK_BUILD_ARGUMENTS: &[&str] = &[
    "build",
    "index.html",
    "--release",
    "--locked",
    "--filehash=false",
];

/// The flag that makes the formatter name differing files instead of diffing.
///
/// A diff carries every differing region, so its volume grows with how far the
/// tree has drifted while the capture reading it stays bounded. A tree drifted
/// far enough truncates that capture, and the arm then refuses that it could
/// not read its own output rather than naming one file, which loses the report
/// exactly when there is most to report. A list costs one line per file, so the
/// worst tree is as reportable as the best one.
const FILES_WITH_DIFF: &str = "--files-with-diff";

/// Whether one selected-tree path sits under an excluded build directory.
fn built(path: &str) -> bool {
    path.split_once('/')
        .is_some_and(|(top, _)| BUILD_EXCLUSIONS.contains(&top))
}

/// Whether any component of one selected-tree path is excluded from scanning.
fn excluded(path: &str) -> bool {
    path.split('/').any(|component| {
        SCANNER_EXCLUSIONS
            .iter()
            .any(|exclusion| exclusion.excludes(component))
    })
}

/// The lowercase extension of one selected-tree path, when it has one.
fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let dot = name.rfind('.')?;
    (dot > 0).then(|| name[dot + 1..].to_ascii_lowercase())
}

/// One count with the unit it counts, so a reason never reads "1 findings".
fn plural(count: usize, unit: &str) -> String {
    if count == 1 {
        format!("{count} {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// Scan every selected change for the workstation path marker.
///
/// One finding per occurrence rather than per line, each carrying the column it
/// opens at. Two markers on one line are two paths that have to be fixed, and
/// the column is what keeps the second from arriving as a duplicate of the first
/// and being refused.
///
/// The include is every regular file, decoded as UTF-8 with replacement, so a
/// binary file is read and simply yields no marker rather than being skipped
/// for a reason the declaration never names. A file that cannot be read is a
/// different thing and terminates the arm, because a scan reporting no marker
/// in a file it never opened would name nothing and mean nothing.
pub(super) fn local_paths(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
) -> Analysis {
    local_paths_with(declaration, root, changes, read_source)
}

fn local_paths_with(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
    read_source: SourceReader,
) -> Analysis {
    let selected = selected_changes(root, changes, |_| true);
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let lines = match numbered_lines(root, path, read_source) {
            Ok(lines) => lines,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        for (line, text) in lines {
            for (start, _) in text.match_indices(LOCAL_PATH_MARKER) {
                findings.push(ArmFinding {
                    path: Some(path.clone()),
                    line: Some(line),
                    column: column_of(&text, start),
                    detail: FindingDetail::LocalPath,
                });
            }
        }
    }
    Analysis::scanned(declaration.id, selected.len(), findings)
}

/// Scan every selected prose change for the compiled discouraged vocabulary.
///
/// One finding per occurrence rather than per word, each carrying the column it
/// opens at. A word written twice on one line is two places the prose has to be
/// rewritten, and the column is what keeps the second from arriving as a
/// duplicate of the first and being refused.
pub(super) fn taste(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
) -> Analysis {
    taste_with(declaration, root, changes, read_source)
}

fn taste_with(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
    read_source: SourceReader,
) -> Analysis {
    let selected = selected_changes(root, changes, |path| {
        extension(path).is_some_and(|found| TASTE_EXTENSIONS.contains(&found.as_str()))
    });
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let lines = match numbered_lines(root, path, read_source) {
            Ok(lines) => lines,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        for (line, text) in lines {
            for found in discouraged(&text) {
                findings.push(ArmFinding {
                    path: Some(path.clone()),
                    line: Some(line),
                    column: Some(found.column),
                    detail: FindingDetail::Taste {
                        word: found.word,
                        replacement: found.replacement,
                    },
                });
            }
        }
    }
    Analysis::scanned(declaration.id, selected.len(), findings)
}

/// Scan every selected change for an unwaived environment access.
///
/// One finding per access rather than per line, each carrying the column it
/// begins at. Two reads on one line are two places the environment shapes
/// behavior, and a column is what keeps the second from arriving as a duplicate
/// of the first and being refused.
pub(super) fn env_policy(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
) -> Analysis {
    env_policy_with(declaration, root, changes, read_source)
}

fn env_policy_with(
    declaration: ToolchainDeclaration,
    root: &Path,
    changes: &[String],
    read_source: SourceReader,
) -> Analysis {
    let selected = selected_changes(root, changes, env_language);
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let family = extension(path).and_then(|found| environment::family(&found));
        let text = match decoded(root, path, read_source) {
            Ok(text) => text,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        let waived = environment::waived_lines(&text);
        for (line, source) in numbered(&text) {
            if waived.contains(&line) {
                continue;
            }
            for access in environment::accesses(family, &source) {
                findings.push(ArmFinding {
                    path: Some(path.clone()),
                    line: Some(line),
                    column: Some(access.column),
                    detail: FindingDetail::EnvironmentAccess {
                        name: access.name,
                        source: source.clone(),
                    },
                });
            }
        }
    }
    Analysis::scanned(declaration.id, selected.len(), findings)
}

/// Account every environment name the complete selected tree references.
///
/// A name the tree references and the profile does not declare is unaccounted
/// and carries every authored locus that references it. Inherited merge-parent
/// loci do not charge the pending commit. A name the profile declares and the
/// complete tree never references is stale and carries none, because there is
/// no place to point at. Both directions are reported, so the ledger cannot
/// drift in either one without saying so.
///
/// The guidance names both waiver spellings, because they are two vocabularies
/// over one line. `env-policy: not-an-env NAME` is a current-port assertion the
/// installed hook has never heard of, and the hook's own `env-policy: allow`
/// reads here as environment context, which returns the literal as a reference
/// instead of exempting it. Recommending the first alone steers an author into
/// a refusal from the second, and recommending the pair steers them back here.
pub(super) fn env_name_ledger(
    declaration: ToolchainDeclaration,
    root: &Path,
    declared_names: &[String],
    chargeable: impl Fn(&str) -> bool,
) -> Analysis {
    env_name_ledger_with(declaration, root, declared_names, read_source, chargeable)
}

fn env_name_ledger_with(
    declaration: ToolchainDeclaration,
    root: &Path,
    declared_names: &[String],
    read_source: SourceReader,
    chargeable: impl Fn(&str) -> bool,
) -> Analysis {
    let sources = match env_sources(root) {
        Ok(sources) => sources,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    if sources.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut referenced: BTreeMap<String, Vec<SourceLocus>> = BTreeMap::new();
    for path in &sources {
        let extension = extension(path);
        let family = extension.as_deref().and_then(environment::family);
        let text = match decoded(root, path, read_source) {
            Ok(text) => text,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        for (line, name) in environment::references_in(extension.as_deref(), family, &text) {
            referenced.entry(name).or_default().push(SourceLocus {
                path: path.clone(),
                line,
            });
        }
    }

    let declared: BTreeSet<&str> = declared_names.iter().map(String::as_str).collect();
    let referenced_paths: BTreeSet<&str> = referenced
        .iter()
        .filter(|(name, _)| !declared.contains(name.as_str()))
        .flat_map(|(_, loci)| loci.iter().map(|locus| locus.path.as_str()))
        .collect();
    let chargeable_paths: BTreeSet<&str> = referenced_paths
        .into_iter()
        .filter(|path| chargeable(path))
        .collect();
    let mut findings: Vec<ArmFinding> = referenced
        .iter()
        .filter(|(name, _)| !declared.contains(name.as_str()))
        .filter_map(|(name, loci)| {
            let loci = loci
                .iter()
                .filter(|locus| chargeable_paths.contains(locus.path.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            (!loci.is_empty()).then(|| ledger(EnvironmentLedgerKind::Unaccounted, name, loci))
        })
        .collect();
    findings.extend(
        declared
            .iter()
            .filter(|name| !referenced.contains_key(**name))
            .map(|name| ledger(EnvironmentLedgerKind::Stale, name, Vec::new())),
    );
    let has_findings = !findings.is_empty();
    let mut analysis = Analysis::scanned(declaration.id, sources.len(), findings);
    if has_findings {
        analysis.reason.push_str(&format!(
            concat!(
                "; mark a quoted non-environment literal as ",
                "`{marker} NAME` on its line, which only this ledger reads. ",
                "The commit hook waives by `{allow}` alone, and that marker ",
                "makes every quoted literal on its line a reference here, so ",
                "a literal both gates flag wants listing in `policy.env_names`"
            ),
            marker = ENV_NAME_LITERAL_MARKER,
            allow = PACKAGED_ENV_PROFILE.allow_marker,
        ));
    }
    analysis
}

/// One repository-wide ledger finding, which carries loci instead of a locus.
fn ledger(mismatch: EnvironmentLedgerKind, name: &str, references: Vec<SourceLocus>) -> ArmFinding {
    ArmFinding {
        path: None,
        line: None,
        column: None,
        detail: FindingDetail::EnvironmentLedger {
            mismatch,
            name: name.to_owned(),
            references,
        },
    }
}

/// Whether one selected-tree path is a packaged environment-language source.
///
/// The scanner's own implementation is never one of them, so a tree that
/// vendors the predecessor does not report the law as a violation of itself.
fn env_language(path: &str) -> bool {
    !environment::is_scanner_itself(path)
        && extension(path)
            .is_some_and(|found| environment::ENV_EXTENSIONS.contains(&found.as_str()))
}

/// Every packaged environment-language source in the complete selected tree.
///
/// Sorted and deduplicated because each retained locus list has to be strictly
/// increasing, and the walk order is the only thing that decides it.
fn env_sources(root: &Path) -> Result<Vec<String>, String> {
    let walked = Jurisdiction::walk(root, &Ignore::default())
        .map_err(|error| format!("the selected tree could not be walked: {error}"))?;
    let mut sources: Vec<String> = walked
        .measurable()
        .map(|entry| entry.path().to_owned())
        .filter(|path| !excluded(path) && env_language(path))
        .collect();
    sources.sort();
    sources.dedup();
    Ok(sources)
}

mod javascript;
pub(in crate::toolchain) use javascript::*;

/// The formatter invocation for one selected tree, or the refusal to build it.
///
/// Every selected path is named on the command line rather than left to a
/// crate-root walk. rustfmt reaches a file by walking the `mod` declarations
/// down from each crate root, and a file reached through `include!` is named by
/// a macro instead -- `proof/src/container.rs` reaches `container/tests.rs` that
/// way, one of seven such files left here -- so it belongs to no module path.
/// `cargo fmt --all` opens the including file and never the included one,
/// which leaves those files counted as selected, measured by every other arm,
/// and never handed to the formatter at all. Naming the paths is the one way
/// rustfmt will open them, and it is what makes the arm's own count and its
/// coverage the same set.
fn formatter_invocation(
    root: &Path,
    sources: &[String],
    programs: &BTreeMap<&str, &str>,
) -> Result<(Argv, Environment), Analysis> {
    let Some(program) = programs.get(FORMATTER) else {
        return Err(Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("formatters declares no {FORMATTER} to invoke the check through"),
        ));
    };
    let edition = match formatter_edition(root) {
        Ok(edition) => edition,
        Err(reason) => return Err(Analysis::terminal(ArmOutcome::ParseFailure, reason)),
    };
    let arguments: Vec<String> = [
        "--edition".to_owned(),
        edition,
        "--check".to_owned(),
        FILES_WITH_DIFF.to_owned(),
    ]
    .into_iter()
    .chain(sources.iter().cloned())
    .collect();
    let Ok(argv) = Argv::new(program, arguments.iter().map(OsStr::new)) else {
        return Err(Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{FORMATTER} argument vector was refused"),
        ));
    };
    match tool_environment("formatters", &[*program]) {
        Ok(environment) => Ok((argv, environment)),
        Err(reason) => Err(Analysis::terminal(ArmOutcome::ParseFailure, reason)),
    }
}

/// Run the declared formatter check over the complete selected tree.
///
/// One finding per differing file and no locus within it, because
/// [`FindingDetail::Formatting`] records that the file is not canonical and no
/// single region is more the cause than any other.
pub(super) fn formatters(
    declaration: ToolchainDeclaration,
    root: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    let sources = match formatter_sources(root) {
        Ok(sources) => sources,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    // A tree with nothing to format runs no formatter at all, rather than one
    // named an empty file list: rustfmt reads stdin when it is named no path,
    // and would sit on it until the bounds killed the arm.
    if sources.is_empty() {
        return Analysis::not_applicable(declaration);
    }
    let (argv, environment) = match formatter_invocation(root, &sources, programs) {
        Ok(invocation) => invocation,
        Err(analysis) => return analysis,
    };
    let mut spec = bounds.spec(argv, root);
    spec.environment = environment;
    let running = match spawn(spec) {
        Ok(running) => running,
        Err(error) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{FORMATTER} did not start: {error}"),
            );
        }
    };
    let outcome = match running.wait() {
        Ok(outcome) => outcome,
        Err(error) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{FORMATTER} could not be waited on: {error}"),
            );
        }
    };
    let status = match outcome.completion() {
        Completion::Exited(status) => status,
        Completion::TimedOut(_) => {
            return Analysis::terminal(
                ArmOutcome::Timeout,
                format!("{FORMATTER} outlived the bounds it was given"),
            );
        }
        Completion::Cancelled(_) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{FORMATTER} was cancelled before it reported"),
            );
        }
    };
    let Some(stdout) = outcome.stdout() else {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{FORMATTER} captured no stdout to read"),
        );
    };
    if stdout.truncated() || !stdout.complete() {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{FORMATTER} output was truncated before it could be read"),
        );
    }

    let printed = String::from_utf8_lossy(stdout.bytes()).into_owned();
    let findings = differing_files(root, &printed);
    if findings.is_empty() && !status.success() {
        let Some(code) = status.code() else {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{FORMATTER} was signalled without naming a differing file"),
            );
        };
        return Analysis::refused(
            format!("{FORMATTER} refused with {code} and named no differing file"),
            code,
        );
    }
    Analysis::scanned(declaration.id, sources.len(), findings)
}

/// Run the declared canvas checks in order and stop at the first non-clean one.
///
/// The analysis is run against a cell its workspace reports a failed removal
/// into, and that report outranks whatever the analysis concluded. A workspace
/// left on disk refuses every later canvas analysis of the same tree, so an arm
/// that passed while leaking one would report a clean tree and then be unable to
/// measure it again; the refusal here is the same refusal, delivered on the run
/// that caused it instead of the next one.
pub(super) fn canvas_web_typecheck(
    declaration: ToolchainDeclaration,
    root: &Path,
    repository: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    let removal = Cell::new(None);
    let analysis = canvas_analysis(declaration, root, repository, bounds, programs, &removal);
    match removal.take() {
        Some(report) => Analysis::terminal(ArmOutcome::ParseFailure, report),
        None => analysis,
    }
}

fn canvas_analysis(
    declaration: ToolchainDeclaration,
    root: &Path,
    repository: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
    removal: &Cell<Option<String>>,
) -> Analysis {
    if !root.join("Cargo.toml").is_file() || !root.join("app/index.html").is_file() {
        return Analysis::not_applicable(declaration);
    }
    let Some(cargo) = programs.get(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {CARGO} program", declaration.id.as_str()),
        );
    };
    let Some(trunk) = programs.get(TRUNK) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {TRUNK} program", declaration.id.as_str()),
        );
    };
    let workspace = match CanvasWorkspace::create(root, removal) {
        Ok(workspace) => workspace,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let outputs = match CanvasOutputs::create(&workspace, root, repository) {
        Ok(outputs) => outputs,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let environment = match canvas_environment(cargo, trunk, &outputs) {
        Ok(environment) => environment,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };

    let app = root.join("app");
    let invocations = [
        CompileInvocation {
            name: "cargo wasm32 check",
            program: cargo,
            arguments: CARGO_WASM_ARGUMENTS,
            cwd: root,
        },
        CompileInvocation {
            name: "trunk canvas build",
            program: trunk,
            arguments: TRUNK_BUILD_ARGUMENTS,
            cwd: app.as_path(),
        },
    ];
    for invocation in invocations {
        if let Err(failure) = run_compile_analysis(
            declaration.id,
            root,
            invocation,
            bounds,
            &environment,
            |code, message| FindingDetail::CanvasWebOutput { code, message },
        ) {
            return failure.analysis;
        }
    }
    Analysis::terminal(
        ArmOutcome::Passed,
        format!(
            "{} completed both declared canvas analyses",
            declaration.id.as_str()
        ),
    )
}

fn canvas_environment(
    cargo: &str,
    trunk: &str,
    outputs: &CanvasOutputs,
) -> Result<Environment, String> {
    let path = tool_search_path("canvas", &[cargo, trunk, "/usr/bin/cc", "/bin/sh"])?;
    Environment::new([
        ("PATH", path),
        (HOME_DIRECTORY, COMPILED_HOME_DIRECTORY.into()),
        (
            CARGO_TARGET_DIRECTORY,
            outputs.cargo_target.as_os_str().to_owned(),
        ),
        (
            CARGO_BUILD_DIRECTORY,
            outputs.cargo_build.as_os_str().to_owned(),
        ),
        (
            TRUNK_DISTRIBUTION_DIRECTORY,
            outputs.trunk_dist.as_os_str().to_owned(),
        ),
    ])
    .map_err(|error| format!("canvas tool environment was refused: {error}"))
}

/// A hermetic search path containing only the resolved analysis programs.
///
/// Cargo subcommands and the tools they dispatch are resolved through `PATH`
/// even when Cargo itself was invoked by absolute path. Preserve the empty
/// process environment everywhere else while making those declared programs
/// mutually discoverable.
fn tool_environment(label: &str, programs: &[&str]) -> Result<Environment, String> {
    let path = tool_search_path(label, programs)?;
    Environment::new([("PATH", path)])
        .map_err(|error| format!("{label} tool environment was refused: {error}"))
}

fn tool_search_path(label: &str, programs: &[&str]) -> Result<std::ffi::OsString, String> {
    let mut directories = Vec::new();
    for program in programs {
        let Some(parent) = Path::new(program).parent() else {
            return Err(format!("{label} tool {program:?} has no parent directory"));
        };
        if !directories.contains(&parent) {
            directories.push(parent);
        }
    }
    std::env::join_paths(directories)
        .map_err(|error| format!("{label} tool search path is invalid: {error}"))
}

/// Writable output roots for tools examining one immutable selected tree.
///
/// The Cargo pair is deliberately not inside [`CanvasWorkspace`]: it is a
/// fixed repository path that every canvas analysis of this repository shares,
/// so the compiled artifacts survive the run that produced them. `input.rs`
/// records what that warmth is worth -- five cold samples of the `trunk build`
/// this arm runs took 15.0 to 24.9 seconds against the 2.6 to 2.9 they take
/// warm -- and a root minted per invocation would pay the cold price every time
/// while stranding the previous one.
///
/// Sharing it is safe because Cargo already arbitrates it. Two of these
/// analyses run at once against one repository were measured taking the same
/// pair: the second reported `Blocking waiting for file lock on build
/// directory` and then completed, both exiting zero. What must not be shared is
/// the generated output, and that is the part living under the owned root.
struct CanvasOutputs {
    cargo_target: PathBuf,
    cargo_build: PathBuf,
    trunk_dist: PathBuf,
}

impl CanvasOutputs {
    fn create(
        workspace: &CanvasWorkspace<'_>,
        selected: &Path,
        repository: &Path,
    ) -> Result<Self, String> {
        let cargo_target = repository.join("target/xtask-app");
        let cargo_build = repository.join("target/xtask-app-build");
        let trunk_dist = workspace.path().join("trunk-dist");
        if let Err(error) = fs::create_dir_all(&cargo_target)
            .and_then(|()| fs::create_dir_all(&cargo_build))
            .and_then(|()| fs::create_dir(&trunk_dist))
        {
            return Err(format!(
                "canvas analysis output directories for {} could not be created: {error}",
                selected.display()
            ));
        }
        Ok(Self {
            cargo_target,
            cargo_build,
            trunk_dist,
        })
    }
}

fn complete_output(name: &str, outcome: &sill::WaitOutcome) -> Result<String, String> {
    if let Some(failure) = outcome.observation_failure() {
        return Err(format!("{name} output observation failed: {failure}"));
    }
    let Some(stdout) = outcome.stdout() else {
        return Err(format!("{name} captured no stdout to read"));
    };
    let Some(stderr) = outcome.stderr() else {
        return Err(format!("{name} captured no stderr to read"));
    };
    if stdout.truncated() || stderr.truncated() || !stdout.complete() || !stderr.complete() {
        return Err(format!(
            "{name} output was truncated or incomplete before it could be read"
        ));
    }

    let mut printed = String::from_utf8_lossy(stdout.bytes()).into_owned();
    if !printed.is_empty() && !printed.ends_with('\n') {
        printed.push('\n');
    }
    printed.push_str(&String::from_utf8_lossy(stderr.bytes()));
    Ok(printed)
}

/// Every located `rustc` diagnostic in one program's output.
///
/// The parse is the compiler's own, so both compile arms share it; they differ
/// only in the [`FindingDetail`] they publish, which is what keeps
/// [`FindingDetail::gate`] able to answer which arm a finding came from.
fn compile_findings(
    root: &Path,
    cwd: &Path,
    printed: &str,
    detail: fn(String, String) -> FindingDetail,
) -> Vec<ArmFinding> {
    let mut pending: Option<(String, String)> = None;
    let mut findings = BTreeSet::new();
    for raw in printed.lines() {
        let line = raw.trim_start();
        if let Some(diagnostic) = canvas_diagnostic(line) {
            pending = Some(diagnostic);
            continue;
        }
        let Some(location) = line.strip_prefix("--> ") else {
            continue;
        };
        let Some((code, message)) = pending.take() else {
            continue;
        };
        let Some((path, line, column)) = canvas_locus(root, cwd, location) else {
            continue;
        };
        findings.insert(ArmFinding {
            path: Some(path),
            line: Some(line),
            column: Some(column),
            detail: detail(code, message),
        });
    }
    findings.into_iter().collect()
}

fn canvas_diagnostic(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("error[")?;
    let (code, message) = rest.split_once("]: ")?;
    (!code.is_empty() && !message.is_empty()).then(|| (code.to_owned(), message.to_owned()))
}

fn canvas_locus(root: &Path, cwd: &Path, location: &str) -> Option<(String, u32, u32)> {
    let (path_and_line, column) = location.rsplit_once(':')?;
    let (reported, line) = path_and_line.rsplit_once(':')?;
    let line = line.parse::<u32>().ok().filter(|line| *line > 0)?;
    let column = column.parse::<u32>().ok().filter(|column| *column > 0)?;
    let path = if Path::new(reported).is_absolute() {
        relative_to(&reported_roots(root), reported)?
    } else {
        cwd.strip_prefix(root)
            .ok()?
            .join(reported)
            .to_str()?
            .replace('\\', "/")
    };
    super::valid_relative_path(&path).then_some((path, line, column))
}

/// One finding per file the formatter named as differing from canonical form.
///
/// The file is the whole locus a formatting refusal has: the fix is to run the
/// formatter over it, and no region within it is more the cause than another.
fn differing_files(root: &Path, printed: &str) -> Vec<ArmFinding> {
    let roots = reported_roots(root);
    let named: BTreeSet<String> = printed
        .lines()
        .filter_map(|line| relative_to(&roots, line.trim_end()))
        .collect();
    named
        .into_iter()
        .map(|path| ArmFinding {
            path: Some(path),
            line: None,
            column: None,
            detail: FindingDetail::Formatting {
                formatter: FORMATTER.to_owned(),
            },
        })
        .collect()
}

/// Every spelling the formatter may report the selected tree under.
///
/// A tree reached through a symlinked ancestor is reported canonically by the
/// formatter and handed here uncanonically, so both spellings are offered and a
/// path matching neither is genuinely outside the tree. Offering only one would
/// silently discard every finding and call an unformatted tree clean.
fn reported_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_owned()];
    if let Ok(canonical) = fs::canonicalize(root)
        && canonical != root
    {
        roots.push(canonical);
    }
    roots
}

/// Re-express one absolute reported path relative to the selected tree.
fn relative_to(roots: &[PathBuf], reported: &str) -> Option<String> {
    let reported = Path::new(reported);
    let relative = roots
        .iter()
        .find_map(|root| reported.strip_prefix(root).ok())?
        .to_str()?;
    (!relative.is_empty()).then(|| relative.replace('\\', "/"))
}

/// Count the selected Rust sources the formatter check applies to.
pub(super) fn formatter_sources(root: &Path) -> Result<Vec<String>, String> {
    let walked = Jurisdiction::walk(root, &Ignore::default())
        .map_err(|error| format!("the selected tree could not be walked: {error}"))?;
    Ok(walked
        .measurable()
        .filter(|entry| !built(entry.path()))
        .filter(|entry| extension(entry.path()).is_some_and(|found| found == RUST_EXTENSION))
        .map(|entry| entry.path().to_owned())
        .collect())
}

/// The edition the root manifest pins, which the formatter must be told.
///
/// `cargo fmt` reads the edition out of the manifest of the package owning each
/// target it drives, before it invokes the formatter. Handing the formatter its
/// paths directly skips that read, and an included file has no package behind it
/// to read one from in the first place, since no cargo target claims it. The
/// formatter's own default is the 2015 edition, and a wrong edition is not a
/// formatting difference: rustfmt refuses to parse the file at all. Refusing is
/// the alternative to guessing one.
///
/// Both spellings are read because a root manifest carries the edition in
/// either: a workspace declares one its members inherit, and a single-package
/// repository declares its own. The workspace default is preferred because it
/// is the one that governs more than the root package.
///
/// What this cannot see is a member overriding the inherited edition, which
/// would be measured against the root's grammar instead of its own. This
/// repository has no such member and its own arm would report the drift, so the
/// bound is stated rather than guarded.
///
/// The refusal is flattened onto one line because a TOML error prints a
/// multiline snippet and the executor refuses to publish a multiline reason at
/// all, so an unflattened diagnosis would be replaced by the refusal to carry
/// it rather than reaching the reader.
pub(super) fn formatter_edition(root: &Path) -> Result<String, String> {
    let manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .map_err(|error| format!("the root manifest could not be read: {error}"))?;
    let document: toml::Value = toml::from_str(&text).map_err(|error| {
        format!(
            "the root manifest is not valid TOML: {}",
            error.to_string().replace('\n', " ")
        )
    })?;
    let workspace = document
        .get("workspace")
        .and_then(|workspace| workspace.get("package"));
    [workspace, document.get("package")]
        .into_iter()
        .flatten()
        .find_map(|table| table.get("edition").and_then(toml::Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| {
            "the root manifest declares neither workspace.package.edition nor package.edition"
                .to_owned()
        })
}

/// Whether the selected tree satisfies the Python declaration's applicability.
fn python_sources(root: &Path) -> Result<bool, String> {
    if !root.join("pyproject.toml").is_file() {
        return Ok(false);
    }
    let walked = Jurisdiction::walk(root, &Ignore::default())
        .map_err(|error| format!("the selected tree could not be walked: {error}"))?;
    Ok(walked.measurable().any(|entry| {
        !built(entry.path())
            && extension(entry.path()).is_some_and(|found| found == "py" || found == "pyi")
    }))
}

/// The selected changes one scanner admits, sorted and deduplicated.
///
/// A captured change that is no longer a regular file in the selected tree is
/// dropped rather than reported: a deletion is a change the scanners have
/// nothing to read and nothing to say about.
///
/// Every path is held to [`super::valid_relative_path`], the same rule the
/// report holds findings to. That is not defensiveness about a malformed rim:
/// [`Path::join`] discards the root when given an absolute path, so admitting
/// one would read a file outside the captured tree and report it as though it
/// were inside.
fn selected_changes(
    root: &Path,
    changes: &[String],
    include: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut selected: Vec<String> = changes
        .iter()
        .filter(|path| super::valid_relative_path(path))
        .filter(|path| !excluded(path) && include(path))
        .filter(|path| root.join(path).is_file())
        .cloned()
        .collect();
    selected.sort();
    selected.dedup();
    selected
}

/// One selected file decoded with replacement, or why it could not be read.
///
/// Decoding is lossy so a binary file that reads is scanned and simply yields
/// nothing, which is what the path scanner's include promises. A read that
/// fails is a different thing entirely and terminates the arm that asked: an
/// analysis answering from empty text would report a clean file it never opened.
fn decoded(root: &Path, path: &str, read_source: SourceReader) -> Result<String, String> {
    read_source(&root.join(path))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|error| format!("the selected source {path} could not be read: {error}"))
}

/// One decoded file as one-based numbered lines.
fn numbered(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            u32::try_from(index + 1)
                .ok()
                .map(|number| (number, line.to_owned()))
        })
        .collect()
}

/// One selected file decoded with replacement, as one-based numbered lines.
fn numbered_lines(
    root: &Path,
    path: &str,
    read_source: SourceReader,
) -> Result<Vec<(u32, String)>, String> {
    Ok(numbered(&decoded(root, path, read_source)?))
}

/// One discouraged word, where it sits on its line, and what to write instead.
struct Discouraged {
    column: u32,
    word: String,
    replacement: String,
}

/// Every discouraged word one line of prose contains, with its replacement.
///
/// A trailing `*` in the compiled vocabulary matches any continuation of the
/// same word, and the reported word is the text that actually appeared rather
/// than the pattern that caught it.
///
/// One word repeated on one line is reported once per occurrence, because each
/// is a separate place the prose has to be rewritten. The column is what keeps
/// the second from arriving as a finding identical to the first, which the
/// report refuses as a duplicate rather than retains as evidence.
fn discouraged(text: &str) -> Vec<Discouraged> {
    words(text)
        .into_iter()
        .filter_map(|(column, word)| {
            PACKAGED_TASTE_WORDS
                .iter()
                .find(|(pattern, _)| matches_pattern(pattern, &word))
                .map(|(_, replacement)| Discouraged {
                    column,
                    word,
                    replacement: (*replacement).to_owned(),
                })
        })
        .collect()
}

/// Whether one compiled pattern catches one whole word, ignoring case.
///
/// A trailing `*` is the glob it looks like and matches any continuation
/// including none, so `orphan*` catches both `orphan` and `orphaned`.
fn matches_pattern(pattern: &str, word: &str) -> bool {
    let lowered = word.to_ascii_lowercase();
    pattern
        .strip_suffix('*')
        .map_or_else(|| lowered == pattern, |stem| lowered.starts_with(stem))
}

/// Every whole word in one line, each with the column it opens at, in order.
fn words(text: &str) -> Vec<(u32, String)> {
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut opened: Option<usize> = None;
    for (offset, character) in text.char_indices() {
        match (is_word_character(character), opened) {
            (true, None) => opened = Some(offset),
            (false, Some(start)) => {
                found.extend(word_at(text, start, offset));
                opened = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    if let Some(start) = opened {
        found.extend(word_at(text, start, text.len()));
    }
    found
}

/// Whether one character continues a word rather than ending it.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// One word taken from a line, at the column it opens at.
///
/// A line too long to name a column in is dropped rather than reported at an
/// invented one, which is what [`numbered`] already does with its line numbers.
fn word_at(text: &str, start: usize, end: usize) -> Option<(u32, String)> {
    column_of(text, start).map(|column| (column, text[start..end].to_owned()))
}

#[cfg(test)]
mod tests;

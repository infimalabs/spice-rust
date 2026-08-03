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

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, Environment, spawn};

use super::{
    ArmFinding, ArmOutcome, EnvironmentLedgerKind, FindingDetail, JavascriptUnusedStatus,
    LOCAL_PATH_MARKER, PACKAGED_ENV_PROFILE, PACKAGED_JAVASCRIPT_EXEMPTIONS, PACKAGED_TASTE_WORDS,
    PACKAGED_TEST_PATHS, SourceLocus, ToolchainDeclaration, column_of, environment,
};
use crate::GateId;
use crate::jurisdiction::{Ignore, Jurisdiction};
use crate::measure::Bounds;
use crate::structural::{StructuralPattern, scan_javascript_symbols};

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
}

impl Analysis {
    /// A terminal state that carries neither findings nor an exit code.
    pub(super) fn terminal(outcome: ArmOutcome, reason: String) -> Self {
        Self {
            outcome,
            reason,
            findings: Vec::new(),
            exit_code: None,
        }
    }

    /// A direct analysis process refused without naming anything it examined.
    fn refused(reason: String, code: i32) -> Self {
        Self {
            outcome: ArmOutcome::NonzeroAnalysis,
            reason,
            findings: Vec::new(),
            exit_code: Some(code),
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
        }
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

/// The compiled form of the Taste include globs, in declaration order.
const PROSE_EXTENSIONS: &[&str] = &["md", "txt", "rst"];

/// The compiled form of the JavaScript-unused include globs.
const JAVASCRIPT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs"];

/// The compiled form of the Formatters Rust include glob.
const RUST_EXTENSION: &str = "rs";

/// The formatter whose canonical bytes every Formatters finding is measured against.
const FORMATTER: &str = "rustfmt";

/// The declared program the formatter check is invoked through.
const CARGO: &str = "cargo";

/// The declared program that validates the canvas asset graph.
const TRUNK: &str = "trunk";

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

#[derive(Clone, Copy)]
struct CanvasInvocation<'a> {
    name: &'static str,
    program: &'a str,
    arguments: &'static [&'static str],
    cwd: &'a Path,
}

/// The line prefix `cargo fmt --check` writes before each differing region.
const DIFF_PREFIX: &str = "Diff in ";

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
    let selected = selected_changes(root, changes, |_| true);
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let lines = match numbered_lines(root, path) {
            Ok(lines) => lines,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        for (line, text) in lines {
            if text.contains(LOCAL_PATH_MARKER) {
                findings.push(ArmFinding {
                    path: Some(path.clone()),
                    line: Some(line),
                    column: None,
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
    let selected = selected_changes(root, changes, |path| {
        extension(path).is_some_and(|found| PROSE_EXTENSIONS.contains(&found.as_str()))
    });
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let lines = match numbered_lines(root, path) {
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
    let selected = selected_changes(root, changes, env_language);
    if selected.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for path in &selected {
        let family = extension(path).and_then(|found| environment::family(&found));
        let text = match decoded(root, path) {
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
/// and carries every locus that references it. A name the profile declares and
/// the tree never references is stale and carries none, because there is no
/// place to point at. Both directions are reported, so the ledger cannot drift
/// in either one without saying so.
pub(super) fn env_name_ledger(declaration: ToolchainDeclaration, root: &Path) -> Analysis {
    let sources = match env_sources(root) {
        Ok(sources) => sources,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    if sources.is_empty() {
        return Analysis::not_applicable(declaration);
    }

    let mut referenced: BTreeMap<String, Vec<SourceLocus>> = BTreeMap::new();
    for path in &sources {
        let family = extension(path).and_then(|found| environment::family(&found));
        let lines = match numbered_lines(root, path) {
            Ok(lines) => lines,
            Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
        };
        for (line, text) in lines {
            for name in environment::references(family, &text) {
                referenced.entry(name).or_default().push(SourceLocus {
                    path: path.clone(),
                    line,
                });
            }
        }
    }

    let declared: BTreeSet<&str> = PACKAGED_ENV_PROFILE
        .declared_names
        .iter()
        .copied()
        .collect();
    let mut findings: Vec<ArmFinding> = referenced
        .iter()
        .filter(|(name, _)| !declared.contains(name.as_str()))
        .map(|(name, loci)| ledger(EnvironmentLedgerKind::Unaccounted, name, loci.clone()))
        .collect();
    findings.extend(
        declared
            .iter()
            .filter(|name| !referenced.contains_key(**name))
            .map(|name| ledger(EnvironmentLedgerKind::Stale, name, Vec::new())),
    );
    Analysis::scanned(declaration.id, sources.len(), findings)
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

#[derive(Debug)]
struct JavascriptDeclaration {
    path: String,
    line: u32,
    declaration_kind: String,
    symbol: String,
    in_test: bool,
}

#[derive(Debug)]
struct JavascriptInventory {
    examined: usize,
    declarations: Vec<JavascriptDeclaration>,
    references: BTreeMap<String, (usize, usize)>,
}

/// Resolve JavaScript declarations against every identifier in the selected modules.
///
/// The production count includes a declaration's own identifier. Consequently
/// `(1, 0)` means a production declaration is truly unused, while `(1, n)`
/// means its only uses beyond the declaration occur in configured test roots.
pub(super) fn javascript_unused(declaration: ToolchainDeclaration, root: &Path) -> Analysis {
    let inventory = match javascript_inventory(root) {
        Ok(inventory) => inventory,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    if inventory.examined == 0 {
        return Analysis::not_applicable(declaration);
    }

    let mut findings = Vec::new();
    for declared in inventory.declarations {
        if javascript_exempt(&declared.path, &declared.symbol) {
            continue;
        }
        let (production, tests) = inventory
            .references
            .get(&declared.symbol)
            .copied()
            .unwrap_or_default();
        let (production, tests) = if declared.in_test {
            (production, tests + 1)
        } else {
            (production + 1, tests)
        };
        let status = if production + tests == 1 {
            Some((
                JavascriptUnusedStatus::CandidateUnused,
                "no_references_outside_declaration",
            ))
        } else if !declared.in_test && production == 1 && tests > 0 {
            Some((JavascriptUnusedStatus::TestOnly, "references_only_in_tests"))
        } else {
            None
        };
        let Some((status, reason)) = status else {
            continue;
        };
        let Ok(reference_count) = u32::try_from(production) else {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!(
                    "javascript-unused reference count overflowed for {}",
                    declared.symbol
                ),
            );
        };
        let Ok(test_reference_count) = u32::try_from(tests) else {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!(
                    "javascript-unused test reference count overflowed for {}",
                    declared.symbol
                ),
            );
        };
        findings.push(ArmFinding {
            path: Some(declared.path),
            line: Some(declared.line),
            column: None,
            detail: FindingDetail::JavascriptUnused {
                symbol: declared.symbol,
                declaration_kind: declared.declaration_kind,
                status,
                reason: reason.to_owned(),
                reference_count,
                test_reference_count,
            },
        });
    }
    Analysis::scanned(declaration.id, inventory.examined, findings)
}

fn javascript_inventory(root: &Path) -> Result<JavascriptInventory, String> {
    let sources = javascript_sources(root)?;
    let mut declarations = Vec::new();
    let mut references: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (path, source, in_test) in &sources {
        let matches = scan_javascript_symbols(source)
            .map_err(|error| format!("javascript-unused could not parse {path}: {error}"))?;
        let declaration_loci: BTreeSet<(usize, usize)> = matches
            .iter()
            .filter(|matched| matched.pattern() == StructuralPattern::JavaScriptDeclaration)
            .flat_map(|matched| matched.captures())
            .map(|capture| (capture.start_byte(), capture.end_byte()))
            .collect();
        for matched in matches {
            let [capture] = matched.captures() else {
                return Err(format!(
                    "javascript-unused received a malformed {} match in {path}",
                    matched.pattern().as_str()
                ));
            };
            match matched.pattern() {
                StructuralPattern::JavaScriptDeclaration => {
                    let line = source_line(source, matched.start_byte())
                        .ok_or_else(|| format!("javascript-unused line overflowed in {path}"))?;
                    declarations.push(JavascriptDeclaration {
                        path: path.clone(),
                        line,
                        declaration_kind: capture.name().to_owned(),
                        symbol: capture.text().to_owned(),
                        in_test: *in_test,
                    });
                }
                StructuralPattern::JavaScriptIdentifier => {
                    if declaration_loci.contains(&(capture.start_byte(), capture.end_byte())) {
                        continue;
                    }
                    let counts = references.entry(capture.text().to_owned()).or_default();
                    if *in_test {
                        counts.1 += 1;
                    } else {
                        counts.0 += 1;
                    }
                }
                StructuralPattern::FunctionCall => {
                    return Err(format!(
                        "javascript-unused received an unrelated structural match in {path}"
                    ));
                }
            }
        }
    }
    Ok(JavascriptInventory {
        examined: sources.len(),
        declarations,
        references,
    })
}

fn javascript_sources(root: &Path) -> Result<Vec<(String, String, bool)>, String> {
    let walked = Jurisdiction::walk(root, &Ignore::default())
        .map_err(|error| format!("the selected tree could not be walked: {error}"))?;
    let mut paths: Vec<String> = walked
        .measurable()
        .map(|entry| entry.path().to_owned())
        .filter(|path| !excluded(path))
        .filter(|path| {
            extension(path).is_some_and(|found| JAVASCRIPT_EXTENSIONS.contains(&found.as_str()))
        })
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(root.join(&path)).map_err(|error| {
                format!("javascript-unused could not read {path} as UTF-8: {error}")
            })?;
            let in_test = PACKAGED_TEST_PATHS.iter().any(|test_root| {
                path == *test_root
                    || path
                        .strip_prefix(test_root)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            });
            Ok((path, source, in_test))
        })
        .collect()
}

fn javascript_exempt(path: &str, symbol: &str) -> bool {
    PACKAGED_JAVASCRIPT_EXEMPTIONS
        .iter()
        .any(|(exempt_path, exempt_symbol, _)| path == *exempt_path && symbol == *exempt_symbol)
}

fn source_line(source: &str, start_byte: usize) -> Option<u32> {
    let prefix = source.as_bytes().get(..start_byte)?;
    u32::try_from(prefix.iter().filter(|byte| **byte == b'\n').count() + 1).ok()
}

/// Run the declared formatter check over the complete selected tree.
///
/// One finding per differing file, carrying the first region the formatter
/// reported, because [`FindingDetail::Formatting`] records that the file is not
/// canonical and no single region is more the cause than any other.
pub(super) fn formatters(
    declaration: ToolchainDeclaration,
    root: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    let sources = match rust_sources(root) {
        Ok(sources) => sources,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    if sources == 0 {
        return Analysis::not_applicable(declaration);
    }
    let Some(program) = programs.get(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("formatters declares no {CARGO} to invoke the check through"),
        );
    };

    let arguments = ["fmt", "--all", "--", "--check"];
    let Ok(argv) = Argv::new(program, arguments.iter().map(OsStr::new)) else {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{CARGO} argument vector was refused"),
        );
    };
    let running = match spawn(bounds.spec(argv, root)) {
        Ok(running) => running,
        Err(error) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{CARGO} fmt did not start: {error}"),
            );
        }
    };
    let outcome = match running.wait() {
        Ok(outcome) => outcome,
        Err(error) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{CARGO} fmt could not be waited on: {error}"),
            );
        }
    };
    let status = match outcome.completion() {
        Completion::Exited(status) => status,
        Completion::TimedOut(_) => {
            return Analysis::terminal(
                ArmOutcome::Timeout,
                format!("{CARGO} fmt outlived the bounds it was given"),
            );
        }
        Completion::Cancelled(_) => {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{CARGO} fmt was cancelled before it reported"),
            );
        }
    };
    let Some(stdout) = outcome.stdout() else {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{CARGO} fmt captured no stdout to read"),
        );
    };
    if stdout.truncated() || !stdout.complete() {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{CARGO} fmt output was truncated before it could be read"),
        );
    }

    let printed = String::from_utf8_lossy(stdout.bytes()).into_owned();
    let findings = differing_files(root, &printed);
    if findings.is_empty() && !status.success() {
        let Some(code) = status.code() else {
            return Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{CARGO} fmt was signalled without naming a differing file"),
            );
        };
        return Analysis::refused(
            format!("{CARGO} fmt refused with {code} and named no differing file"),
            code,
        );
    }
    Analysis::scanned(declaration.id, sources, findings)
}

/// Run the declared canvas checks in order and stop at the first non-clean one.
pub(super) fn canvas_web_typecheck(
    declaration: ToolchainDeclaration,
    root: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
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
    let environment = match canvas_environment(cargo, trunk) {
        Ok(environment) => environment,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };

    let app = root.join("app");
    let invocations = [
        CanvasInvocation {
            name: "cargo wasm32 check",
            program: cargo,
            arguments: CARGO_WASM_ARGUMENTS,
            cwd: root,
        },
        CanvasInvocation {
            name: "trunk canvas build",
            program: trunk,
            arguments: TRUNK_BUILD_ARGUMENTS,
            cwd: app.as_path(),
        },
    ];
    for invocation in invocations {
        if let Err(analysis) =
            run_canvas_analysis(declaration.id, root, invocation, bounds, &environment)
        {
            return analysis;
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

fn run_canvas_analysis(
    id: GateId,
    root: &Path,
    invocation: CanvasInvocation<'_>,
    bounds: &Bounds,
    environment: &Environment,
) -> Result<(), Analysis> {
    let CanvasInvocation {
        name,
        program,
        arguments,
        cwd,
    } = invocation;
    let argv = Argv::new(program, arguments.iter().map(OsStr::new)).map_err(|error| {
        Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} argument vector was refused: {error}"),
        )
    })?;
    let mut spec = bounds.spec(argv, cwd);
    spec.environment = environment.clone();
    let running = spawn(spec).map_err(|error| {
        Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} did not start: {error}"),
        )
    })?;
    let outcome = running.wait().map_err(|error| {
        Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} could not be waited on: {error}"),
        )
    })?;
    let status = match outcome.completion() {
        Completion::Exited(status) => status,
        Completion::TimedOut(_) => {
            return Err(Analysis::terminal(
                ArmOutcome::Timeout,
                format!("{name} outlived the bounds it was given"),
            ));
        }
        Completion::Cancelled(_) => {
            return Err(Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{name} was cancelled before it reported"),
            ));
        }
    };
    let printed = complete_output(name, &outcome)
        .map_err(|reason| Analysis::terminal(ArmOutcome::ParseFailure, reason))?;
    let findings = canvas_findings(root, cwd, &printed);
    if !findings.is_empty() {
        return Err(Analysis {
            outcome: ArmOutcome::Findings,
            reason: format!(
                "{} parsed {} from {name}",
                id.as_str(),
                plural(findings.len(), "diagnostic")
            ),
            findings,
            exit_code: None,
        });
    }
    if status.success() {
        return Ok(());
    }
    let Some(code) = status.code() else {
        return Err(Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} was signalled without a typed diagnostic"),
        ));
    };
    Err(Analysis::refused(
        format!("{name} refused with {code} and emitted no typed diagnostic"),
        code,
    ))
}

fn canvas_environment(cargo: &str, trunk: &str) -> Result<Environment, String> {
    let mut directories = Vec::new();
    for program in [cargo, trunk] {
        let Some(parent) = Path::new(program).parent() else {
            return Err(format!("canvas tool {program:?} has no parent directory"));
        };
        if !directories.contains(&parent) {
            directories.push(parent);
        }
    }
    let path = std::env::join_paths(directories)
        .map_err(|error| format!("canvas tool search path is invalid: {error}"))?;
    Environment::new([("PATH", path)])
        .map_err(|error| format!("canvas tool environment was refused: {error}"))
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

fn canvas_findings(root: &Path, cwd: &Path, printed: &str) -> Vec<ArmFinding> {
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
            detail: FindingDetail::CanvasWebOutput { code, message },
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

/// One finding per file `cargo fmt --check` reported a differing region in.
fn differing_files(root: &Path, printed: &str) -> Vec<ArmFinding> {
    let roots = reported_roots(root);
    let mut first: BTreeMap<String, u32> = BTreeMap::new();
    for line in printed.lines() {
        let Some(rest) = line.strip_prefix(DIFF_PREFIX) else {
            continue;
        };
        let located = rest.trim_end_matches(':');
        let Some((path, number)) = located.rsplit_once(':') else {
            continue;
        };
        let Ok(number) = number.parse::<u32>() else {
            continue;
        };
        let Some(relative) = relative_to(&roots, path) else {
            continue;
        };
        first
            .entry(relative)
            .and_modify(|earliest| *earliest = (*earliest).min(number.max(1)))
            .or_insert_with(|| number.max(1));
    }
    first
        .into_iter()
        .map(|(path, line)| ArmFinding {
            path: Some(path),
            line: Some(line),
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
fn rust_sources(root: &Path) -> Result<usize, String> {
    let walked = Jurisdiction::walk(root, &Ignore::default())
        .map_err(|error| format!("the selected tree could not be walked: {error}"))?;
    Ok(walked
        .measurable()
        .filter(|entry| !built(entry.path()))
        .filter(|entry| extension(entry.path()).is_some_and(|found| found == RUST_EXTENSION))
        .count())
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
fn decoded(root: &Path, path: &str) -> Result<String, String> {
    fs::read(root.join(path))
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
fn numbered_lines(root: &Path, path: &str) -> Result<Vec<(u32, String)>, String> {
    Ok(numbered(&decoded(root, path)?))
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
include!("analyze/tests.rs");

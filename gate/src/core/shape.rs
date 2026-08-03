//! Repository-shape acquisition over one captured selected-tree inventory.

use std::collections::{BTreeMap, BTreeSet};

use gix_glob::Pattern;
use regex::Regex;

use super::acquire::CoreSource;
use super::admission::validate_repo_path;
use super::{CoreEvidence, CoreFinding, CoreInputError, CoreObservation, RepoShapeEvidence};
use crate::input::{Contract, ShapePolicy};
use crate::jurisdiction::{Entry, Shape};

const MIN_NAME_CLUSTER_THRESHOLD: usize = 3;
const ALLOWED_FILES: &[&str] = &["__main__.py", "conftest.py", "py.typed"];

pub(super) fn repo_shape(
    source: &CoreSource<'_>,
    contract: &Contract,
) -> Result<CoreObservation, CoreInputError> {
    let policy = contract.shape();
    if policy.package_roots().is_empty() {
        return Ok(CoreObservation::Skipped {
            reason: "no Python package roots are declared in tracked policy".to_owned(),
        });
    }
    if policy.name_cluster_threshold() < MIN_NAME_CLUSTER_THRESHOLD {
        return Err(CoreInputError::new(format!(
            "policy.name_cluster_threshold must be at least {MIN_NAME_CLUSTER_THRESHOLD}"
        )));
    }

    let entries = source.jurisdiction().entries();
    let generated = GeneratedPaths::compile(policy.generated_paths())?;
    let roots = active_roots(entries, policy)?;
    if roots.is_empty() {
        return Ok(CoreObservation::Skipped {
            reason: "no declared Python package root exists in the selected tree".to_owned(),
        });
    }
    if !entries.iter().any(|entry| {
        entry.measurable()
            && entry.path().ends_with(".py")
            && roots.iter().any(|root| beneath(entry.path(), root))
            && !residue(entry.path())
            && !generated.matches(entry)
    }) {
        return Ok(CoreObservation::Skipped {
            reason: "declared Python package roots contain no admitted modules".to_owned(),
        });
    }

    let boundary = Regex::new(r"^_*[0-9a-z]+_*$").expect("compiled module-name policy");
    let mut findings = Vec::new();
    namespace_findings(entries, &roots, &generated, &mut findings)?;
    path_shape_findings(entries, &roots, &generated, &boundary, &mut findings)?;
    name_cluster_findings(
        entries,
        &roots,
        &generated,
        &boundary,
        policy.name_cluster_threshold(),
        &mut findings,
    )?;
    Ok(CoreObservation::Measured {
        evidence: CoreEvidence::RepoShape(RepoShapeEvidence::new(findings)),
    })
}

fn active_roots(entries: &[Entry], policy: &ShapePolicy) -> Result<Vec<String>, CoreInputError> {
    let mut roots = Vec::new();
    for root in policy.package_roots() {
        validate_repo_path(root)?;
        if entries.iter().any(|entry| {
            entry.path() == root && entry.admission().admits() && entry.shape() == Shape::Directory
        }) {
            roots.push(root.clone());
        }
    }
    Ok(roots)
}

fn namespace_findings(
    entries: &[Entry],
    roots: &[String],
    generated: &GeneratedPaths,
    findings: &mut Vec<CoreFinding>,
) -> Result<(), CoreInputError> {
    for entry in entries.iter().filter(|entry| {
        entry.measurable()
            && file_name(entry.path()) == "__init__.py"
            && roots.iter().any(|root| beneath(entry.path(), root))
            && !generated.matches(entry)
    }) {
        findings.push(CoreFinding::new(
            "namespace-package",
            Some(entry.path()),
            "__init__.py is forbidden under a declared namespace-package root",
        )?);
    }
    Ok(())
}

fn path_shape_findings(
    entries: &[Entry],
    roots: &[String],
    generated: &GeneratedPaths,
    boundary: &Regex,
    findings: &mut Vec<CoreFinding>,
) -> Result<(), CoreInputError> {
    let mut scan_roots = roots.to_vec();
    if entries.iter().any(|entry| {
        entry.path() == "tests" && entry.admission().admits() && entry.shape() == Shape::Directory
    }) {
        scan_roots.push("tests".to_owned());
    }
    scan_roots.sort();
    scan_roots.dedup();

    let files: BTreeSet<&str> = entries
        .iter()
        .filter(|entry| entry.measurable())
        .map(Entry::path)
        .collect();
    for entry in entries.iter().filter(|entry| {
        entry.admission().admits()
            && scan_roots
                .iter()
                .any(|root| entry.path() != root && beneath(entry.path(), root))
            && !residue(entry.path())
            && !generated.matches(entry)
    }) {
        let name = file_name(entry.path());
        if entry.shape() == Shape::Directory {
            if !boundary.is_match(name) {
                findings.push(CoreFinding::new(
                    "path-shape",
                    Some(entry.path()),
                    "directory name violates the boundary-underscore module shape",
                )?);
            }
            continue;
        }
        if entry.shape() != Shape::File || !name.ends_with(".py") || ALLOWED_FILES.contains(&name) {
            continue;
        }

        let stem = name.trim_end_matches(".py");
        let module = if beneath(entry.path(), "tests") {
            stem.strip_prefix("test_")
        } else {
            Some(stem)
        };
        if !module.is_some_and(|module| boundary.is_match(module)) {
            findings.push(CoreFinding::new(
                "path-shape",
                Some(entry.path()),
                "file name violates the boundary-underscore module shape",
            )?);
        } else if stem.as_bytes().last().is_some_and(u8::is_ascii_digit) {
            findings.push(CoreFinding::new(
                "generic-split",
                Some(entry.path()),
                "generic numbered split; name the seam instead",
            )?);
        } else if sibling_shard(entry.path(), &files) {
            findings.push(CoreFinding::new(
                "continuation-shard",
                Some(entry.path()),
                "continuation shard of a sibling; name the seam instead",
            )?);
        }
    }
    Ok(())
}

fn name_cluster_findings(
    entries: &[Entry],
    roots: &[String],
    generated: &GeneratedPaths,
    boundary: &Regex,
    threshold: usize,
    findings: &mut Vec<CoreFinding>,
) -> Result<(), CoreInputError> {
    let mut directories: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for entry in entries.iter().filter(|entry| {
        entry.measurable()
            && entry.path().ends_with(".py")
            && roots.iter().any(|root| beneath(entry.path(), root))
            && !residue(entry.path())
            && !generated.matches(entry)
            && !ALLOWED_FILES.contains(&file_name(entry.path()))
    }) {
        let stem = file_name(entry.path()).trim_end_matches(".py");
        if boundary.is_match(stem) {
            directories
                .entry(parent(entry.path()))
                .or_default()
                .push((stem.trim_matches('_'), entry.path()));
        }
    }

    for (directory, modules) in directories {
        for suffix in [false, true] {
            for (affix, members) in clusters(&modules, suffix, threshold) {
                let names = members
                    .iter()
                    .map(|path| file_name(path))
                    .collect::<Vec<_>>()
                    .join(", ");
                let kind = if suffix { "suffix" } else { "prefix" };
                findings.push(CoreFinding::new(
                    format!("name-cluster-{kind}"),
                    Some(directory),
                    format!(
                        concat!(
                            "{} sibling modules share the {} {:?} ({}); ",
                            "split the shared concept into a namespace subpackage",
                        ),
                        members.len(),
                        kind,
                        affix,
                        names,
                    ),
                )?);
            }
        }
    }
    Ok(())
}

fn clusters<'a>(
    modules: &[(&'a str, &'a str)],
    suffix: bool,
    threshold: usize,
) -> Vec<(String, Vec<&'a str>)> {
    let mut affixes = BTreeSet::new();
    for (index, (left, _)) in modules.iter().enumerate() {
        for (right, _) in &modules[index + 1..] {
            let affix = common_affix(left, right, suffix);
            if affix.len() >= 4 && affix.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                affixes.insert(affix);
            }
        }
    }
    let mut candidates: Vec<_> = affixes
        .into_iter()
        .filter_map(|affix| {
            let members: Vec<_> = modules
                .iter()
                .filter(|(stem, _)| {
                    stem.len() > affix.len()
                        && if suffix {
                            stem.ends_with(&affix)
                        } else {
                            stem.starts_with(&affix)
                        }
                })
                .map(|(_, path)| *path)
                .collect();
            (members.len() >= threshold).then_some((affix, members))
        })
        .collect();
    candidates.sort_by(|left, right| {
        left.0
            .len()
            .cmp(&right.0.len())
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut selected: Vec<(String, Vec<&str>)> = Vec::new();
    for candidate in candidates {
        let member_set: BTreeSet<_> = candidate.1.iter().copied().collect();
        if selected.iter().any(|(_, selected_members)| {
            let selected_set: BTreeSet<_> = selected_members.iter().copied().collect();
            member_set.is_subset(&selected_set)
        }) {
            continue;
        }
        selected.push(candidate);
    }
    selected
}

fn common_affix(left: &str, right: &str, suffix: bool) -> String {
    if suffix {
        let count = left
            .bytes()
            .rev()
            .zip(right.bytes().rev())
            .take_while(|(left, right)| left == right)
            .count();
        left[left.len().saturating_sub(count)..].to_owned()
    } else {
        let count = left
            .bytes()
            .zip(right.bytes())
            .take_while(|(left, right)| left == right)
            .count();
        left[..count].to_owned()
    }
}

fn sibling_shard(path: &str, files: &BTreeSet<&str>) -> bool {
    let name = file_name(path);
    let stem = name.trim_end_matches(".py");
    let Some((last, base)) = stem.as_bytes().split_last() else {
        return false;
    };
    if base.is_empty() || base.ends_with(b"_") {
        return false;
    }
    let base = String::from_utf8_lossy(base);
    let sibling = join(parent(path), &format!("{base}.py"));
    if !files.contains(sibling.as_str()) {
        return false;
    }
    if last.is_ascii_digit() {
        return true;
    }
    if !last.is_ascii_lowercase() {
        return false;
    }
    files
        .iter()
        .filter(|candidate| parent(candidate) == parent(path))
        .filter(|candidate| {
            let candidate = file_name(candidate).trim_end_matches(".py");
            candidate.len() == base.len() + 1
                && candidate.starts_with(base.as_ref())
                && candidate
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_lowercase)
        })
        .count()
        >= 2
}

fn beneath(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn residue(path: &str) -> bool {
    path.split('/').any(|part| part == "__pycache__") || path.ends_with(".pyc")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

struct GeneratedPaths {
    patterns: Vec<(String, Pattern)>,
}

impl GeneratedPaths {
    fn compile(patterns: &[String]) -> Result<Self, CoreInputError> {
        let mut compiled = Vec::new();
        for pattern in patterns {
            let Some(glob) = Pattern::from_bytes(pattern.as_bytes()) else {
                return Err(CoreInputError::new(format!(
                    "generated path pattern {pattern:?} is empty"
                )));
            };
            compiled.push((pattern.clone(), glob));
        }
        Ok(Self { patterns: compiled })
    }

    fn matches(&self, entry: &Entry) -> bool {
        self.patterns.iter().any(|(source, pattern)| {
            if !source.contains(['*', '?', '[']) && beneath(entry.path(), source) {
                return true;
            }
            pattern.matches_repo_relative_path(
                entry.path().as_bytes().into(),
                entry.path().rfind('/').map(|slash| slash + 1),
                Some(entry.shape() == Shape::Directory),
                gix_glob::pattern::Case::Sensitive,
                gix_glob::wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            )
        })
    }
}

//! Fact-recorded phase boundaries first; Git trailers are corroboration, not ownership.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

use super::Result;
use super::publication::{Quantity, derived_json, quantity};
use super::reduce::Study;

const GIT_CHUNK: usize = 128;
const RECORD_SEPARATOR: char = '\u{1e}';
const FIELD_SEPARATOR: char = '\u{1f}';

#[derive(Serialize)]
struct Object {
    id: String,
    parents: Vec<String>,
    timestamp: String,
    task_key: String,
    task_handle: String,
    project: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct GitCase {
    pub evidence: String,
    pub implementation_head: String,
    pub integrated: String,
    pub merge_parents: Vec<String>,
    pub head_is_merge_parent: bool,
    pub head_is_ancestor: bool,
    pub contribution_commits: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Concordance {
    pub implementation_phases: usize,
    pub recorded_integrated: usize,
    pub absent_integrated: usize,
    pub object_references: usize,
    pub available_objects: usize,
    pub missing_objects: usize,
    pub matching_head_trailers: usize,
    pub disagreeing_head_trailers: usize,
    pub absent_head_trailers: usize,
    pub separately_dated_integration: usize,
    pub cases: Vec<GitCase>,
}

type Phase<'a> = (&'a String, &'a super::reduce::Observation);

pub(super) fn observe(study: &Study, directory: &Path) -> Result<Concordance> {
    let (phases, objects) = referenced(study);
    let known = available_commits(&objects, directory)?;
    let metadata = read_metadata(&known)?;
    let aliases = metadata
        .keys()
        .enumerate()
        .map(|(index, id)| (id.clone(), format!("G{:04}", index + 1)))
        .collect::<BTreeMap<_, _>>();
    let mut result = Concordance {
        implementation_phases: phases.len(),
        recorded_integrated: 0,
        absent_integrated: 0,
        object_references: objects.len(),
        available_objects: metadata.len(),
        missing_objects: objects.len() - metadata.len(),
        matching_head_trailers: 0,
        disagreeing_head_trailers: 0,
        absent_head_trailers: 0,
        separately_dated_integration: 0,
        cases: Vec::new(),
    };
    for phase in &phases {
        tally(phase, &metadata, &mut result);
    }
    for (birth, evidence) in [
        ("dS8s5DQTD", "E01"),
        ("dS977gL0K", "E01"),
        ("dSw0Rz0zJ", "E03"),
        ("dTv81GDMK", "E04"),
        ("1kSgr8J0", "E05"),
    ] {
        result
            .cases
            .push(curated_case(study, birth, evidence, &metadata, &aliases)?);
    }
    let revision = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
    if !revision.status.success() {
        return Err("repository evidence revision unavailable".into());
    }
    derived_json(
        &directory.join("git-private.json"),
        &serde_json::json!({
            "version": 1, "repository_revision": String::from_utf8(revision.stdout)?.trim(),
            "input_fact_sha256": study.manifest.facts_sha256, "objects": metadata, "aliases": aliases,
            "missing": objects.into_iter().filter(|id| !known.contains(id)).collect::<Vec<_>>()
        }),
    )?;
    Ok(result)
}

/// Every non-review phase end and every Git object its recorded boundary names.
fn referenced(study: &Study) -> (Vec<Phase<'_>>, BTreeSet<String>) {
    let mut objects = BTreeSet::<String>::new();
    let mut phases = Vec::new();
    for task in study.tasks.values() {
        for observation in task
            .phase_ends
            .iter()
            .filter(|observation| observation.phase != "review")
        {
            phases.push((&task.birth, observation));
            let Some(git) = &observation.git else {
                continue;
            };
            for name in ["head", "integrated", "upstream_head", "baseline"] {
                if let Some(object) = git.get(name).and_then(serde_json::Value::as_str) {
                    objects.insert(object.to_owned());
                }
            }
        }
    }
    (phases, objects)
}

fn available_commits(objects: &BTreeSet<String>, directory: &Path) -> Result<Vec<String>> {
    let request = directory.join("git-object-request.txt");
    fs::write(
        &request,
        objects
            .iter()
            .map(|id| format!("{id}\n"))
            .collect::<String>(),
    )?;
    let checked = Command::new("git")
        .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"])
        .stdin(Stdio::from(File::open(&request)?))
        .output()?;
    if !checked.status.success() {
        return Err("Git object availability read failed".into());
    }
    Ok(String::from_utf8(checked.stdout)?
        .lines()
        .filter_map(|line| {
            let (id, kind) = line.split_once(' ')?;
            (kind == "commit").then(|| id.to_owned())
        })
        .collect())
}

fn read_metadata(known: &[String]) -> Result<BTreeMap<String, Object>> {
    let mut metadata = BTreeMap::new();
    let format = "--format=%H%x1f%P%x1f%ct%x1f%(trailers:key=Task-Key,valueonly)%x1f%(trailers:key=Task-Handle,valueonly)%x1f%(trailers:key=Task-Project,valueonly)%x1e";
    for chunk in known.chunks(GIT_CHUNK) {
        let result = Command::new("git")
            .args(["show", "--no-patch", format, "--end-of-options"])
            .args(chunk)
            .output()?;
        if !result.status.success() {
            return Err("Git metadata read failed after object admission".into());
        }
        for row in String::from_utf8(result.stdout)?
            .split(RECORD_SEPARATOR)
            .map(str::trim)
            .filter(|row| !row.is_empty())
        {
            let fields = row.split(FIELD_SEPARATOR).collect::<Vec<_>>();
            let [id, parents, timestamp, task_key, task_handle, project] = fields.as_slice() else {
                return Err("Git metadata record has an unsupported shape".into());
            };
            metadata.insert(
                (*id).to_owned(),
                Object {
                    id: (*id).to_owned(),
                    parents: parents.split_whitespace().map(str::to_owned).collect(),
                    timestamp: timestamp.trim().to_owned(),
                    task_key: task_key.trim().to_owned(),
                    task_handle: task_handle.trim().to_owned(),
                    project: project.trim().to_owned(),
                },
            );
        }
    }
    Ok(metadata)
}

fn tally(
    (birth, observation): &Phase<'_>,
    metadata: &BTreeMap<String, Object>,
    result: &mut Concordance,
) {
    let Some(git) = &observation.git else {
        result.absent_integrated += 1;
        result.absent_head_trailers += 1;
        return;
    };
    if git
        .get("integrated")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        result.recorded_integrated += 1;
    } else {
        result.absent_integrated += 1;
    }
    let object = git
        .get("head")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| metadata.get(id));
    match object.and_then(trailer_birth) {
        Some(trailer) if &trailer == *birth => result.matching_head_trailers += 1,
        Some(_) => result.disagreeing_head_trailers += 1,
        None => result.absent_head_trailers += 1,
    }
}

fn curated_case(
    study: &Study,
    birth: &str,
    evidence: &str,
    metadata: &BTreeMap<String, Object>,
    aliases: &BTreeMap<String, String>,
) -> Result<GitCase> {
    let observation = study
        .tasks
        .get(birth)
        .ok_or("curated Git case lacks its task")?
        .phase_ends
        .iter()
        .rev()
        .find(|observation| observation.phase == "todo")
        .ok_or("curated Git case lacks implementation completion")?;
    let git = observation
        .git
        .as_ref()
        .ok_or("curated Git case lacks recorded boundary")?;
    let head = git
        .get("head")
        .and_then(serde_json::Value::as_str)
        .ok_or("curated Git head absent")?;
    let integrated = git
        .get("integrated")
        .and_then(serde_json::Value::as_str)
        .ok_or("curated integration absent")?;
    let object = metadata
        .get(integrated)
        .ok_or("curated integrated object unavailable")?;
    let ancestor = Command::new("git")
        .args(["merge-base", "--is-ancestor", head, integrated])
        .status()?;
    if !matches!(ancestor.code(), Some(0 | 1)) {
        return Err("curated Git ancestry could not be observed".into());
    }
    Ok(GitCase {
        evidence: evidence.to_owned(),
        implementation_head: aliases
            .get(head)
            .ok_or("curated head object unavailable")?
            .clone(),
        integrated: aliases[integrated].clone(),
        merge_parents: object
            .parents
            .iter()
            .enumerate()
            .map(|(index, parent)| {
                aliases
                    .get(parent)
                    .cloned()
                    .unwrap_or_else(|| format!("P{} (context parent)", index + 1))
            })
            .collect(),
        head_is_merge_parent: object.parents.iter().any(|parent| parent == head),
        head_is_ancestor: ancestor.success(),
        contribution_commits: contribution(object)?,
    })
}

/// Commits a two-parent integration brought in beyond its first parent.
fn contribution(object: &Object) -> Result<Option<usize>> {
    let [first, second] = object.parents.as_slice() else {
        return Ok(None);
    };
    let range = Command::new("git")
        .args(["rev-list", "--count", &format!("{first}..{second}")])
        .output()?;
    if !range.status.success() {
        return Err("curated merge contribution range unavailable".into());
    }
    Ok(Some(String::from_utf8(range.stdout)?.trim().parse()?))
}

fn trailer_birth(object: &Object) -> Option<String> {
    let field = if object.task_key.is_empty() {
        &object.task_handle
    } else {
        &object.task_key
    };
    // Same Task-Key/Task-Handle precedence and historical handle contraction as release_history.rs.
    let mut lines = field.lines();
    let value = lines.next()?.trim();
    if value.is_empty() || lines.next().is_some() {
        return None;
    }
    Some(
        value
            .rsplit_once('-')
            .map_or(value, |(_, birth)| birth)
            .to_owned(),
    )
}

pub(super) fn quantities(git: &Concordance) -> Vec<Quantity> {
    vec![
        quantity(
            "Q25",
            "Recorded integration boundaries",
            "All admitted completed non-review phases by the horizon",
            "phases",
            "Whether the phase-ending fact carries an integrated boundary. A boundary observed at done does not separately date integration; absence is coverage, not failed landing.",
            git.implementation_phases,
            [
                (
                    "Recorded integrated boundary".to_owned(),
                    git.recorded_integrated as f64,
                ),
                (
                    "No recorded integrated boundary".to_owned(),
                    git.absent_integrated as f64,
                ),
                (
                    "Separately dated done-to-integration interval".to_owned(),
                    git.separately_dated_integration as f64,
                ),
            ],
        ),
        quantity(
            "Q26",
            "Git object coverage",
            "Distinct object identities referenced by completed non-review phase facts",
            "objects",
            "Exact recorded identities checked against the pinned repository snapshot; unavailable objects are never replaced by nearby commits.",
            git.object_references,
            [
                ("Available commits".to_owned(), git.available_objects as f64),
                (
                    "Unavailable references".to_owned(),
                    git.missing_objects as f64,
                ),
            ],
        ),
        quantity(
            "Q27",
            "Implementation-head trailer concordance",
            "Same completed non-review phases as Q25",
            "phases",
            "Task-Key outranks Task-Handle; historical handles contract to canonical birth. Trailers corroborate the implementation head, not an unrelated final review head. Missing or disagreeing trailers are named evidence, not automatic ownership or failure.",
            git.implementation_phases,
            [
                (
                    "Matching retained task trailer".to_owned(),
                    git.matching_head_trailers as f64,
                ),
                (
                    "Disagreeing retained task trailer".to_owned(),
                    git.disagreeing_head_trailers as f64,
                ),
                (
                    "No unambiguous retained task trailer".to_owned(),
                    git.absent_head_trailers as f64,
                ),
            ],
        ),
    ]
}

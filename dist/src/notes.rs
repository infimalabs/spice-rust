//! Release-note folding from the first-parent landing ledger.

use std::collections::{BTreeMap, BTreeSet};

use wire::manifest::RELEASE_NOTES_VERSION;
use wire::{Digest, Domain, ReleaseNotes, ReleaseNotesEntry};

const GIT_OBJECT_HEX_LEN: usize = 40;

/// Marker separating curator-owned prose from the regenerated inventory.
pub const INVENTORY_OPEN: &str = "<details>";
/// Placeholder that makes an untouched draft unpublishable.
pub const HIGHLIGHTS_PLACEHOLDER: &str = concat!(
    "_Replace this line with a short, curated set of highlights folded from ",
    "the changes below._",
);

/// One first-parent landing observed by the repository rim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Landing {
    /// Fully resolved landing commit.
    pub commit: String,
    /// First parent immediately preceding this landing, absent for a root.
    pub first_parent: Option<String>,
    /// Commit subject.
    pub subject: String,
    /// Project declared by the commit.
    pub project: String,
    /// Stable task identity declared by the commit, when present.
    pub task: Option<String>,
    /// Commit body used to recognize a structural revert.
    pub body: String,
}

/// Complete observations needed to fold one release-note document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotesInput {
    /// Version named by the release page.
    pub release_version: String,
    /// Fully resolved release commit.
    pub release_commit: String,
    /// Highest-versioned reachable prior release tag.
    pub previous_tag: Option<String>,
    /// Ordered first-parent landings in the selected window.
    pub landings: Vec<Landing>,
}

/// Fold a release-note draft from first-parent landings.
///
/// `contains` answers whether a reverted side-branch commit is reachable from
/// a candidate commit. A landing introduces the target only when the target is
/// reachable from the landing but not its first parent. Keeping both Git
/// observations at the rim leaves suppression, task collapsing, grouping, and
/// rendering deterministic.
///
/// # Errors
///
/// Returns the ancestry observation failure with both commits that caused it.
pub fn release_notes(
    input: NotesInput,
    mut contains: impl FnMut(&str, &str) -> Result<bool, String>,
) -> Result<ReleaseNotes, NotesError> {
    let suppressed = suppressed_landings(&input.landings, &mut contains)?;
    let entries = fold_entries(&input.landings, &suppressed);
    let document = render(&input, &entries);
    Ok(ReleaseNotes {
        version: RELEASE_NOTES_VERSION,
        identity: Digest::compute(Domain::DistReleaseNotes, document.as_bytes()),
        release_version: input.release_version,
        release_commit: input.release_commit,
        previous_tag: input.previous_tag.clone(),
        bounded_recent_window: input.previous_tag.is_none(),
        entries,
        document,
    })
}

/// Keep curator-owned prose and regenerate the inventory from `canonical`.
///
/// # Errors
///
/// Refuses the untouched generated draft, any surviving placeholder, a
/// curator region containing only generated banner/headings, or a canonical
/// document with no inventory boundary.
pub fn curated_release_notes(
    candidate: &str,
    canonical: &ReleaseNotes,
) -> Result<ReleaseNotes, NotesError> {
    if candidate.as_bytes() == canonical.document.as_bytes() {
        return Err(NotesError::UntouchedDraft);
    }
    if candidate.contains(HIGHLIGHTS_PLACEHOLDER) {
        return Err(NotesError::HighlightsPlaceholder);
    }
    let curated = curated_region(candidate);
    if !carries_highlights(curated) {
        return Err(NotesError::HighlightsAbsent);
    }
    let inventory = canonical
        .document
        .find(INVENTORY_OPEN)
        .ok_or(NotesError::InventoryMissing)?;
    let document = format!(
        "{}\n\n{}",
        curated.trim_end(),
        &canonical.document[inventory..]
    );
    let mut notes = canonical.clone();
    notes.identity = Digest::compute(Domain::DistReleaseNotes, document.as_bytes());
    notes.document = document;
    Ok(notes)
}

fn suppressed_landings(
    landings: &[Landing],
    contains: &mut impl FnMut(&str, &str) -> Result<bool, String>,
) -> Result<BTreeSet<usize>, NotesError> {
    let mut suppressed = BTreeSet::new();
    for (revert_index, revert) in landings.iter().enumerate() {
        let Some(target) = reverted_commit(&revert.body) else {
            continue;
        };
        let mut introduced = None;
        for (landing_index, landing) in landings.iter().enumerate() {
            if landing_index == revert_index {
                continue;
            }
            let included =
                contains(target, &landing.commit).map_err(|detail| NotesError::Ancestry {
                    target: target.to_owned(),
                    landing: landing.commit.clone(),
                    detail,
                })?;
            let included_by_parent = match landing.first_parent.as_deref() {
                Some(parent) if included => {
                    contains(target, parent).map_err(|detail| NotesError::Ancestry {
                        target: target.to_owned(),
                        landing: parent.to_owned(),
                        detail,
                    })?
                }
                Some(_) | None => false,
            };
            if included && !included_by_parent {
                introduced = Some(landing_index);
                break;
            }
        }
        let Some(introduced) = introduced else {
            continue;
        };
        suppressed.insert(revert_index);
        let task = landings[introduced].task.as_deref();
        if let Some(task) = task {
            for (index, landing) in landings.iter().enumerate() {
                if landing.task.as_deref() == Some(task) {
                    suppressed.insert(index);
                }
            }
        } else {
            suppressed.insert(introduced);
        }
    }
    Ok(suppressed)
}

fn reverted_commit(body: &str) -> Option<&str> {
    const PREFIX: &str = "This reverts commit ";
    let tail = body.split_once(PREFIX)?.1;
    let commit = tail
        .split(|character: char| !character.is_ascii_hexdigit())
        .next()?;
    (commit.len() >= 7 && commit.len() <= GIT_OBJECT_HEX_LEN).then_some(commit)
}

fn fold_entries(landings: &[Landing], suppressed: &BTreeSet<usize>) -> Vec<ReleaseNotesEntry> {
    let mut entries: Vec<ReleaseNotesEntry> = Vec::new();
    let mut task_positions: BTreeMap<String, usize> = BTreeMap::new();
    for (index, landing) in landings.iter().enumerate() {
        if suppressed.contains(&index) {
            continue;
        }
        let project = project_key(&landing.project);
        let subject =
            release_note_subject(&landing.subject, landing.task.as_deref(), &landing.project);
        if let Some(task) = landing.task.as_deref()
            && let Some(position) = task_positions.get(task).copied()
        {
            let entry = &mut entries[position];
            entry.project = project;
            entry.subject = subject;
            entry.references.push(landing.commit.clone());
            continue;
        }
        if let Some(task) = landing.task.as_deref() {
            task_positions.insert(task.to_owned(), entries.len());
        }
        entries.push(ReleaseNotesEntry {
            project,
            subject,
            task: landing.task.clone(),
            references: vec![landing.commit.clone()],
        });
    }
    entries
}

fn render(input: &NotesInput, entries: &[ReleaseNotesEntry]) -> String {
    let mut groups: Vec<ProjectGroup> = Vec::new();
    for entry in entries {
        let group = if let Some(index) = groups
            .iter()
            .position(|group| group.project == entry.project)
        {
            &mut groups[index].highlights
        } else {
            groups.push(ProjectGroup {
                project: entry.project.clone(),
                highlights: Vec::new(),
            });
            &mut groups
                .last_mut()
                .expect("group was just inserted")
                .highlights
        };
        let text = edited_release_highlight(&entry.subject);
        if let Some(highlight) = group.iter_mut().find(|highlight| highlight.text == text) {
            highlight
                .references
                .extend(entry.references.iter().cloned());
        } else {
            group.push(ProjectHighlight {
                text,
                references: entry.references.clone(),
            });
        }
    }

    let mut lines = vec![
        "> [!IMPORTANT]".to_owned(),
        "> **Draft release notes — curate Highlights before publishing.** Replace".to_owned(),
        "> the placeholder under _Highlights_ with a short summary, then delete this".to_owned(),
        "> banner. Everything from the collapsed _Task-level changes_ section down".to_owned(),
        "> is regenerated at publication against the real release commit.".to_owned(),
        String::new(),
        "## Highlights".to_owned(),
        String::new(),
        HIGHLIGHTS_PLACEHOLDER.to_owned(),
        String::new(),
        INVENTORY_OPEN.to_owned(),
        "<summary>Task-level changes</summary>".to_owned(),
        String::new(),
        "## Changes by project".to_owned(),
        String::new(),
    ];
    if groups.is_empty() {
        lines.extend(["- No non-release commits found.".to_owned(), String::new()]);
    } else {
        for ProjectGroup {
            project,
            highlights,
        } in groups
        {
            lines.extend([format!("### {}", project_heading(&project)), String::new()]);
            for highlight in highlights {
                let references = highlight
                    .references
                    .iter()
                    .map(|reference| short_commit(reference))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("- {} ({references})", highlight.text));
            }
            lines.push(String::new());
        }
    }
    let release_short = short_commit(&input.release_commit);
    lines.extend([
        "</details>".to_owned(),
        String::new(),
        "## Package Notes".to_owned(),
        String::new(),
        format!("- Spice release: `{}`", input.release_version),
        format!("- Release commit: `{release_short}`"),
    ]);
    if let Some(previous) = &input.previous_tag {
        lines.push(format!(
            "- Commit range: `refs/tags/{previous}..{release_short}`"
        ));
    } else {
        lines.push(format!(
            "- Commit range: bounded latest first-parent commits ending at `{release_short}`"
        ));
    }
    lines.push(
        "- Commit source: first-parent history grouped by `Task-Project` metadata".to_owned(),
    );
    lines.push(String::new());
    lines.join("\n")
}

struct ProjectGroup {
    project: String,
    highlights: Vec<ProjectHighlight>,
}

struct ProjectHighlight {
    text: String,
    references: Vec<String>,
}

fn curated_region(candidate: &str) -> &str {
    candidate
        .split_once(INVENTORY_OPEN)
        .map_or(candidate, |(curated, _)| curated)
}

fn carries_highlights(curated: &str) -> bool {
    curated.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with(['>', '#'])
    })
}

fn release_note_subject(subject: &str, task: Option<&str>, project: &str) -> String {
    let mut trimmed = strip_phase_prefix(subject.trim());
    let stem = project.split('.').next().unwrap_or(project).trim();
    if !stem.is_empty()
        && trimmed
            .get(..stem.len().saturating_add(2))
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&format!("{stem}: ")))
    {
        trimmed = trimmed[stem.len() + 2..].trim();
        trimmed = strip_phase_suffix(trimmed);
    }
    if let Some(task) = task
        && let Some((head, tail)) = trimmed.rsplit_once(' ')
        && !head.is_empty()
        && tail.ends_with(&format!("-{task}"))
    {
        trimmed = head.trim_end();
    }
    trimmed.to_owned()
}

fn strip_phase_prefix(subject: &str) -> &str {
    const PHASES: [&str; 5] = ["design", "plan", "todo", "verify", "review"];
    let Some(close) = subject.find("): ") else {
        return subject;
    };
    let Some((phase, _)) = subject[..close].split_once('(') else {
        return subject;
    };
    if PHASES
        .iter()
        .any(|candidate| phase.eq_ignore_ascii_case(candidate))
    {
        &subject[close + 3..]
    } else {
        subject
    }
}

fn strip_phase_suffix(subject: &str) -> &str {
    const PHASES: [&str; 4] = ["design", "plan", "verify", "review"];
    for phase in PHASES {
        let suffix = format!(" ({phase})");
        if subject
            .get(subject.len().saturating_sub(suffix.len())..)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&suffix))
        {
            return &subject[..subject.len() - suffix.len()];
        }
    }
    subject
}

fn edited_release_highlight(subject: &str) -> String {
    let raw = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if raw.is_empty() {
        return "Updated the release.".to_owned();
    }
    const REPLACEMENTS: [(&str, &str); 12] = [
        ("fix ", "Fixed "),
        ("prefer ", "Improved "),
        ("add ", "Added "),
        ("expose ", "Added "),
        ("remove ", "Removed "),
        ("update ", "Updated "),
        ("track ", "Tracked "),
        ("document ", "Documented "),
        ("restore ", "Restored "),
        ("clean ", "Cleaned "),
        ("wire ", "Wired "),
        ("make ", "Made "),
    ];
    let lower = raw.to_lowercase();
    let rewritten = REPLACEMENTS
        .iter()
        .find(|(prefix, _)| lower.starts_with(prefix))
        .map_or_else(
            || capitalize_first(&raw),
            |(prefix, replacement)| format!("{replacement}{}", &raw[prefix.len()..]),
        );
    punctuate(rewritten)
}

fn project_key(project: &str) -> String {
    let project = project.trim().to_lowercase();
    if project.is_empty() || project.starts_with("agent.") {
        "general".to_owned()
    } else {
        project
    }
}

fn project_heading(project: &str) -> String {
    let words = project
        .replace('_', "-")
        .split(['.', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| match word {
            "cli" => "CLI".to_owned(),
            "ui" => "UI".to_owned(),
            _ => capitalize_first(word),
        })
        .collect::<Vec<_>>();
    if words.is_empty() {
        "General".to_owned()
    } else {
        words.join(" ")
    }
}

fn capitalize_first(text: &str) -> String {
    let mut characters = text.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_uppercase().chain(characters).collect()
}

fn punctuate(mut text: String) -> String {
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Why landing history could not become publishable release notes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotesError {
    /// A containment query failed instead of answering true or false.
    #[error(
        "could not test whether reverted commit {target} belongs to landing {landing}: {detail}"
    )]
    Ancestry {
        target: String,
        landing: String,
        detail: String,
    },
    /// The operator supplied the generated draft unchanged.
    #[error("release notes still exactly match the generated draft")]
    UntouchedDraft,
    /// The generated curation placeholder remains anywhere in the candidate.
    #[error("release notes still carry the Highlights placeholder")]
    HighlightsPlaceholder,
    /// The curator-owned region contains no actual prose or list entry.
    #[error("the curated release-note region contains only banner text and headings")]
    HighlightsAbsent,
    /// The canonical renderer failed to provide its generated-tail boundary.
    #[error("the canonical release-note document has no task inventory boundary")]
    InventoryMissing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverts_suppress_the_whole_task_and_repeated_landings_keep_every_reference() {
        let input = fixture(vec![
            landing(
                "1111111111",
                "canvas: Add palette UI-keep",
                "canvas.ui",
                Some("keep"),
                "",
            ),
            landing(
                "2222222222",
                "canvas: Fix palette UI-keep (review)",
                "canvas.ui",
                Some("keep"),
                "",
            ),
            landing(
                "3333333333",
                "gate: Add seam GATE-gone",
                "gate.tests",
                Some("gone"),
                "",
            ),
            landing(
                "4444444444",
                "gate: Verify seam GATE-gone (review)",
                "gate.tests",
                Some("gone"),
                "",
            ),
            landing(
                "5555555555",
                "Revert gate seam",
                "gate.tests",
                None,
                "This reverts commit abcdef0123456789.",
            ),
            landing("6666666666", "Document fallback", "", None, ""),
        ]);

        let notes = release_notes(input, |target, landing| {
            Ok(target == "abcdef0123456789" && landing == "3333333333")
        })
        .expect("history folds");

        assert_eq!(notes.entries.len(), 2);
        assert_eq!(notes.entries[0].subject, "Fix palette");
        assert_eq!(notes.entries[0].references, ["1111111111", "2222222222"]);
        assert_eq!(notes.entries[1].project, "general");
        assert!(!notes.document.contains("seam"));
        assert!(notes.document.contains("### Canvas UI"));
        assert!(notes.document.contains("Fixed palette. (1111111, 2222222)"));
    }

    #[test]
    fn no_tag_is_a_bounded_and_explicit_window() {
        let notes = release_notes(fixture(Vec::new()), |_target, _landing| Ok(false))
            .expect("empty history folds");

        assert!(notes.bounded_recent_window);
        assert!(notes.previous_tag.is_none());
        assert!(
            notes
                .document
                .contains("bounded latest first-parent commits")
        );
        assert!(notes.document.contains("No non-release commits found"));
    }

    #[test]
    fn curation_is_required_and_only_the_canonical_tail_survives() {
        let canonical = release_notes(
            NotesInput {
                previous_tag: Some("v1.2.3".to_owned()),
                ..fixture(vec![landing(
                    "1234567890",
                    "dist: Add history notes NOTES-note",
                    "dist.notes",
                    Some("note"),
                    "",
                )])
            },
            |_target, _landing| Ok(false),
        )
        .expect("history folds");

        assert_eq!(
            curated_release_notes(&canonical.document, &canonical),
            Err(NotesError::UntouchedDraft)
        );
        assert_eq!(
            curated_release_notes(
                concat!(
                    "## Highlights\n\n_Replace this line with a short, curated set of ",
                    "highlights folded from the changes below._",
                ),
                &canonical
            ),
            Err(NotesError::HighlightsPlaceholder)
        );
        assert_eq!(
            curated_release_notes("> banner\n\n## Highlights\n", &canonical),
            Err(NotesError::HighlightsAbsent)
        );

        let candidate = concat!(
            "## Highlights\r\n\r\n- Release notes now follow landing history.",
            "\r\n\r\n<details>\r\nold inventory",
        );
        let curated = curated_release_notes(candidate, &canonical).expect("highlights curate");
        assert!(
            curated
                .document
                .starts_with("## Highlights\r\n\r\n- Release notes now follow landing history.")
        );
        assert!(curated.document.contains("Added history notes"));
        assert!(!curated.document.contains("old inventory"));
        assert_ne!(curated.identity, canonical.identity);
    }

    #[test]
    fn a_handle_shaped_suffix_is_removed_only_for_its_own_task() {
        let notes = release_notes(
            fixture(vec![
                landing("1111111111", "Fix alpha MAYBE-other", "", Some("own"), ""),
                landing("2222222222", "Fix beta TASK-own", "", Some("own-2"), ""),
            ]),
            |_target, _landing| Ok(false),
        )
        .expect("history folds");

        assert_eq!(notes.entries[0].subject, "Fix alpha MAYBE-other");
        assert_eq!(notes.entries[1].subject, "Fix beta TASK-own");
    }

    fn fixture(landings: Vec<Landing>) -> NotesInput {
        NotesInput {
            release_version: "1.2.4".to_owned(),
            release_commit: "abcdef0123456789".to_owned(),
            previous_tag: None,
            landings,
        }
    }

    fn landing(
        commit: &str,
        subject: &str,
        project: &str,
        task: Option<&str>,
        body: &str,
    ) -> Landing {
        Landing {
            commit: commit.to_owned(),
            first_parent: Some(format!("parent-{commit}")),
            subject: subject.to_owned(),
            project: project.to_owned(),
            task: task.map(str::to_owned),
            body: body.to_owned(),
        }
    }
}

//! The folder oh-my-pi names for one working directory, and the transcripts
//! it files there.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::Environment;

const RELATED_DIRECTORY_LIMIT: usize = 256;
const STORED_FOLDER_LIMIT: usize = 4_096;
const STORED_ENTRY_LIMIT: usize = 65_536;

/// The transcripts omp keeps for `thread` beneath the folder named by
/// the working directory this process was asked from, or failing that by
/// the nearest ancestor of that directory whose folder holds one.
///
/// Resolving one's own live thread is by far the most common lookup, and
/// the process asking is usually sitting in the directory that named the
/// folder, or in a subdirectory of it (`target/`, a crate, a scratch dir),
/// so this needs no search at all, only the folders named by the directory
/// and its ancestors checked nearest first under the same regular-file rule
/// the walk admits by. Nearest wins, so a nested project with its own
/// session folder is never shadowed by its parent's; the ancestor count is
/// the path depth, so no bound is needed. A miss is silent: the caller tries
/// sibling worktrees and then walks the sessions root exactly as if this
/// were never tried.
pub(super) fn working_directory_transcripts(
    sessions: &Path,
    thread: &str,
    environment: &Environment,
) -> Vec<PathBuf> {
    let Some(cwd) = environment.cwd.as_deref() else {
        return Vec::new();
    };
    let Ok(cwd) = fs::canonicalize(cwd) else {
        return Vec::new();
    };
    cwd.ancestors()
        .filter_map(|directory| omp_session_folder(directory, environment))
        .map(|folder| transcripts_in(&sessions.join(folder), thread))
        .find(|transcripts| !transcripts.is_empty())
        .unwrap_or_default()
}

/// OMP folders implied by sibling working directories that share the current
/// directory's stem. Git worktrees commonly differ only in a final lane
/// suffix (`project-a`, `project-b`, ...), so this resolves handoffs without
/// inspecting unrelated stored sessions.
pub(super) fn related_working_directory_transcripts(
    sessions: &Path,
    thread: &str,
    environment: &Environment,
) -> Vec<PathBuf> {
    let Some(cwd) = environment.cwd.as_deref() else {
        return Vec::new();
    };
    let cwd = match fs::canonicalize(cwd) {
        Ok(cwd) => cwd,
        Err(_) => return Vec::new(),
    };
    let Some(parent) = cwd.parent() else {
        return Vec::new();
    };
    let Some(name) = cwd.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let Some((stem, _)) = name.rsplit_once('-') else {
        return Vec::new();
    };
    let prefix = format!("{stem}-");
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate == cwd
            || !entry.file_name().to_string_lossy().starts_with(&prefix)
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        candidates.push(candidate);
    }
    candidates.sort();
    candidates.truncate(RELATED_DIRECTORY_LIMIT);

    let mut matches = BTreeSet::new();
    for candidate in candidates {
        let Some(folder) = omp_session_folder(&candidate, environment) else {
            continue;
        };
        matches.extend(transcripts_in(&sessions.join(folder), thread));
    }
    matches.into_iter().collect()
}

/// Every matching transcript in OMP's known two-level storage layout.
///
/// This is deliberately not a recursive filesystem walk: OMP files a
/// transcript directly under one working-directory folder and puts artifacts
/// in a same-stem directory beside it. The generous bounds protect malformed
/// stores while keeping normal lookup proportional to directory entries, not
/// transcript bytes.
pub(super) fn stored_transcripts(sessions: &Path, thread: &str) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(sessions)
        .map_err(|error| format!("could not read directory {}: {error}", sessions.display()))?;
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "could not read an entry beneath {}: {error}",
                sessions.display()
            )
        })?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?
            .is_dir()
        {
            folders.push(path);
            if folders.len() > STORED_FOLDER_LIMIT {
                return Err(format!(
                    "OMP session-folder search exceeded its {STORED_FOLDER_LIMIT}-folder limit beneath {}",
                    sessions.display()
                ));
            }
        }
    }
    folders.sort();

    let suffixes = [format!("_{thread}.jsonl"), format!("_{thread}.jsonl.gz")];
    let mut inspected = 0_usize;
    let mut matches = BTreeSet::new();
    for folder in folders {
        let entries = fs::read_dir(&folder)
            .map_err(|error| format!("could not read directory {}: {error}", folder.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "could not read an entry beneath {}: {error}",
                    folder.display()
                )
            })?;
            inspected += 1;
            if inspected > STORED_ENTRY_LIMIT {
                return Err(format!(
                    "OMP session-entry search exceeded its {STORED_ENTRY_LIMIT}-entry limit beneath {}",
                    sessions.display()
                ));
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if suffixes.iter().any(|suffix| name.ends_with(suffix))
                && entry
                    .file_type()
                    .map_err(|error| format!("could not inspect {}: {error}", path.display()))?
                    .is_file()
            {
                matches.insert(path);
            }
        }
    }
    Ok(matches.into_iter().collect())
}

fn transcripts_in(folder: &Path, thread: &str) -> Vec<PathBuf> {
    let regular = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    };
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let suffixes = [format!("_{thread}.jsonl"), format!("_{thread}.jsonl.gz")];
    let mut matches = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy());
            name.is_some_and(|name| suffixes.iter().any(|suffix| name.ends_with(suffix)))
                && regular(path)
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

/// The folder oh-my-pi names for `cwd`: `-` and the path relative to the
/// home, `-tmp` and the path relative to the temporary directory, or `--`,
/// the absolute path, `--`, every path resolved through its symlinks first
/// and every separator and `:` replaced by `-`.
fn omp_session_folder(cwd: &Path, environment: &Environment) -> Option<String> {
    let cwd = fs::canonicalize(cwd).ok()?;
    let beneath = |base: Option<&Path>| {
        let base = fs::canonicalize(base?).ok()?;
        cwd.strip_prefix(base).ok().map(omp_dashed)
    };
    Some(if let Some(rest) = beneath(environment.home.as_deref()) {
        format!("-{rest}")
    } else if let Some(rest) = beneath(environment.temp.as_deref()) {
        if rest.is_empty() {
            "-tmp".to_owned()
        } else {
            format!("-tmp-{rest}")
        }
    } else {
        let absolute = cwd
            .strip_prefix(std::path::MAIN_SEPARATOR_STR)
            .unwrap_or(&cwd);
        format!("--{}--", omp_dashed(absolute))
    })
}

fn omp_dashed(path: &Path) -> String {
    path.to_string_lossy().replace(['/', '\\', ':'], "-")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::tests::{Scratch, omp_environment};
    use super::{
        omp_session_folder, related_working_directory_transcripts, working_directory_transcripts,
    };

    const THREAD: &str = "01234567-89ab-cdef-0123-456789abcdef";

    #[test]
    fn omp_names_a_folder_relative_to_the_home_the_temporary_directory_or_the_root() {
        let scratch = Scratch::new("omp-folder");
        let home = scratch.path().join("home");
        let temp = scratch.path().join("temp");
        let elsewhere = scratch.path().join("elsewhere").join("work:tree");
        fs::create_dir_all(home.join("devel").join("worktree")).expect("home worktree exists");
        fs::create_dir_all(temp.join("scratch")).expect("temporary worktree exists");
        fs::create_dir_all(&elsewhere).expect("outside worktree exists");
        let resolved = scratch
            .path()
            .canonicalize()
            .expect("fixture root canonicalizes");
        let absolute = format!(
            "--{}-elsewhere-work-tree--",
            resolved
                .strip_prefix("/")
                .expect("fixture root is absolute")
                .to_string_lossy()
                .replace('/', "-")
        );

        for (cwd, expected) in [
            (
                home.join("devel").join("worktree"),
                "-devel-worktree".to_owned(),
            ),
            (home.clone(), "-".to_owned()),
            (temp.join("scratch"), "-tmp-scratch".to_owned()),
            (temp.clone(), "-tmp".to_owned()),
            (elsewhere.clone(), absolute),
        ] {
            let environment = omp_environment(scratch.path(), &cwd, &home, Some(&temp));
            assert_eq!(
                omp_session_folder(&cwd, &environment).as_deref(),
                Some(expected.as_str()),
                "{}",
                cwd.display()
            );
        }
    }

    #[test]
    fn a_subdirectory_resolves_through_the_nearest_ancestor_folder_holding_the_thread() {
        let scratch = Scratch::new("omp-ancestor-folder");
        let home = scratch.path().join("home");
        let project = home.join("devel").join("project");
        let cwd = project.join("target").join("review").join("cards");
        fs::create_dir_all(&cwd).expect("nested working directory exists");
        let sessions = scratch.path().join("sessions");
        let filed = |folder: &str| {
            let folder = sessions.join(folder);
            fs::create_dir_all(&folder).expect("OMP folder exists");
            let transcript = folder.join(format!("2026-10-02T00-00-00-000Z_{THREAD}.jsonl"));
            fs::write(&transcript, b"\n").expect("transcript exists");
            transcript
        };
        let environment = omp_environment(scratch.path(), &cwd, &home, None);

        assert_eq!(
            working_directory_transcripts(&sessions, THREAD, &environment),
            Vec::<std::path::PathBuf>::new(),
            "no ancestor's folder holds the thread yet"
        );

        let under_project = filed("-devel-project");
        assert_eq!(
            working_directory_transcripts(&sessions, THREAD, &environment),
            vec![under_project.clone()],
            "the worktree root's folder is found from three directories down"
        );

        let under_target = filed("-devel-project-target");
        assert_eq!(
            working_directory_transcripts(&sessions, THREAD, &environment),
            vec![under_target],
            "the nearer ancestor's folder wins over the root's"
        );
    }

    #[test]
    fn sibling_worktree_folders_are_probed_before_stored_sessions_are_scanned() {
        let scratch = Scratch::new("omp-related-folder");
        let home = scratch.path().join("home");
        let cwd = home.join("devel").join("project-z");
        let sibling = home.join("devel").join("project-e");
        fs::create_dir_all(&cwd).expect("current worktree exists");
        fs::create_dir_all(&sibling).expect("sibling worktree exists");
        let sessions = scratch.path().join("sessions");
        let folder = sessions.join("-devel-project-e");
        fs::create_dir_all(&folder).expect("sibling OMP folder exists");
        let transcript = folder.join(format!("2026-10-02T00-00-00-000Z_{THREAD}.jsonl"));
        fs::write(&transcript, b"\n").expect("sibling transcript exists");
        let environment = omp_environment(scratch.path(), &cwd, &home, None);

        assert_eq!(
            related_working_directory_transcripts(&sessions, THREAD, &environment),
            vec![transcript]
        );
    }
}

//! The folder oh-my-pi names for one working directory, and the transcripts
//! it files there.

use std::fs;
use std::path::{Path, PathBuf};

use super::Environment;

/// The transcripts omp keeps for `thread` beneath the folder named by
/// the working directory this process was asked from, when any are there.
///
/// Resolving one's own live thread is by far the most common lookup, and
/// the process asking is, unless it changed directory afterward, still
/// sitting in the exact directory that named the folder -- so this needs no
/// search at all, only that one folder checked under the same regular-file
/// rule the walk admits by. A miss is silent: the caller walks the sessions
/// root exactly as if this were never tried.
pub(super) fn working_directory_transcripts(
    sessions: &Path,
    thread: &str,
    environment: &Environment,
) -> Vec<PathBuf> {
    let Some(cwd) = environment.cwd.as_deref() else {
        return Vec::new();
    };
    let regular = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    };
    let Some(folder) = omp_session_folder(cwd, environment) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(sessions.join(folder)) else {
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
    use super::omp_session_folder;

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
}

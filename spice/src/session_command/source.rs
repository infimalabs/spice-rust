use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use wire::Refusal;

use super::{Environment, model::Vendor};

const THREAD_HEX_LEN: usize = 32;
const SEARCH_DEPTH: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSource {
    pub(crate) path: PathBuf,
    pub(crate) vendor: Vendor,
    pub(crate) thread: Option<String>,
}

pub(crate) fn resolve_inputs(
    values: &[String],
    environment: &Environment,
) -> Result<Vec<ResolvedSource>, Refusal> {
    let resolved = if values.is_empty() {
        resolve_ambient(environment)?
    } else {
        values
            .iter()
            .map(|value| resolve_input(value, environment))
            .collect::<Result<Vec<_>, _>>()?
    };
    deduplicate(resolved)
}

pub(crate) fn resolve_thread(
    value: &str,
    environment: &Environment,
) -> Result<ResolvedSource, Refusal> {
    let thread = canonical_thread(value).ok_or_else(|| {
        refusal(
            "session::thread-invalid",
            Some(value.to_owned()),
            "pass one dashed or dashless hexadecimal UUID thread identity",
        )
    })?;
    let preferred = preferred_vendor(environment);
    let vendors = match preferred {
        Vendor::Claude => [Vendor::Claude, Vendor::Codex],
        _ => [Vendor::Codex, Vendor::Claude],
    };
    let mut failures = Vec::new();
    for vendor in vendors {
        match find_thread(&thread, vendor, environment) {
            Ok(path) => {
                return admit(path, vendor, Some(thread));
            }
            Err(detail) => failures.push(detail),
        }
    }
    Err(refusal(
        "session::thread-unresolved",
        Some(value.to_owned()),
        &format!(
            "restore the owning Codex state index or Claude project transcript, then retry; {}",
            failures.join("; ")
        ),
    ))
}

fn resolve_ambient(environment: &Environment) -> Result<Vec<ResolvedSource>, Refusal> {
    for (variable, raw, vendor) in [
        (
            "CODEX_THREAD_ID", // env-policy: allow
            environment.codex_thread.as_ref(),
            Vendor::Codex,
        ),
        (
            "CLAUDE_CODE_SESSION_ID", // env-policy: allow
            environment.claude_session.as_ref(),
            Vendor::Claude,
        ),
    ] {
        if let Some(raw) = raw {
            let value = raw.to_string_lossy();
            let thread = canonical_thread(&value).ok_or_else(|| {
                refusal(
                    "session::ambient-thread-invalid",
                    Some(variable.to_owned()),
                    "repair the live vendor thread identity or pass an explicit transcript path",
                )
            })?;
            let path = find_thread(&thread, vendor, environment).map_err(|detail| {
                refusal(
                    "session::ambient-transcript-unresolved",
                    Some(thread.clone()),
                    &format!("pass the transcript path explicitly; {detail}"),
                )
            })?;
            return Ok(vec![admit(path, vendor, Some(thread))?]);
        }
    }

    let current = std::env::current_dir().map_err(|error| {
        refusal(
            "session::cwd-unavailable",
            Some(error.to_string()),
            "restore the working directory or pass an absolute transcript path",
        )
    })?;
    let mut paths = fs::read_dir(&current)
        .map_err(|error| {
            refusal(
                "session::cwd-unreadable",
                Some(error.to_string()),
                "restore read access or pass an explicit transcript path",
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| transcript_extension(path))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(refusal(
            "session::transcript-unspecified",
            None,
            "pass a .jsonl or .jsonl.gz path, or run inside a live Codex or Claude agent",
        ));
    }
    paths
        .into_iter()
        .map(|path| admit(path, Vendor::Unknown, None))
        .collect()
}

fn resolve_input(value: &str, environment: &Environment) -> Result<ResolvedSource, Refusal> {
    let expanded = expand_home(value, environment);
    if expanded.exists() {
        return admit(expanded, Vendor::Unknown, None);
    }
    if canonical_thread(value).is_some() {
        return resolve_thread(value, environment);
    }
    Err(refusal(
        "session::transcript-missing",
        Some(expanded.display().to_string()),
        "restore the transcript or pass an existing .jsonl or .jsonl.gz path",
    ))
}

fn admit(path: PathBuf, vendor: Vendor, thread: Option<String>) -> Result<ResolvedSource, Refusal> {
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        refusal(
            "session::transcript-unavailable",
            Some(format!("{}: {error}", path.display())),
            "restore a readable regular transcript file and retry",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(refusal(
            "session::transcript-not-regular",
            Some(path.display().to_string()),
            "pass a real regular .jsonl or .jsonl.gz file, not a link, directory, or device",
        ));
    }
    if !transcript_extension(&path) {
        return Err(refusal(
            "session::transcript-extension",
            Some(path.display().to_string()),
            "pass a transcript ending in .jsonl or .jsonl.gz",
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        refusal(
            "session::transcript-unavailable",
            Some(format!("{}: {error}", path.display())),
            "restore stable read access to the transcript and retry",
        )
    })?;
    Ok(ResolvedSource {
        path: canonical,
        vendor,
        thread,
    })
}

fn deduplicate(values: Vec<ResolvedSource>) -> Result<Vec<ResolvedSource>, Refusal> {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for value in values {
        if seen.insert(value.path.clone()) {
            rows.push(value);
        }
    }
    if rows.is_empty() {
        return Err(refusal(
            "session::transcript-unspecified",
            None,
            "pass at least one distinct transcript path",
        ));
    }
    Ok(rows)
}

fn preferred_vendor(environment: &Environment) -> Vendor {
    match environment
        .preferred_vendor
        .as_deref()
        .map(|value| value.to_string_lossy())
        .as_deref()
    {
        Some("claude") => Vendor::Claude,
        Some("codex") => Vendor::Codex,
        _ if environment.claude_session.is_some() => Vendor::Claude,
        _ => Vendor::Codex,
    }
}

fn find_thread(thread: &str, vendor: Vendor, environment: &Environment) -> Result<PathBuf, String> {
    match vendor {
        Vendor::Codex => find_codex_thread(thread, environment),
        Vendor::Claude => find_claude_thread(thread, environment),
        Vendor::Unknown => Err("unknown vendor cannot own a thread".to_owned()),
    }
}

fn find_codex_thread(thread: &str, environment: &Environment) -> Result<PathBuf, String> {
    let root = environment_root(environment.codex_home.as_deref(), environment, ".codex")?;
    let state = root.join("state_5.sqlite");
    let mut state_failure = None;
    if state.is_file() {
        match codex_state_path(&state, thread) {
            Ok(Some(path)) if path.is_file() => return Ok(path),
            Ok(_) => {}
            Err(error) => state_failure = Some(format!("{}: {error}", state.display())),
        }
    } else {
        state_failure = Some(format!("missing Codex state database {}", state.display()));
    }
    if let Some(path) = find_named_transcript(&root.join("sessions"), thread) {
        return Ok(path);
    }
    Err(state_failure.unwrap_or_else(|| {
        format!("Codex thread {thread} is absent from state and session storage")
    }))
}

fn codex_state_path(state: &Path, thread: &str) -> Result<Option<PathBuf>, String> {
    let connection = Connection::open_with_flags(
        state,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT rollout_path FROM threads WHERE replace(lower(id), '-', '') = ?1 LIMIT 1")
        .map_err(|error| error.to_string())?;
    let mut rows = statement
        .query([thread])
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.next().map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let path = row
        .get::<_, String>(0)
        .map(PathBuf::from)
        .map_err(|error| error.to_string())?;
    Ok(Some(path))
}

fn find_claude_thread(thread: &str, environment: &Environment) -> Result<PathBuf, String> {
    let root = environment_root(environment.claude_home.as_deref(), environment, ".claude")?;
    find_named_transcript(&root.join("projects"), thread).ok_or_else(|| {
        format!(
            "Claude thread {thread} is absent beneath {}",
            root.join("projects").display()
        )
    })
}

fn find_named_transcript(root: &Path, thread: &str) -> Option<PathBuf> {
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut matches = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        if depth > SEARCH_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push((path, depth + 1));
                continue;
            }
            let name = entry.file_name().to_string_lossy().replace('-', "");
            if kind.is_file() && transcript_extension(&path) && name.contains(thread) {
                matches.push(path);
            }
        }
    }
    matches.sort();
    matches.into_iter().next_back()
}

fn environment_root(
    configured: Option<&Path>,
    environment: &Environment,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(value) = configured {
        return Ok(value.to_path_buf());
    }
    environment
        .home
        .as_ref()
        .map(|home| home.join(fallback))
        .ok_or_else(|| format!("the vendor home and HOME are absent for {fallback}"))
}

fn expand_home(value: &str, environment: &Environment) -> PathBuf {
    let Some(rest) = value.strip_prefix("~/") else {
        return PathBuf::from(value);
    };
    environment
        .home
        .as_ref()
        .map_or_else(|| PathBuf::from(value), |home| home.join(rest))
}

fn transcript_extension(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.ends_with(".jsonl") || text.ends_with(".jsonl.gz")
}

pub(crate) fn canonical_thread(value: &str) -> Option<String> {
    let compact = value.trim().replace('-', "").to_ascii_lowercase();
    (compact.len() == THREAD_HEX_LEN && compact.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(compact)
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Refusal {
    Refusal {
        reason: reason.to_owned(),
        witness: Some("session-transcript".to_owned()),
        expected: Some("immutable transcript evidence readable as JSONL".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    }
}
